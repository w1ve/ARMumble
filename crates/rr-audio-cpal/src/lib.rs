//! Desktop sound-card backend (`cpal`) for the `RemoteRadio` audio engine.
//!
//! The backend does no signal processing. It converts the device's sample
//! format to `f32` and maps channel layouts, nothing else; sample-rate
//! conversion is left to the engine's high-quality converter.
//!
//! Typical use:
//!
//! 1. [`list_devices`] to populate a picker.
//! 2. [`input_params`] / [`output_params`] to learn which rate and channel
//!    count a device will run at, and build the client with them.
//! 3. [`start_input`] / [`start_output`] with the client's audio endpoints.
//!
//! Streams are not guaranteed to be `Send`; keep them on the thread that
//! created them (a dedicated audio thread works well).
#![cfg(not(target_os = "android"))]

mod devices;
mod stream;

pub use devices::{input_params, list_devices, output_params, DeviceInfo, StreamParams};
pub use stream::{start_input, start_input_with, start_output, BackendError, Health, RunningInput, RunningOutput};
