//! The public client facade.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rr_audio::bandwidth::BandwidthPlan;
use rr_audio::capture::{CaptureControl, CaptureEngine};
use rr_audio::playout::{playout, PlayoutHandle};
use rr_audio::SpeakerStats;
use rr_mumble::client::{ClientConfig, ClientHandle};
use rr_mumble::command::{
    Authenticate, ChannelListen, Disconnect, JoinChannel, SendPluginMessage, SetVoiceTarget, VoiceTargetEntry,
};
use rr_mumble::state::ServerState;
use rr_mumble::transport::tcp::TcpConfig;
use rr_mumble::transport::udp::UdpConfig;
use rr_radio::message::{PttMessage, PAYLOAD_TYPE_PTT, PLUGIN_NAME, REFRESH_MS};
use rr_radio::{HoldAction, ListenIn, StationPtt, WhisperHold};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::config::{ClientBuilder, ClientSettings};
use crate::event::{ChannelInfo, Event, UserInfo};
use crate::handler::Handler;
use crate::io::{AudioEndpoints, InputPipe};
use crate::shared::{Shared, WHISPER_SLOT};

/// Errors from client operations.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Not connected to a server.
    #[error("not connected")]
    NotConnected,
    /// The target lies outside the configured scope.
    #[error("channel {0} is outside the allowed scope")]
    OutOfScope(u32),
    /// The audio engine could not be created.
    #[error("audio: {0}")]
    Audio(#[from] rr_audio::AudioError),
    /// Protocol-level failure.
    #[error("mumble: {0}")]
    Mumble(#[from] rr_mumble::error::Error),
}

/// A headless `RemoteRadio` client. Cheap to clone; all clones share one connection.
#[derive(Debug, Clone)]
pub struct RadioClient {
    shared: Arc<Shared>,
    holds: Arc<Mutex<(ListenIn, WhisperHold)>>,
}

impl RadioClient {
    /// Start configuring a client.
    #[must_use]
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Create a client and its audio endpoints. Nothing connects until
    /// [`RadioClient::start`] is called.
    pub fn new(settings: ClientSettings) -> Result<(Self, AudioEndpoints), ClientError> {
        let (engine, control) = CaptureEngine::new(settings.capture)?;
        let (output, play) = playout(settings.output_rate);
        let shared = Shared::new(settings, play, control);
        shared.replan();
        let client = Self { shared: Arc::clone(&shared), holds: Arc::default() };
        let endpoints = AudioEndpoints { input: InputPipe::new(engine, shared), output };
        Ok((client, endpoints))
    }

    /// Connect (and keep reconnecting if enabled). Must be called from inside a
    /// Tokio runtime.
    pub fn start(&self) -> JoinHandle<()> {
        let rt = tokio::runtime::Handle::current();
        let _ = self.shared.rt.set(rt);
        self.shared.shutdown.store(false, Ordering::Relaxed);
        tokio::spawn(supervise(Arc::clone(&self.shared)))
    }

    /// Log out and stop reconnecting.
    pub async fn disconnect(&self) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
        if let Some(h) = self.shared.handle() {
            let _ = h.send(Disconnect).await;
        }
    }

    /// Subscribe to events.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.shared.events.subscribe()
    }

    /// Users visible inside the scope.
    #[must_use]
    pub fn users(&self) -> Vec<UserInfo> {
        self.shared.users()
    }

    /// Channels visible inside the scope.
    #[must_use]
    pub fn channels(&self) -> Vec<ChannelInfo> {
        self.shared.channels()
    }

    /// Our session id once logged in.
    #[must_use]
    pub fn own_session(&self) -> Option<u32> {
        self.shared.state().own_session()
    }

    /// The channel we are in.
    #[must_use]
    pub fn own_channel(&self) -> Option<u32> {
        let st = self.shared.state();
        self.shared.own_channel(&st)
    }

    /// Live capture controls (gain, TX mode, VOX, meters).
    #[must_use]
    pub fn capture(&self) -> Arc<CaptureControl> {
        Arc::clone(&self.shared.capture)
    }

    /// Live playout controls (volumes, meters, stereo mode).
    #[must_use]
    pub fn playout(&self) -> PlayoutHandle {
        self.shared.playout.clone()
    }

    /// Per-talker playout statistics.
    #[must_use]
    pub fn speakers(&self) -> Vec<SpeakerStats> {
        self.shared.playout.stats()
    }

    /// The current encoder plan, including whether the server limit clamped it.
    #[must_use]
    pub fn bandwidth_plan(&self) -> Option<BandwidthPlan> {
        self.shared.current_plan()
    }

    /// Packets handed to the network and packets dropped because it was busy.
    #[must_use]
    pub fn packet_counts(&self) -> (u64, u64) {
        (self.shared.sent.load(Ordering::Relaxed), self.shared.dropped.load(Ordering::Relaxed))
    }

    /// Change the requested quality; the plan is recomputed against the server limit.
    pub fn set_quality(&self, bitrate_bps: u32, stereo: bool, max_frame_ms: u32) {
        self.shared.bitrate.store(bitrate_bps, Ordering::Relaxed);
        self.shared.stereo.store(stereo, Ordering::Relaxed);
        self.shared.max_frame.store(max_frame_ms, Ordering::Relaxed);
        self.shared.replan();
    }

    fn handle(&self) -> Result<ClientHandle, ClientError> {
        self.shared.handle().ok_or(ClientError::NotConnected)
    }

    fn check_scope(&self, channel: u32) -> Result<(), ClientError> {
        let st = self.shared.state();
        if self.shared.in_scope(&st, channel) {
            Ok(())
        } else {
            Err(ClientError::OutOfScope(channel))
        }
    }

    /// Move to another channel.
    pub async fn join_channel(&self, channel_id: u32) -> Result<(), ClientError> {
        self.check_scope(channel_id)?;
        self.handle()?.send(JoinChannel { channel_id, password: None }).await?;
        Ok(())
    }

    /// Carry out hold-hotkey actions.
    pub async fn apply(&self, actions: Vec<HoldAction>) -> Result<(), ClientError> {
        for a in actions {
            match a {
                HoldAction::ListenAdd(c) => {
                    self.check_scope(c)?;
                    self.handle()?.send(ChannelListen { add: vec![c], remove: Vec::new() }).await?;
                }
                HoldAction::ListenRemove(c) => {
                    self.handle()?.send(ChannelListen { add: Vec::new(), remove: vec![c] }).await?;
                }
                HoldAction::Whisper(Some(c)) => {
                    self.check_scope(c)?;
                    let entry = VoiceTargetEntry {
                        sessions: Vec::new(),
                        channel_id: Some(c),
                        group: None,
                        links: false,
                        children: false,
                    };
                    self.handle()?.send(SetVoiceTarget { id: WHISPER_SLOT, targets: vec![entry] }).await?;
                    self.shared.target.store(WHISPER_SLOT, Ordering::Relaxed);
                }
                HoldAction::Whisper(None) => self.shared.target.store(0, Ordering::Relaxed),
                HoldAction::MuteHome(m) => self.shared.mute_home.store(m, Ordering::Relaxed),
            }
        }
        Ok(())
    }

    /// Listen In hotkey: while held you also hear `target`.
    pub async fn listen_in(&self, target: u32, down: bool, mute_home: bool) -> Result<(), ClientError> {
        let actions = {
            let mut holds = self.holds.lock().unwrap_or_else(PoisonError::into_inner);
            if down {
                let home = self.own_channel().unwrap_or(0);
                holds.0.press(target, home, mute_home)
            } else {
                holds.0.release()
            }
        };
        self.apply(actions).await
    }

    /// Whisper hotkey: while held your voice goes to `target` only.
    pub async fn whisper(&self, target: u32, down: bool) -> Result<(), ClientError> {
        let actions = {
            let mut holds = self.holds.lock().unwrap_or_else(PoisonError::into_inner);
            if down {
                let home = self.own_channel().unwrap_or(0);
                holds.1.press(target, home)
            } else {
                holds.1.release()
            }
        };
        self.apply(actions).await
    }

    /// Press or release PTT. Opens the local transmit gate (in PTT mode) and
    /// tells the channel's station to key its rig, repeating while held so a
    /// lost release cannot leave the rig on the air.
    pub fn set_ptt(&self, down: bool) {
        let sh = &self.shared;
        sh.capture.set_ptt(down);
        if sh.ptt_down.swap(down, Ordering::Relaxed) == down {
            return;
        }
        let seq = sh.ptt_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let msg = PttMessage { down, seq };
        send_ptt(sh, msg);
        if !down {
            return;
        }
        if let Some(rt) = sh.rt.get() {
            let _task = rt.spawn(ptt_refresh(Arc::clone(sh), seq, msg));
        }
    }

    /// Drive a rig from this client: remote PTT messages and (if the station
    /// has VOX enabled) inbound audio key it. The returned task ends when the
    /// client is dropped.
    pub fn attach_station(&self, station: StationPtt) -> JoinHandle<()> {
        let shared = Arc::clone(&self.shared);
        let mut rx = self.subscribe();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(10));
            let mut active = false;
            loop {
                tokio::select! {
                    ev = rx.recv() => match ev {
                        Ok(Event::RemotePtt { down, .. }) => station.set_remote(down),
                        Ok(Event::Disconnected { .. }) => {
                            station.set_remote(false);
                            station.set_audio_active(false);
                            active = false;
                        }
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                    _ = tick.tick() => {
                        let now_active = shared.activity.is_active();
                        if now_active != active {
                            active = now_active;
                            station.set_audio_active(active);
                        }
                    }
                }
            }
        })
    }
}

/// Re-send a held PTT until it is released or superseded.
async fn ptt_refresh(shared: Arc<Shared>, seq: u64, msg: PttMessage) {
    loop {
        tokio::time::sleep(Duration::from_millis(REFRESH_MS)).await;
        let still_held = shared.ptt_down.load(Ordering::Relaxed) && shared.ptt_seq.load(Ordering::Relaxed) == seq;
        if !still_held {
            break;
        }
        send_ptt(&shared, msg);
    }
}

fn send_ptt(sh: &Arc<Shared>, msg: PttMessage) {
    let channel = {
        let st = sh.state();
        sh.own_channel(&st)
    };
    let (Some(h), Some(rt)) = (sh.handle(), sh.rt.get()) else {
        return;
    };
    let _task = rt.spawn(async move {
        let _ = h
            .send(SendPluginMessage {
                plugin_name: PLUGIN_NAME.to_owned(),
                payload_type: PAYLOAD_TYPE_PTT.to_owned(),
                payload: msg.encode(),
                target_sessions: Vec::new(),
                channel_id: channel,
            })
            .await;
    });
}

/// Connection supervisor: connect, log in, wait, back off, repeat.
async fn supervise(shared: Arc<Shared>) {
    let (mut a, mut b) = (1_u64, 1_u64);
    while !shared.shutdown.load(Ordering::Relaxed) {
        shared.emit(Event::Connecting);
        *shared.state() = ServerState::new();
        shared.ptt_filters().clear();
        let s = &shared.settings;
        let cfg = ClientConfig {
            tcp: TcpConfig { server_host: s.host.clone(), server_port: s.port, ..TcpConfig::default() },
            udp: UdpConfig { server_host: s.host.clone(), server_port: s.port },
            force_tcp: s.force_tcp,
            ..ClientConfig::default()
        };
        let started = Instant::now();
        match rr_mumble::client::run(cfg, Handler { shared: Arc::clone(&shared) }).await {
            Ok((handle, join)) => {
                shared.set_handle(Some(handle.clone()));
                let login = Authenticate {
                    username: s.username.clone(),
                    password: s.password.clone(),
                    tokens: s.tokens.clone(),
                };
                if handle.send(login).await.is_ok() {
                    let _ = join.await;
                }
                shared.set_handle(None);
                shared.emit(Event::Disconnected { reason: "connection closed".into() });
            }
            Err(e) => shared.emit(Event::Disconnected { reason: e.to_string() }),
        }
        if shared.shutdown.load(Ordering::Relaxed) || !s.auto_reconnect {
            break;
        }
        if started.elapsed() > Duration::from_secs(10) {
            (a, b) = (1, 1);
        }
        tokio::time::sleep(Duration::from_secs(a)).await;
        (a, b) = (b, (a + b).min(30));
    }
}

impl ClientBuilder {
    /// Build the client and its audio endpoints.
    pub fn build(self) -> Result<(RadioClient, AudioEndpoints), ClientError> {
        RadioClient::new(self.settings)
    }
}
