//! `RemoteRadio` rig PTT (push-to-talk) keying over RS-232 control lines.
//!
//! Keying a transmitter is a **physical, regulated action**: a stuck PTT can
//! cook a radio's finals, cause prolonged interference, and is an
//! unattended-transmission problem. So this crate is arranged, like
//! [`rr_input`](../rr_input/index.html), so that the dangerous part cannot be
//! reached without passing through the safety layer.
//!
//! # The shape of it
//!
//! | Piece | Job |
//! |---|---|
//! | [`PttSink`] | The trait that actually asserts the key line on the rig. |
//! | [`PttSource`] | The trait that produces key/unkey *intent* (footswitch, command, VOX). |
//! | [`SerialPttSink`] | Real DTR/RTS keying, behind the `serial` feature. |
//! | [`SerialPttSource`] | Real CTS/DSR/CD/RI footswitch sensing, behind `serial`. |
//! | [`RecordingPttSink`] / [`RecordingPttSource`] | Record / script calls, for tests. |
//! | [`Watchdog`] | Wraps a [`PttSink`] and enforces the §14e safety limits. |
//!
//! # Why the watchdog is a wrapper and not a convention
//!
//! "Remember to unkey if the controller vanishes" is a bug waiting to happen,
//! and the consequence here is a transmitter left keyed on the air. So the
//! [`Watchdog`] *owns* the real sink and is the only thing the rest of the
//! application holds. It fails **closed**: any error from the serial layer, any
//! missed keep-alive, any exceeded transmit-time limit, and the key line is
//! driven low (unkeyed). Dropping the watchdog releases the line. There is no
//! path that leaves the rig keyed through inattention.
//!
//! # Line sense, not CAT
//!
//! PTT is a contact closure, universal across radios, so this crate deliberately
//! does **not** speak any rig's CAT command set. The Station drives a writable
//! control line (DTR or RTS) that a small optocoupler/open-collector stage keys
//! the rig's PTT with; the Operator senses a footswitch looped back onto a
//! readable line (CTS, DSR, CD or RI). This is the same mechanism `hamlib` calls
//! `PTT_TYPE_DTR` / `PTT_TYPE_RTS` and what SignaLink/RigBlaster interfaces have
//! always used.
//!
//! # Testability
//!
//! The sink and source are traits and the real serial backend is optional, so
//! the watchdog's timers and fail-closed behaviour — the parts that keep a
//! transmitter from sticking — are tested exhaustively on a headless machine
//! with no serial port present. Time is injected (every state-advancing call
//! takes a `now: Instant`), so the deadman and max-transmit timeouts are tested
//! deterministically without sleeping.

#![forbid(unsafe_code)]

mod recorder;
mod watchdog;

#[cfg(feature = "serial")]
mod serial;

pub use recorder::{PttEvent, RecordingPttSink, RecordingPttSource};
pub use watchdog::{Watchdog, WatchdogConfig};

#[cfg(feature = "serial")]
pub use serial::{
    available_ports, control_line, sense_line, ControlLine, SenseLine, SerialPttConfig,
    SerialPttSink, SerialPttSource,
};

/// Which control line a [`SerialPttSink`] drives, or a [`SerialPttSource`] senses.
///
/// Kept here (not gated behind `serial`) so the application and its settings
/// layer can describe a configuration without the serial backend compiled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PttLine {
    /// Data Terminal Ready (writable output / rarely an input).
    Dtr,
    /// Request To Send (writable output).
    Rts,
    /// Clear To Send (readable input — a common footswitch return line).
    Cts,
    /// Data Set Ready (readable input).
    Dsr,
    /// Carrier Detect (readable input).
    Cd,
    /// Ring Indicator (readable input).
    Ri,
}

/// The key/unkey intent a [`PttSource`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PttIntent {
    /// The operator wants the transmitter keyed.
    Key,
    /// The operator wants the transmitter unkeyed.
    Unkey,
}

impl PttIntent {
    /// `true` when this intent is [`PttIntent::Key`].
    #[must_use]
    pub fn is_key(self) -> bool {
        matches!(self, PttIntent::Key)
    }
}

/// Errors from PTT I/O.
#[derive(Debug, thiserror::Error)]
pub enum PttError {
    /// The serial port could not be opened or configured.
    #[error("serial port open/configure failed: {0}")]
    Open(String),
    /// A control-line read or write failed. The [`Watchdog`] treats this as a
    /// reason to fail closed (unkey).
    #[error("serial control-line I/O failed: {0}")]
    Io(String),
    /// PTT is not available in this build or on this platform.
    #[error("serial PTT is unavailable: {0}")]
    Unavailable(&'static str),
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, PttError>;

/// Something that can assert the rig's PTT key line.
///
/// Deliberately tiny and free of policy: *when* to key is decided by the
/// [`Watchdog`] (safety) and the application (intent). An implementation only
/// drives the physical line, so the dangerous surface is a couple of lines with
/// no branching.
///
/// Implementations **must** power up unkeyed and **must** unkey on `Drop`
/// (closing a serial port usually deasserts DTR/RTS, but an explicit release in
/// `Drop` guarantees it rather than relying on the adapter).
pub trait PttSink {
    /// Drive the key line to `down` (`true` = keyed / transmitting).
    ///
    /// # Errors
    /// Returns [`PttError`] if the control line could not be driven. The caller
    /// (the [`Watchdog`]) treats any error as fail-closed.
    fn set_keyed(&mut self, down: bool) -> Result<()>;
}

/// Something that reports key/unkey intent — a footswitch on a serial input
/// line, a UI command, or an inbound-audio voice gate.
///
/// Polled (there is no portable interrupt-driven control-line notification), so
/// the implementation reports the *current* desired state each time; the caller
/// debounces by acting only on changes. Returning `None` means "no reading this
/// cycle" (e.g. a transient read error), which the caller treats as "no change"
/// — it must never be read as "unkey", because dropping to unkey on a glitchy
/// read would stutter a transmission.
pub trait PttSource {
    /// Poll the current intent, or `None` if it could not be read this cycle.
    fn poll(&mut self) -> Option<PttIntent>;
}

#[cfg(test)]
mod tests;
