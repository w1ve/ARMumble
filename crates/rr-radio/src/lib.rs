//! Rig control for `RemoteRadio`.
//!
//! * **Local PTT**: a footswitch on an RS-232 input line. Its state is sent to
//!   the remote station as a [`message::PttMessage`], repeated while held.
//! * **Station PTT**: an RS-232 output line that keys the rig when a remote
//!   operator holds PTT or, with VOX on, whenever inbound audio is present.
//!   A fail-safe watchdog sits in front of the line.
//! * **Hold hotkeys**: Whisper and Listen In as press/release state machines.
//! * **OTRSP**: decode a contest logger's SO2R commands to switch audio.
//!
//! Nothing here touches audio samples; it only decides when to key.

pub mod hold;
pub mod keyer;
pub mod message;
pub mod otrsp;
pub mod runner;
#[cfg(feature = "serial")]
pub mod serial;
pub mod settings;

pub use hold::{HoldAction, ListenIn, WhisperHold};
pub use keyer::StationKeyer;
pub use message::{PttMessage, SeqFilter};
pub use otrsp::{Listen, OtrspCommand, OtrspParser, Radio};
pub use runner::{LocalPtt, StationPtt};
pub use settings::{KeyLine, LocalPttSettings, PttSettings, SenseLine, StationPttSettings};
