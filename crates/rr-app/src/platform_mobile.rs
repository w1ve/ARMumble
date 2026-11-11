//! Mobile platform glue. The Android sound backend is not wired in yet; the
//! rest of the app builds so the UI can be developed against it.

use rr_client::AudioEndpoints;
use rr_radio::{LocalPtt, LocalPttSettings, StationPtt, StationPttSettings};
use serde::Serialize;
use tauri::AppHandle;

use crate::settings::HotkeySettings;

/// Rate and channel count a device will run at.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// Frames per second.
    pub rate: u32,
    /// Interleaved channels.
    pub channels: u16,
}

/// A sound device for the picker.
#[derive(Debug, Serialize)]
pub struct DeviceDto {
    /// Stable id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Can capture.
    pub input: bool,
    /// Can play.
    pub output: bool,
    /// System default.
    pub is_default: bool,
}

/// A serial port for the picker.
#[derive(Debug, Serialize)]
pub struct PortDto {
    /// OS name.
    pub name: String,
    /// Description.
    pub label: String,
}

/// No device list on mobile.
pub fn devices() -> Vec<DeviceDto> {
    Vec::new()
}

/// No serial ports on mobile.
pub fn serial_ports() -> Vec<PortDto> {
    Vec::new()
}

/// Fixed 48 kHz stereo until the mobile backend exists.
#[allow(clippy::unnecessary_wraps, reason = "same signature as the desktop version")]
pub fn params(_input: Option<&str>, _output: Option<&str>) -> Result<(Params, Params), String> {
    let p = Params { rate: 48_000, channels: 2 };
    Ok((p, p))
}

/// Placeholder owner of the mobile streams.
#[derive(Debug)]
pub struct AudioHandle;

impl AudioHandle {
    /// No streams, no errors.
    pub fn health(&self) -> (u64, String) {
        (0, String::new())
    }
}

/// Not available yet.
pub fn start_audio(_i: Option<String>, _o: Option<String>, _ep: AudioEndpoints) -> Result<AudioHandle, String> {
    Err("the audio backend for this platform is not available yet".into())
}

/// Not available on mobile.
pub fn open_station(_s: &StationPttSettings) -> Result<StationPtt, String> {
    Err("serial PTT is not available on this platform".into())
}

/// Not available on mobile.
pub fn open_local<F>(_s: &LocalPttSettings, _f: F) -> Result<LocalPtt, String>
where
    F: Fn(rr_radio::PttMessage) + Send + 'static,
{
    Err("serial PTT is not available on this platform".into())
}

/// Hotkeys do not exist on mobile.
pub fn apply_hotkeys(_app: &AppHandle, _hk: &HotkeySettings) {}
