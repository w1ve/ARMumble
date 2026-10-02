//! Station-side control fabric: one [`ControlGate`] shared across many viewers.
//!
//! A [`PeerSession`](crate::PeerSession) is a single leg — one Station talking
//! to one viewer — and has no idea that other viewers exist. Arbitration is the
//! opposite: deciding *who* among several viewers may drive the desktop is only
//! meaningful across all of them at once. [`StationControl`] is where those two
//! shapes meet.
//!
//! # What it owns
//!
//! - One [`ControlGate`], behind a mutex, shared by every connected viewer. The
//!   gate is the single authority on who controls input and the only path to a
//!   real keystroke.
//! - A registry mapping each viewer's [`SessionId`] to the sink that reaches
//!   that specific peer, so a targeted notification goes to one viewer and an
//!   authoritative state broadcast goes to all of them.
//! - An event channel ([`StationEvent`]) surfacing the two things the gate
//!   cannot act on itself — a keyframe request and a monitor switch — to the
//!   capture/codec layer that lives above this crate.
//!
//! # The flow
//!
//! ```text
//!   viewer A ─┐                         ┌─ inject (if A holds control)
//!   viewer B ─┤  on_control(session,msg)│
//!   viewer C ─┴──▶ ControlGate.handle ──┼─ Broadcast ControlState → A, B, C
//!                                        ├─ Notify requester        → one viewer
//!                                        └─ keyframe / monitor       → StationEvent
//! ```
//!
//! # Injector-agnostic by construction
//!
//! `StationControl<I>` is generic over the gate's injector `I`. The Station app
//! instantiates it with `EnigoInjector` (behind `rr-input`'s `inject` feature);
//! the tests in this crate instantiate it with `RecordingInjector` and assert
//! on exactly what was and was not injected — no display or input system
//! required. That is why this crate never names a concrete injector.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, warn};

use rr_input::{ControlGate, InputInjector};
use rr_proto::ControlMsg;
use rr_session::{ControlState, Effect, RejectReason, SessionId};

use crate::{ConnectionState, SessionObserver};

/// Something that can deliver a [`ControlMsg`] to one viewer.
///
/// A real [`PeerSession`](crate::PeerSession) sends over its `rr-control` data
/// channel; a test uses an in-memory recorder. Keeping this a trait is what lets
/// the arbitration path be exercised without a live peer connection.
// As on `SessionObserver`, `double_must_use` fires on the `async_trait`
// expansion's boxed future, not on anything written here.
#[allow(
    clippy::double_must_use,
    reason = "emitted by the `async_trait` expansion, not by this code"
)]
#[async_trait]
pub trait ControlSink: Send + Sync + 'static {
    /// Deliver `msg` to this viewer. Errors are logged and swallowed by the
    /// caller — one unreachable viewer must not stop a broadcast to the rest.
    async fn send(&self, msg: &ControlMsg) -> crate::Result<()>;
}

#[async_trait]
impl ControlSink for crate::PeerSession {
    async fn send(&self, msg: &ControlMsg) -> crate::Result<()> {
        self.send_control(msg).await
    }
}

/// A shared handle to a per-viewer control sink.
pub type SharedSink = Arc<dyn ControlSink>;

/// Something the gate decided but cannot carry out itself, surfaced to the
/// capture/codec layer above this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StationEvent {
    /// A viewer asked for a fresh keyframe (IDR). Any viewer may ask; the
    /// `session` is who asked, so the encoder can decide whether a shared IDR
    /// already in flight suffices.
    KeyframeRequested {
        /// The viewer that asked.
        session: SessionId,
    },
    /// The controller asked to share a different monitor. Only the controller
    /// can trigger this; the gate has already enforced that.
    MonitorSelected {
        /// The monitor id to switch the shared stream to.
        monitor: u32,
    },
}

/// The Station's control fabric: one arbitration gate, many viewers.
///
/// Clone is cheap — every clone shares the same gate, registry and event
/// channel, so a per-session observer can hold one without copying state.
pub struct StationControl<I> {
    inner: Arc<Inner<I>>,
}

impl<I> Clone for StationControl<I> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

struct Inner<I> {
    /// The single authority. Behind an async mutex because the gate is `!Sync`
    /// in general (its injector may not be) and every viewer touches it.
    gate: Mutex<ControlGate<I>>,
    /// Who to reach for each connected viewer.
    sinks: Mutex<HashMap<SessionId, SharedSink>>,
    /// Keyframe / monitor-switch events for the layer above.
    events: mpsc::UnboundedSender<StationEvent>,
}

impl<I: std::fmt::Debug> std::fmt::Debug for StationControl<I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StationControl").finish_non_exhaustive()
    }
}

impl<I: InputInjector + Send + 'static> StationControl<I> {
    /// Build a control fabric around `gate`.
    ///
    /// Returns the fabric and the receiving end of its [`StationEvent`] stream;
    /// the caller (the capture/codec layer) drains the receiver to learn about
    /// keyframe requests and monitor switches.
    #[must_use]
    pub fn new(gate: ControlGate<I>) -> (Self, mpsc::UnboundedReceiver<StationEvent>) {
        let (events, rx) = mpsc::unbounded_channel();
        let control = Self {
            inner: Arc::new(Inner {
                gate: Mutex::new(gate),
                sinks: Mutex::new(HashMap::new()),
                events,
            }),
        };
        (control, rx)
    }

    /// Register a viewer: associate its [`SessionId`] with the sink that
    /// reaches it, and hand back an observer to attach to that viewer's
    /// [`PeerSession`](crate::PeerSession).
    ///
    /// The observer tags every inbound control frame with `session` before it
    /// reaches the gate, which is the whole point: the gate arbitrates by
    /// session id and a leg cannot supply its own.
    pub async fn register(&self, session: SessionId, sink: SharedSink) -> GateObserver<I> {
        // Re-registering a session id replaces its sink; the previous entry (if
        // any) is intentionally discarded.
        let _ = self.inner.sinks.lock().await.insert(session, sink);
        debug!(session, "registered viewer with the control fabric");
        GateObserver {
            session,
            control: self.clone(),
        }
    }

    /// Forget a viewer and release any control or modifiers it held.
    ///
    /// Call this when a peer connection fails or closes. Dropping the sink stops
    /// targeted sends to a dead peer; `disconnect` on the gate frees the token
    /// if that viewer held it and releases keys it was holding, then the
    /// resulting broadcast tells everyone else control is free.
    pub async fn unregister(&self, session: SessionId) {
        let _ = self.inner.sinks.lock().await.remove(&session);
        let effects = self.inner.gate.lock().await.disconnect(session);
        self.dispatch(session, effects).await;
        debug!(session, "unregistered viewer from the control fabric");
    }

    /// Advance the gate's logical clock so pending handoff requests can expire,
    /// dispatching any effects (a timeout notifies the requester and rebroadcasts).
    pub async fn tick(&self, now: u64) {
        let effects = self.inner.gate.lock().await.tick(now);
        // A tick belongs to no single viewer; `from` only steers targeted
        // notifications, and a tick's effects target the requester named inside
        // them, so the `from` here is never consulted for those.
        self.dispatch(SessionId::MAX, effects).await;
    }

    /// Record which monitor is being shared, so input aimed elsewhere is refused.
    pub async fn set_shared_monitor(&self, monitor: Option<u32>) {
        self.inner.gate.lock().await.set_shared_monitor(monitor);
    }

    /// Replace the monitor arrangement after a display change.
    pub async fn set_layout(&self, layout: rr_input::MonitorLayout) {
        self.inner.gate.lock().await.set_layout(layout);
    }

    /// The viewer currently holding control, if any.
    pub async fn controller(&self) -> Option<SessionId> {
        self.inner.gate.lock().await.controller()
    }

    /// Handle one control frame from `session`, applying it to the gate and
    /// carrying out every resulting effect. This is what the per-session
    /// observer calls; it is also directly callable in tests.
    async fn on_control(&self, session: SessionId, msg: ControlMsg) {
        let outcome = {
            let mut gate = self.inner.gate.lock().await;
            match gate.handle(session, &msg) {
                Ok(outcome) => outcome,
                Err(e) => {
                    // Injection itself failed (platform refused the event). The
                    // arbitration state is still consistent; log and move on
                    // rather than tearing down the session.
                    warn!(session, error = %e, "input injection failed");
                    return;
                }
            }
        };

        if let Some(refusal) = outcome.refused {
            debug!(session, ?refusal, "control message refused");
        }

        if outcome.keyframe_requested {
            // A closed receiver means nobody is consuming events; that is not an
            // error here, so ignore the send result.
            let _ = self
                .inner
                .events
                .send(StationEvent::KeyframeRequested { session });
        }
        if let Some(monitor) = outcome.monitor_selected {
            let _ = self
                .inner
                .events
                .send(StationEvent::MonitorSelected { monitor });
        }

        self.dispatch(session, outcome.effects).await;
    }

    /// Carry out arbitration effects: broadcast state to everyone, send targeted
    /// notifications to one viewer.
    ///
    /// `from` is the session that triggered these effects; it is unused for
    /// broadcasts (which go to all) and for notifications (which name their own
    /// target), but kept in the signature so the call sites read clearly.
    async fn dispatch(&self, _from: SessionId, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Broadcast(state) => self.broadcast_state(state).await,
                Effect::NotifyControlRequested { requester } => {
                    // The holder needs the prompt, not the requester. The
                    // authoritative `pending` field in the broadcast already
                    // tells every viewer a request is outstanding, so the
                    // per-holder prompt is driven from that broadcast on the
                    // Client; nothing extra to send here beyond what the
                    // accompanying Broadcast already carries.
                    debug!(requester, "control requested; holder prompted via broadcast");
                }
                Effect::NotifyRequestRejected { requester, reason } => {
                    self.notify_rejected(requester, reason).await;
                }
            }
        }
    }

    /// Send the authoritative control state to every connected viewer.
    async fn broadcast_state(&self, state: ControlState) {
        let msg = ControlMsg::ControlState {
            controller: state.controller,
            pending: state.pending,
        };
        let sinks: Vec<(SessionId, SharedSink)> = self
            .inner
            .sinks
            .lock()
            .await
            .iter()
            .map(|(id, s)| (*id, Arc::clone(s)))
            .collect();
        for (id, sink) in sinks {
            if let Err(e) = sink.send(&msg).await {
                warn!(session = id, error = %e, "failed to broadcast control state");
            }
        }
    }

    /// Tell one viewer its control request was denied or timed out. The wire
    /// has no dedicated reject message; the viewer learns it is no longer
    /// pending from the authoritative state, so we send the current state
    /// directly to that viewer, which clears its "requesting" UI.
    async fn notify_rejected(&self, requester: SessionId, reason: RejectReason) {
        debug!(requester, ?reason, "control request rejected");
        let msg = {
            let gate = self.inner.gate.lock().await;
            ControlMsg::ControlState {
                controller: gate.controller(),
                pending: gate.pending(),
            }
        };
        let sink = self.inner.sinks.lock().await.get(&requester).map(Arc::clone);
        if let Some(sink) = sink {
            if let Err(e) = sink.send(&msg).await {
                warn!(session = requester, error = %e, "failed to notify rejected requester");
            }
        }
    }
}

/// The per-viewer bridge from a [`PeerSession`](crate::PeerSession) to the
/// shared [`StationControl`].
///
/// One of these is attached to each viewer's session as its
/// [`SessionObserver`]. It does exactly one interesting thing: stamp every
/// inbound control frame with *this* viewer's [`SessionId`] before handing it to
/// the gate. A session cannot forge another's id because it never supplies the
/// id at all — the id lives here, set once at registration.
pub struct GateObserver<I> {
    session: SessionId,
    control: StationControl<I>,
}

impl<I> std::fmt::Debug for GateObserver<I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GateObserver")
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl<I: InputInjector + Send + 'static> SessionObserver for GateObserver<I> {
    async fn on_connection_state(&self, state: ConnectionState) {
        // A leg that fails or closes should release whatever it held. The
        // Station app may also call `unregister` explicitly; doing it here too
        // is idempotent and covers the case where the app only watches events.
        if matches!(state, ConnectionState::Failed | ConnectionState::Closed) {
            self.control.unregister(self.session).await;
        }
    }

    async fn on_control(&self, msg: ControlMsg) {
        self.control.on_control(self.session, msg).await;
    }
}
