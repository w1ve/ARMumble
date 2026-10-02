//! Arbitration-gated input injection.
//!
//! This is the Station's only route from a received message to a real keystroke,
//! and the gate is structural rather than a convention:
//!
//! - The injector is a **private field**. There is no public method that injects
//!   without a [`SessionId`], so a caller cannot forget to check.
//! - [`ControlGate::handle`] requires the sender's `SessionId` and consults
//!   [`Arbiter::may_control`] on **every** event, not once per session. Control can
//!   change hands between two messages from the same peer.
//! - The same arbiter instance decides handoff *and* authorises injection, so the
//!   two can never disagree. A separate "is this peer allowed" cache is exactly the
//!   sort of thing that goes stale and lets a released controller keep typing.
//!
//! `rr-session` already models the cooperative handoff and is tested on its own.
//! This module is the integration: wire messages in, effects and injections out.
//!
//! # Defence in depth
//!
//! The Client also hides its input UI when it does not hold control, so in normal
//! operation nothing uncontrolled is ever sent. This gate assumes that layer is
//! absent or compromised. A peer that sends input it was not granted gets it
//! dropped and counted, because the alternative is a viewer silently driving
//! someone else's desktop.

use rr_proto::{ControlMsg, InputEvent};
use rr_session::{Arbiter, Effect, Input as ArbiterInput, SessionId};
use tracing::{debug, warn};

use crate::{InputInjector, MonitorLayout, Result};

/// Why an input event was not injected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The sender does not hold control.
    NotController,
    /// The event addressed a monitor the Station does not have.
    UnknownMonitor(u32),
    /// The key code was outside the known table, so injecting would be a guess.
    UnknownKey(u32),
}

/// What the caller must do after a message was handled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Arbitration side effects to carry out — broadcasts and notifications.
    pub effects: Vec<Effect>,
    /// Whether an input event was actually injected.
    pub injected: bool,
    /// Why an input event was not injected, if it was not.
    pub refused: Option<Refusal>,
    /// The controller asked for a fresh keyframe.
    pub keyframe_requested: bool,
    /// The controller asked to share a different monitor.
    pub monitor_selected: Option<u32>,
}

/// Counts of refused events, for diagnostics and for noticing abuse.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RefusalCounts {
    /// Input arrived from a peer that does not hold control.
    pub not_controller: u64,
    /// Input addressed an unknown monitor.
    pub unknown_monitor: u64,
    /// Input carried an unmapped key code.
    pub unknown_key: u64,
}

/// The Station's control surface: arbitration plus gated injection.
pub struct ControlGate<I> {
    /// Cooperative handoff state machine. Also the authorisation oracle.
    arbiter: Arbiter,
    /// Where injected events go. Private on purpose — see the module docs.
    injector: I,
    /// Monitor arrangement for coordinate mapping.
    layout: MonitorLayout,
    /// Which monitor is currently shared, used to reject input aimed elsewhere.
    shared_monitor: Option<u32>,
    /// Refusal tally.
    refusals: RefusalCounts,
}

impl<I: std::fmt::Debug> std::fmt::Debug for ControlGate<I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlGate")
            .field("controller", &self.arbiter.controller())
            .field("pending", &self.arbiter.pending())
            .field("shared_monitor", &self.shared_monitor)
            .field("refusals", &self.refusals)
            .finish_non_exhaustive()
    }
}

impl<I: InputInjector> ControlGate<I> {
    /// Build a gate.
    ///
    /// `handoff_timeout` is in the same logical time unit passed to
    /// [`ControlGate::tick`], matching `rr-session`'s convention.
    pub fn new(injector: I, layout: MonitorLayout, handoff_timeout: u64) -> Self {
        Self {
            arbiter: Arbiter::new(handoff_timeout),
            injector,
            layout,
            shared_monitor: None,
            refusals: RefusalCounts::default(),
        }
    }

    /// The session holding control, if any.
    pub fn controller(&self) -> Option<SessionId> {
        self.arbiter.controller()
    }

    /// The session awaiting approval, if any.
    pub fn pending(&self) -> Option<SessionId> {
        self.arbiter.pending()
    }

    /// Refused-event tallies.
    pub const fn refusals(&self) -> RefusalCounts {
        self.refusals
    }

    /// Replace the monitor arrangement, after a display change.
    pub fn set_layout(&mut self, layout: MonitorLayout) {
        self.layout = layout;
    }

    /// Record which monitor is being shared.
    ///
    /// Input addressed to any other monitor is refused: the controller can only
    /// see what is being streamed, so acting on anything else is acting blind.
    pub fn set_shared_monitor(&mut self, monitor: Option<u32>) {
        self.shared_monitor = monitor;
    }

    /// Advance logical time, so pending handoff requests can expire.
    pub fn tick(&mut self, now: u64) -> Vec<Effect> {
        self.arbiter.apply(ArbiterInput::Tick { now })
    }

    /// Note that a session went away.
    ///
    /// Releases any modifiers it was holding before giving up its control, so a
    /// controller that vanishes mid-chord does not leave Ctrl stuck down on the
    /// Station.
    pub fn disconnect(&mut self, who: SessionId) -> Vec<Effect> {
        if self.arbiter.controller() == Some(who) {
            if let Err(e) = self.injector.release_all() {
                warn!(error = %e, session = who, "could not release held keys on disconnect");
            }
        }
        self.arbiter.apply(ArbiterInput::Disconnect { who })
    }

    /// Handle one `rr-control` message from `from`.
    ///
    /// # Errors
    /// Returns an error only if injection itself failed. A refused event is not an
    /// error — it is reported in [`Outcome::refused`], because a peer sending input
    /// it is not entitled to is an expected condition, not a fault.
    pub fn handle(&mut self, from: SessionId, msg: &ControlMsg) -> Result<Outcome> {
        match msg {
            ControlMsg::Input(event) => self.handle_input(from, event),

            ControlMsg::RequestControl => Ok(Outcome {
                effects: self.arbiter.apply(ArbiterInput::Request { who: from }),
                ..Outcome::default()
            }),

            // Approve and deny are the *holder's* decisions. The arbiter checks
            // that `from` is the holder; a non-holder's approval is a no-op there,
            // which is the behaviour we want rather than an error path here.
            ControlMsg::ApproveHandoff { to } => Ok(Outcome {
                effects: self.approve(from, *to),
                ..Outcome::default()
            }),
            ControlMsg::DenyHandoff { to } => Ok(Outcome {
                effects: self.deny(from, *to),
                ..Outcome::default()
            }),

            ControlMsg::ReleaseControl => Ok(Outcome {
                effects: self.release(from),
                ..Outcome::default()
            }),

            ControlMsg::SelectMonitor { monitor } => {
                // Changing the shared monitor is a controller action: it changes
                // what every other viewer sees.
                if !self.arbiter.may_control(from) {
                    self.refusals.not_controller += 1;
                    return Ok(Outcome {
                        refused: Some(Refusal::NotController),
                        ..Outcome::default()
                    });
                }
                Ok(Outcome {
                    monitor_selected: Some(*monitor),
                    ..Outcome::default()
                })
            }

            // Deliberately ungated: any viewer may ask for a keyframe. It costs one
            // IDR and is exactly what a viewer needs on joining or after loss, so
            // requiring control would make a passive viewer unable to see anything.
            ControlMsg::RequestKeyframe => Ok(Outcome {
                keyframe_requested: true,
                ..Outcome::default()
            }),

            // A Station receiving its own broadcast shape means the peer is
            // confused or hostile. Ignore it rather than act on it.
            ControlMsg::ControlState { .. } => {
                debug!(session = from, "ignoring ControlState from a peer");
                Ok(Outcome::default())
            }
        }
    }

    /// The gate itself.
    fn handle_input(&mut self, from: SessionId, event: &InputEvent) -> Result<Outcome> {
        // Checked per event, not per session: control can change hands between two
        // messages from the same peer.
        if !self.arbiter.may_control(from) {
            self.refusals.not_controller += 1;
            debug!(
                session = from,
                controller = ?self.arbiter.controller(),
                "refused input from a non-controller"
            );
            return Ok(Outcome {
                refused: Some(Refusal::NotController),
                ..Outcome::default()
            });
        }

        match self.inject(event)? {
            None => Ok(Outcome {
                injected: true,
                ..Outcome::default()
            }),
            Some(refusal) => {
                match refusal {
                    Refusal::UnknownMonitor(_) => self.refusals.unknown_monitor += 1,
                    Refusal::UnknownKey(_) => self.refusals.unknown_key += 1,
                    Refusal::NotController => self.refusals.not_controller += 1,
                }
                Ok(Outcome {
                    refused: Some(refusal),
                    ..Outcome::default()
                })
            }
        }
    }

    /// Inject one event, returning `Some(reason)` if it had to be refused.
    ///
    /// Only reached for a peer that holds control.
    ///
    /// Every refusal leaves by an early `return`, so reaching the end means the
    /// event was injected. That is why the success value sits after the `match`
    /// rather than in each arm: there is one way to succeed and it is written once.
    fn inject(&mut self, event: &InputEvent) -> Result<Option<Refusal>> {
        match event {
            InputEvent::MouseMove { monitor, x, y } => {
                // Refuse input aimed at a monitor other than the one being
                // streamed: the controller cannot see it.
                if let Some(shared) = self.shared_monitor {
                    if *monitor != shared {
                        return Ok(Some(Refusal::UnknownMonitor(*monitor)));
                    }
                }
                let Some((abs_x, abs_y)) = self.layout.to_absolute(*monitor, *x, *y) else {
                    return Ok(Some(Refusal::UnknownMonitor(*monitor)));
                };
                self.injector.mouse_move_abs(abs_x, abs_y)?;
            }
            InputEvent::MouseButton { button, down } => {
                self.injector.mouse_button(*button, *down)?;
            }
            InputEvent::Wheel { dx, dy } => {
                self.injector.wheel(*dx, *dy)?;
            }
            InputEvent::Key { code, down } => {
                let Some(key) = crate::KeyCode::from_u32(*code) else {
                    // Refuse rather than guess: a wrong guess types something the
                    // controller never pressed.
                    return Ok(Some(Refusal::UnknownKey(*code)));
                };
                self.injector.key(key, *down)?;
            }
            InputEvent::Text { text } => {
                self.injector.text(text)?;
            }
        }
        Ok(None)
    }

    /// Approve a handoff, releasing the outgoing holder's modifiers first.
    fn approve(&mut self, from: SessionId, to: SessionId) -> Vec<Effect> {
        let was_holder = self.arbiter.controller() == Some(from);
        let effects = self.arbiter.apply(ArbiterInput::Approve { to });
        // Only if control actually moved. Releasing modifiers on a no-op approval
        // would interrupt the current controller for nothing.
        if was_holder && self.arbiter.controller() == Some(to) {
            self.release_modifiers_for_handoff();
        }
        effects
    }

    /// Deny a handoff.
    fn deny(&mut self, _from: SessionId, to: SessionId) -> Vec<Effect> {
        self.arbiter.apply(ArbiterInput::Deny { to })
    }

    /// Release control, clearing any held keys first.
    fn release(&mut self, from: SessionId) -> Vec<Effect> {
        if self.arbiter.controller() == Some(from) {
            self.release_modifiers_for_handoff();
        }
        self.arbiter.apply(ArbiterInput::Release)
    }

    /// Drop every held key and button at a change of control.
    ///
    /// Without this, a controller who releases while holding Shift leaves Shift
    /// down for whoever gets control next, and the symptom — everything typed in
    /// capitals by a different person — is baffling to debug.
    fn release_modifiers_for_handoff(&mut self) {
        if let Err(e) = self.injector.release_all() {
            warn!(error = %e, "could not release held keys at handoff");
        }
    }
}
