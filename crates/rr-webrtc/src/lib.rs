//! `RemoteRadio` peer-to-peer WebRTC transport.
//!
//! This crate owns the peer-connection lifecycle for a single remote-desktop
//! session leg: offer/answer negotiation, trickle ICE, and the `rr-control`
//! data channel that carries [`rr_proto::ControlMsg`] frames.
//!
//! # Roles
//!
//! Per the spec, the **Client (viewer) is the offerer** and the **Station
//! (host) is the answerer**. This keeps the Station passive until a viewer
//! asks for a session and scales naturally to several independent viewers,
//! each with its own peer connection.
//!
//! # Signaling is abstracted
//!
//! Media and data are peer-to-peer, but the SDP/ICE handshake needs a side
//! channel. In production that is the Mumble `WebRtcSignal` relay; in tests it
//! is an in-process channel. Both are expressed through [`SignalTransport`], so
//! this crate never depends on Mumble and the full handshake is testable
//! without a server.
//!
//! # Example flow
//!
//! ```text
//! Client                          Station
//!   PeerSession::offer()            PeerSession::answer()
//!   ├─ creates rr-control DC        │
//!   ├─ create_offer ──────SDP──────▶ set_remote_description
//!   │                               ├─ create_answer
//!   set_remote_description ◀──SDP───┤
//!   └─ trickle ICE ◀────────────────┴─ trickle ICE
//! ```

#![forbid(unsafe_code)]

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};

mod session;
mod signal;
mod station;
mod video;

pub use session::{ConnectionState, PeerSession, SessionConfig};
pub use signal::{InMemorySignaling, SignalMessage, SignalTransport};
pub use station::{ControlSink, GateObserver, SharedSink, StationControl, StationEvent};
pub use video::{
    VideoReceiver, VideoSender, H264_PAYLOAD_TYPE, VIDEO_STREAM_ID, VIDEO_TRACK_ID,
};

/// Default public STUN servers used for server-reflexive candidate discovery.
///
/// TURN relay configuration is added in a later phase; these only help with
/// direct hole-punching.
pub const DEFAULT_STUN_SERVERS: &[&str] = &[
    "stun:stun.l.google.com:19302",
    "stun:stun1.l.google.com:19302",
];

/// Label of the reliable, ordered data channel carrying control traffic
/// (input events, handoff, monitor selection, keyframe requests).
pub const CONTROL_CHANNEL_LABEL: &str = "rr-control";

/// Errors produced by the transport layer.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The underlying `WebRTC` stack reported an error.
    #[error("webrtc error: {0}")]
    WebRtc(String),
    /// Signaling could not deliver or parse a message.
    #[error("signaling error: {0}")]
    Signaling(String),
    /// A wire-protocol (`rr-proto`) encode/decode failure.
    #[error("protocol error: {0}")]
    Proto(#[from] rr_proto::ProtoError),
    /// The control data channel is not open yet (or has closed).
    #[error("control channel unavailable")]
    ControlChannelUnavailable,
    /// An operation exceeded its deadline.
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
}

/// Convenience result alias for this crate.
pub type Result<T> = std::result::Result<T, TransportError>;

/// Callbacks delivered to the application as a session progresses.
///
/// Implementations must be cheap and non-blocking; do real work on a task.
// See the matching note on `SignalTransport`: `double_must_use` fires on the
// `async_trait` expansion, not on anything written here.
#[allow(
    clippy::double_must_use,
    reason = "emitted by the `async_trait` expansion, not by this code"
)]
#[async_trait]
pub trait SessionObserver: Send + Sync + 'static {
    /// The peer connection's aggregate state changed.
    async fn on_connection_state(&self, _state: ConnectionState) {}

    /// A decoded control message arrived on the `rr-control` channel.
    async fn on_control(&self, _msg: rr_proto::ControlMsg) {}

    /// The control channel became usable in both directions.
    async fn on_control_open(&self) {}
}

/// A [`SessionObserver`] that records everything, for tests and diagnostics.
#[derive(Debug, Default)]
pub struct RecordingObserver {
    states: Mutex<Vec<ConnectionState>>,
    control: Mutex<Vec<rr_proto::ControlMsg>>,
    opened: Mutex<bool>,
    control_tx: Mutex<Option<mpsc::UnboundedSender<rr_proto::ControlMsg>>>,
}

impl RecordingObserver {
    /// Create an empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a recorder that also forwards control messages to `tx`, so a
    /// test can `await` the next message instead of polling.
    #[must_use]
    pub fn with_channel(tx: mpsc::UnboundedSender<rr_proto::ControlMsg>) -> Self {
        Self {
            control_tx: Mutex::new(Some(tx)),
            ..Self::default()
        }
    }

    /// Snapshot of observed connection states, oldest first.
    pub async fn states(&self) -> Vec<ConnectionState> {
        self.states.lock().await.clone()
    }

    /// Snapshot of received control messages, oldest first.
    pub async fn control_messages(&self) -> Vec<rr_proto::ControlMsg> {
        self.control.lock().await.clone()
    }

    /// Whether the control channel has reported open.
    pub async fn control_opened(&self) -> bool {
        *self.opened.lock().await
    }
}

#[async_trait]
impl SessionObserver for RecordingObserver {
    async fn on_connection_state(&self, state: ConnectionState) {
        self.states.lock().await.push(state);
    }

    async fn on_control(&self, msg: rr_proto::ControlMsg) {
        if let Some(tx) = self.control_tx.lock().await.as_ref() {
            // A closed receiver is not an error here: the recorder still keeps
            // the message for later inspection.
            let _ = tx.send(msg.clone());
        }
        self.control.lock().await.push(msg);
    }

    async fn on_control_open(&self) {
        *self.opened.lock().await = true;
    }
}

/// A no-op observer for callers that drive everything themselves.
#[derive(Debug, Default)]
pub struct NullObserver;

impl SessionObserver for NullObserver {}

/// Shared handle to an observer.
pub type SharedObserver = Arc<dyn SessionObserver>;
