//! `RemoteRadio` input control.
//!
//! The Station turns [`rr_proto::ControlMsg`]s from a Client into real mouse and
//! keyboard events. That is the most dangerous thing this project does — it is
//! remote code execution by design — so the crate is arranged so the dangerous
//! part cannot be reached by accident.
//!
//! # The shape of it
//!
//! | Piece | Job |
//! |---|---|
//! | [`InputInjector`] | The trait that actually moves the pointer and presses keys. |
//! | [`EnigoInjector`] | The real implementation, behind the `inject` feature. |
//! | [`RecordingInjector`] | Records calls instead of performing them, for tests. |
//! | [`ControlGate`] | Owns an injector and an `Arbiter`; the **only** way in. |
//! | [`MonitorLayout`] | Monitor-relative to virtual-desktop coordinate mapping. |
//! | [`KeyCode`] | The platform-independent key table the wire format refers to. |
//!
//! # Why the gate is a type and not a rule
//!
//! [`ControlGate`] keeps its injector private and exposes no method that injects
//! without a `SessionId`. Every input event is checked against
//! `Arbiter::may_control` at the moment it arrives, using the same arbiter that
//! decides handoff — so authorisation cannot drift out of step with the handoff
//! state, and a caller cannot forget the check because there is no unchecked path
//! to forget.
//!
//! The Client also hides its input UI when it does not hold control, so in normal
//! operation nothing uncontrolled is ever sent. The gate assumes that layer is
//! missing or compromised.
//!
//! # Deliberate refusals
//!
//! Three things are refused rather than guessed, because each guess would mean
//! acting on a machine the person cannot see:
//!
//! - **Input from a non-controller.** Counted, dropped, not an error.
//! - **Input addressed to a monitor that is not being shared.** The controller
//!   cannot see it, so acting on it is acting blind.
//! - **An unmapped key code.** Typing an approximation is worse than typing
//!   nothing.
//!
//! Out-of-range *coordinates* are the exception: those are clamped to the shared
//! monitor, because a pointer at the edge is a normal consequence of a differently
//! sized viewer window, and dropping the event would make the pointer stick.
//!
//! # Testability
//!
//! Injection is a trait and the real backend is optional, so the arbitration and
//! coordinate logic — the parts that decide whether a keystroke happens at all —
//! are tested exhaustively on a headless machine with no input system present.

#![forbid(unsafe_code)]

mod gate;
mod keymap;
mod layout;
mod recorder;

#[cfg(feature = "inject")]
mod enigo_backend;

pub use gate::{ControlGate, Outcome, Refusal, RefusalCounts};
pub use keymap::KeyCode;
pub use layout::{MonitorLayout, MonitorPlacement};
pub use recorder::{RecordedAction, RecordingInjector};

#[cfg(feature = "inject")]
pub use enigo_backend::EnigoInjector;

use rr_proto::MouseButton;

/// Errors from injecting input.
#[derive(Debug, thiserror::Error)]
pub enum InputError {
    /// The platform input system could not be reached or refused the event.
    #[error("input injection failed: {0}")]
    Injection(String),
    /// Injection is not available in this build or on this platform.
    #[error("input injection is unavailable: {0}")]
    Unavailable(&'static str),
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, InputError>;

/// Something that can perform input events on the local machine.
///
/// Deliberately small and free of policy: *whether* an event should happen is
/// [`ControlGate`]'s business, and keeping that decision out of here is what makes
/// the dangerous part a handful of lines with no branching.
///
/// Coordinates are absolute virtual-desktop pixels. Mapping from the
/// monitor-relative wire format happens in [`MonitorLayout`] before this is called,
/// so an implementation never has to reason about display arrangement.
pub trait InputInjector {
    /// Move the pointer to an absolute virtual-desktop position.
    ///
    /// # Errors
    /// Returns [`InputError`] if the platform refuses the event.
    fn mouse_move_abs(&mut self, x: i32, y: i32) -> Result<()>;

    /// Press or release a mouse button.
    ///
    /// # Errors
    /// Returns [`InputError`] if the platform refuses the event.
    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()>;

    /// Scroll. Positive `dy` scrolls up, positive `dx` scrolls right.
    ///
    /// # Errors
    /// Returns [`InputError`] if the platform refuses the event.
    fn wheel(&mut self, dx: i32, dy: i32) -> Result<()>;

    /// Press or release a key.
    ///
    /// # Errors
    /// Returns [`InputError`] if the platform refuses the event.
    fn key(&mut self, key: KeyCode, down: bool) -> Result<()>;

    /// Type text directly, bypassing key codes.
    ///
    /// For IME and paste-like input, where the controller's keyboard layout is
    /// irrelevant and only the resulting characters matter.
    ///
    /// # Errors
    /// Returns [`InputError`] if the platform refuses the event.
    fn text(&mut self, text: &str) -> Result<()>;

    /// Release everything currently held down.
    ///
    /// Called at every change of control. A controller who hands over or
    /// disconnects while holding Shift would otherwise leave it down for the next
    /// person, whose symptom — someone else's typing arriving in capitals — gives
    /// no hint of the cause.
    ///
    /// # Errors
    /// Returns [`InputError`] if the platform refuses the events.
    fn release_all(&mut self) -> Result<()>;
}

#[cfg(test)]
mod tests;
