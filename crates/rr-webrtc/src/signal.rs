//! Transport-agnostic signaling.
//!
//! The SDP/ICE handshake needs a side channel before the peer-to-peer link
//! exists. Production uses the Mumble `WebRtcSignal` relay (wire ID 120, which
//! already carries exactly these message kinds); tests use
//! [`InMemorySignaling`]. Both implement [`SignalTransport`], so the session
//! logic is identical in either case and the whole handshake is testable with
//! no server.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};

use crate::{Result, TransportError};

/// One signaling message exchanged between two peers.
///
/// The variants deliberately mirror the `SignalType` enum already defined by
/// the Mumble `WebRtcSignal` message, so mapping to and from the wire is a
/// direct correspondence rather than a translation layer.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignalMessage {
    /// Announce availability / request a session (`SignalType::START`).
    Start {
        /// Optional JSON presence payload (see `rr_proto::Presence`).
        payload: String,
    },
    /// Tear down a session (`SignalType::STOP`).
    Stop,
    /// SDP offer from the Client/viewer (`SignalType::SDP_OFFER`).
    Offer {
        /// The SDP text.
        sdp: String,
    },
    /// SDP answer from the Station/host (`SignalType::SDP_ANSWER`).
    Answer {
        /// The SDP text.
        sdp: String,
    },
    /// A trickled ICE candidate (`SignalType::ICE_CANDIDATE`).
    IceCandidate {
        /// JSON-encoded `RTCIceCandidateInit`.
        candidate: String,
    },
}

/// A bidirectional signaling channel to exactly one remote peer.
///
/// Implementations are expected to be cheap to clone/share behind an `Arc`.
// `async_trait` desugars each method to one returning `Pin<Box<dyn Future>>` and
// marks it `#[must_use]` to keep `async fn`'s semantics. A boxed future is
// already `#[must_use]`, so clippy sees the attribute twice and fires
// `double_must_use` — on generated code, at the span of the attribute macro, with
// nothing in this file to change. Native `async fn` in traits would avoid the
// macro but is not `dyn`-compatible, and this trait is used behind `Arc<dyn ..>`.
#[allow(
    clippy::double_must_use,
    reason = "emitted by the `async_trait` expansion, not by this code"
)]
#[async_trait]
pub trait SignalTransport: Send + Sync + 'static {
    /// Send a message to the remote peer.
    async fn send(&self, msg: SignalMessage) -> Result<()>;

    /// Await the next message from the remote peer.
    ///
    /// Returns `None` when the channel is closed and no messages remain, so
    /// callers can drive it with `while let Some(msg) = t.recv().await`.
    async fn recv(&self) -> Option<SignalMessage>;
}

/// An in-process [`SignalTransport`] pair for tests and local loopback.
///
/// [`InMemorySignaling::pair`] returns two endpoints wired head-to-tail: what
/// one sends, the other receives.
#[derive(Debug)]
pub struct InMemorySignaling {
    tx: mpsc::UnboundedSender<SignalMessage>,
    rx: Mutex<mpsc::UnboundedReceiver<SignalMessage>>,
}

impl InMemorySignaling {
    /// Create two connected endpoints.
    #[must_use]
    pub fn pair() -> (Arc<Self>, Arc<Self>) {
        let (a_tx, a_rx) = mpsc::unbounded_channel();
        let (b_tx, b_rx) = mpsc::unbounded_channel();

        // Endpoint A sends into B's queue and reads from its own.
        let a = Arc::new(Self {
            tx: b_tx,
            rx: Mutex::new(a_rx),
        });
        let b = Arc::new(Self {
            tx: a_tx,
            rx: Mutex::new(b_rx),
        });
        (a, b)
    }
}

#[async_trait]
impl SignalTransport for InMemorySignaling {
    async fn send(&self, msg: SignalMessage) -> Result<()> {
        self.tx
            .send(msg)
            .map_err(|e| TransportError::Signaling(format!("peer endpoint dropped: {e}")))
    }

    async fn recv(&self) -> Option<SignalMessage> {
        self.rx.lock().await.recv().await
    }
}

#[cfg(test)]
mod tests;
