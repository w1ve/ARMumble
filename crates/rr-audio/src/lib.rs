//! `RemoteRadio` audio engine.
//!
//! Processing-free, high-fidelity stereo audio for amateur-radio links over
//! Mumble. The engine never applies echo cancellation, automatic gain, noise
//! suppression, gating or limiting. The only things it does to a signal are the
//! ones that are unavoidable: sample-rate conversion with a proper filter,
//! user-chosen gain, and Opus coding.
//!
//! * [`capture`]: device samples to Opus packets, with VOX / PTT gating.
//! * [`playout`]: Opus packets to device samples, with a drift-compensating
//!   jitter buffer per talker.
//! * [`bandwidth`]: fit the requested quality into the server's limit.

pub mod asrc;
pub mod bandwidth;
pub mod capture;
pub mod combine;
pub mod codec;
pub mod error;
pub mod jitter;
pub mod playout;
pub mod stream;
mod util;

pub use error::{AudioError, Result};
pub use stream::SpeakerStats;

// Only the integration tests use this.
#[cfg(test)]
use rustfft as _;
