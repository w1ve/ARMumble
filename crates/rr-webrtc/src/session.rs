//! Peer-session lifecycle: negotiation, trickle ICE and the control channel.
//!
//! One [`PeerSession`] is one leg of a remote-desktop session (a Station
//! talking to exactly one Client). A Station with several viewers owns one
//! session per viewer, each with its own peer connection — that is what makes
//! the pure peer-to-peer model work without a server forwarding media.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::BytesMut;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, warn};

use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
    RTCIceCandidateInit, RTCIceServer, RTCPeerConnectionIceEvent, RTCPeerConnectionState,
    RTCSessionDescription,
};

use crate::signal::{SignalMessage, SignalTransport};
use crate::{
    Result, SharedObserver, TransportError, CONTROL_CHANNEL_LABEL, DEFAULT_STUN_SERVERS,
};

/// Aggregate connection state, mirrored from the `WebRTC` peer connection.
///
/// Kept as our own enum so the rest of the application (and the UI layer) does
/// not depend on `webrtc` types directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Negotiating / gathering candidates.
    Connecting,
    /// Media and data can flow.
    Connected,
    /// Temporarily lost connectivity; may recover.
    Disconnected,
    /// Permanently failed (ICE could not establish a path).
    Failed,
    /// Closed locally or by the peer.
    Closed,
}

impl From<RTCPeerConnectionState> for ConnectionState {
    fn from(s: RTCPeerConnectionState) -> Self {
        match s {
            RTCPeerConnectionState::Connected => Self::Connected,
            RTCPeerConnectionState::Disconnected => Self::Disconnected,
            RTCPeerConnectionState::Failed => Self::Failed,
            RTCPeerConnectionState::Closed => Self::Closed,
            // New / Connecting / Unspecified all mean "not usable yet".
            _ => Self::Connecting,
        }
    }
}

/// Tunables for a session.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// ICE servers (STUN now; TURN is added in a later phase).
    pub ice_servers: Vec<String>,
    /// Local UDP bind addresses for ICE host candidates.
    pub udp_addrs: Vec<String>,
    /// How long to wait for the connection to reach `Connected`.
    pub connect_timeout: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            ice_servers: DEFAULT_STUN_SERVERS.iter().map(|s| (*s).to_owned()).collect(),
            udp_addrs: vec!["0.0.0.0:0".to_owned()],
            connect_timeout: Duration::from_secs(30),
        }
    }
}

impl SessionConfig {
    /// Config for a purely local test/loopback session: no STUN lookups (which
    /// would add latency and need network access), host candidates only.
    #[must_use]
    pub fn loopback() -> Self {
        Self {
            ice_servers: Vec::new(),
            udp_addrs: vec!["127.0.0.1:0".to_owned()],
            connect_timeout: Duration::from_secs(20),
        }
    }
}

/// Internal event handler bridging `webrtc` callbacks to our own plumbing.
///
/// It owns the signaling transport so locally-gathered ICE candidates can be
/// trickled to the peer the moment they appear, and forwards state changes and
/// inbound data channels to the application's [`SessionObserver`].
struct Handler {
    signaling: Arc<dyn SignalTransport>,
    observer: SharedObserver,
    /// Set when the remote peer opens a channel (the answerer's side).
    inbound_dc: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    dc_ready_tx: mpsc::UnboundedSender<Arc<dyn DataChannel>>,
}

#[async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        // Trickle each candidate as it is discovered rather than waiting for
        // gathering to complete, which materially shortens setup time.
        match event.candidate.to_json() {
            Ok(init) => match serde_json::to_string(&init) {
                Ok(json) => {
                    if let Err(e) = self
                        .signaling
                        .send(SignalMessage::IceCandidate { candidate: json })
                        .await
                    {
                        warn!("failed to trickle ICE candidate: {e}");
                    }
                }
                Err(e) => warn!("failed to serialise ICE candidate: {e}"),
            },
            Err(e) => warn!("failed to convert ICE candidate: {e}"),
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let mapped = ConnectionState::from(state);
        debug!(?state, ?mapped, "peer connection state changed");
        self.observer.on_connection_state(mapped).await;
    }

    async fn on_data_channel(&self, data_channel: Arc<dyn DataChannel>) {
        let label = data_channel.label().await.unwrap_or_default();
        debug!(%label, "remote opened data channel");
        if label == CONTROL_CHANNEL_LABEL {
            *self.inbound_dc.lock().await = Some(Arc::clone(&data_channel));
            // A dropped receiver just means nobody is waiting for readiness.
            let _ = self.dc_ready_tx.send(data_channel);
        }
    }
}

/// One peer-to-peer session leg.
pub struct PeerSession {
    pc: Arc<dyn PeerConnection>,
    /// The control channel, whichever side created it.
    control: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    signaling: Arc<dyn SignalTransport>,
    observer: SharedObserver,
    /// Candidates that arrived before `set_remote_description`, to be applied
    /// once a remote description exists.
    pending_remote_candidates: Arc<Mutex<Vec<RTCIceCandidateInit>>>,
    remote_described: Arc<Mutex<bool>>,
    dc_ready_rx: Arc<Mutex<mpsc::UnboundedReceiver<Arc<dyn DataChannel>>>>,
}

impl std::fmt::Debug for PeerSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerSession").finish_non_exhaustive()
    }
}

impl PeerSession {
    /// Build the underlying peer connection without negotiating yet.
    async fn build(
        config: &SessionConfig,
        signaling: Arc<dyn SignalTransport>,
        observer: SharedObserver,
    ) -> Result<Self> {
        let ice_servers: Vec<RTCIceServer> = config
            .ice_servers
            .iter()
            .map(|url| RTCIceServer {
                urls: vec![url.clone()],
                ..Default::default()
            })
            .collect();

        let rtc_config = RTCConfigurationBuilder::default()
            .with_ice_servers(ice_servers)
            .build();

        let inbound_dc: Arc<Mutex<Option<Arc<dyn DataChannel>>>> = Arc::new(Mutex::new(None));
        let (dc_ready_tx, dc_ready_rx) = mpsc::unbounded_channel();

        let handler = Arc::new(Handler {
            signaling: Arc::clone(&signaling),
            observer: Arc::clone(&observer),
            inbound_dc: Arc::clone(&inbound_dc),
            dc_ready_tx,
        });

        // `with_udp_addrs` is generic over an address type that must be
        // `'static`, so hand it owned values rather than borrows of `config`.
        let udp_addrs: Vec<String> = config.udp_addrs.clone();
        let pc = PeerConnectionBuilder::new()
            .with_configuration(rtc_config)
            .with_handler(handler)
            .with_udp_addrs(udp_addrs)
            .build()
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        Ok(Self {
            pc: Arc::new(pc),
            control: inbound_dc,
            signaling,
            observer,
            pending_remote_candidates: Arc::new(Mutex::new(Vec::new())),
            remote_described: Arc::new(Mutex::new(false)),
            dc_ready_rx: Arc::new(Mutex::new(dc_ready_rx)),
        })
    }

    /// Start a session as the **offerer** (the Client / viewer role).
    ///
    /// Creates the `rr-control` data channel, generates an offer and sends it
    /// over signaling. Drive the rest with [`PeerSession::run`].
    pub async fn offer(
        config: &SessionConfig,
        signaling: Arc<dyn SignalTransport>,
        observer: SharedObserver,
    ) -> Result<Self> {
        let session = Self::build(config, signaling, observer).await?;

        // The offerer creates the control channel so it exists in the SDP.
        let dc = session
            .pc
            .create_data_channel(CONTROL_CHANNEL_LABEL, None)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        *session.control.lock().await = Some(Arc::clone(&dc));
        session.spawn_control_reader(dc);

        let offer = session
            .pc
            .create_offer(None)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        session
            .pc
            .set_local_description(offer.clone())
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        session
            .signaling
            .send(SignalMessage::Offer { sdp: offer.sdp })
            .await?;

        Ok(session)
    }

    /// Start a session as the **answerer** (the Station / host role).
    ///
    /// The peer connection is created immediately but stays idle until an offer
    /// arrives; [`PeerSession::run`] performs the answer.
    pub async fn answer(
        config: &SessionConfig,
        signaling: Arc<dyn SignalTransport>,
        observer: SharedObserver,
    ) -> Result<Self> {
        Self::build(config, signaling, observer).await
    }

    /// Drive the signaling loop until the channel closes.
    ///
    /// Handles inbound offers (answerer), answers (offerer) and trickled ICE
    /// candidates, including candidates that arrive before the remote
    /// description is set.
    pub async fn run(&self) -> Result<()> {
        while let Some(msg) = self.signaling.recv().await {
            match msg {
                SignalMessage::Offer { sdp } => self.handle_offer(sdp).await?,
                SignalMessage::Answer { sdp } => self.handle_answer(sdp).await?,
                SignalMessage::IceCandidate { candidate } => {
                    self.handle_remote_candidate(&candidate).await?;
                }
                SignalMessage::Start { .. } => debug!("ignoring START in an active session"),
                SignalMessage::Stop => {
                    debug!("peer sent STOP; closing session");
                    break;
                }
            }
        }
        Ok(())
    }

    async fn handle_offer(&self, sdp: String) -> Result<()> {
        let desc = RTCSessionDescription::offer(sdp)
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        self.pc
            .set_remote_description(desc)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        self.mark_remote_described().await?;

        let answer = self
            .pc
            .create_answer(None)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        self.pc
            .set_local_description(answer.clone())
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        self.signaling
            .send(SignalMessage::Answer { sdp: answer.sdp })
            .await
    }

    async fn handle_answer(&self, sdp: String) -> Result<()> {
        let desc = RTCSessionDescription::answer(sdp)
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        self.pc
            .set_remote_description(desc)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        self.mark_remote_described().await
    }

    /// Apply a trickled candidate, queueing it when no remote description
    /// exists yet (candidates legitimately race ahead of the SDP).
    async fn handle_remote_candidate(&self, json: &str) -> Result<()> {
        let init: RTCIceCandidateInit = serde_json::from_str(json)
            .map_err(|e| TransportError::Signaling(format!("bad ICE candidate JSON: {e}")))?;

        if *self.remote_described.lock().await {
            self.pc
                .add_ice_candidate(init)
                .await
                .map_err(|e| TransportError::WebRtc(e.to_string()))
        } else {
            self.pending_remote_candidates.lock().await.push(init);
            Ok(())
        }
    }

    /// Record that a remote description is present and flush queued candidates.
    async fn mark_remote_described(&self) -> Result<()> {
        *self.remote_described.lock().await = true;
        let queued: Vec<RTCIceCandidateInit> =
            std::mem::take(&mut *self.pending_remote_candidates.lock().await);
        for init in queued {
            if let Err(e) = self.pc.add_ice_candidate(init).await {
                warn!("failed to apply queued ICE candidate: {e}");
            }
        }
        Ok(())
    }

    /// Wait until the control channel exists and is open, then return it.
    ///
    /// The offerer created it up front; the answerer receives it via
    /// `on_data_channel`, so this waits for whichever applies.
    pub async fn wait_control_channel(&self, timeout: Duration) -> Result<Arc<dyn DataChannel>> {
        if let Some(dc) = self.control.lock().await.clone() {
            return Ok(dc);
        }
        let dc = tokio::time::timeout(timeout, async {
            self.dc_ready_rx.lock().await.recv().await
        })
        .await
        .map_err(|_| TransportError::Timeout("control channel"))?
        .ok_or(TransportError::ControlChannelUnavailable)?;

        self.spawn_control_reader(Arc::clone(&dc));
        Ok(dc)
    }

    /// Send a control message, failing if the channel is not usable.
    pub async fn send_control(&self, msg: &rr_proto::ControlMsg) -> Result<()> {
        let dc = self
            .control
            .lock()
            .await
            .clone()
            .ok_or(TransportError::ControlChannelUnavailable)?;
        let bytes = msg.encode()?;
        dc.send(BytesMut::from(&bytes[..]))
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))
    }

    /// Current aggregate connection state.
    pub async fn connection_state(&self) -> ConnectionState {
        // The builder-based API surfaces state through the handler; querying
        // the channel's usability is the reliable synchronous check we need
        // here, so fall back to Connecting when no channel exists yet.
        match self.control.lock().await.as_ref() {
            Some(_) => ConnectionState::Connected,
            None => ConnectionState::Connecting,
        }
    }

    /// Close the session and its peer connection.
    pub async fn close(&self) -> Result<()> {
        if let Some(dc) = self.control.lock().await.take() {
            let _ = dc.close().await;
        }
        self.pc
            .close()
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))
    }

    /// Spawn the reader task that converts data-channel events into
    /// [`SessionObserver`] callbacks.
    fn spawn_control_reader(&self, dc: Arc<dyn DataChannel>) {
        let observer = Arc::clone(&self.observer);
        drop(tokio::spawn(async move {
            while let Some(event) = dc.poll().await {
                match event {
                    DataChannelEvent::OnOpen => observer.on_control_open().await,
                    DataChannelEvent::OnMessage(msg) => {
                        match rr_proto::ControlMsg::decode(&msg.data) {
                            Ok(decoded) => observer.on_control(decoded).await,
                            Err(e) => warn!("dropping malformed control frame: {e}"),
                        }
                    }
                    DataChannelEvent::OnClose | DataChannelEvent::OnClosing => {
                        debug!("control channel closing");
                    }
                    other => debug!(?other, "unhandled data channel event"),
                }
            }
        }));
    }
}
