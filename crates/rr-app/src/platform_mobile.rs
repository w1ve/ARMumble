//! Mobile platform glue: Oboe on Android, a stub elsewhere; the
//! rest of the app builds so the UI can be developed against it.

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

#[cfg(target_os = "android")]
mod audio {
    use std::sync::mpsc::{channel, Sender};
    use std::sync::Arc;
    use std::thread::{self, JoinHandle};

    use rr_audio_oboe::Health;
    use rr_client::AudioEndpoints;

    use super::Params;

    /// Oboe always runs at 48 kHz stereo.
    #[allow(clippy::unnecessary_wraps, reason = "same signature as the desktop version")]
    pub fn params(_input: Option<&str>, _output: Option<&str>) -> Result<(Params, Params), String> {
        let p = Params { rate: rr_audio_oboe::RATE, channels: rr_audio_oboe::CHANNELS };
        Ok((p, p))
    }

    /// Owns the Oboe streams on a dedicated thread.
    #[derive(Debug)]
    pub struct AudioHandle {
        stop: Option<Sender<()>>,
        join: Option<JoinHandle<()>>,
        health: Arc<Health>,
    }

    impl AudioHandle {
        /// Total stream errors and the latest message.
        pub fn health(&self) -> (u64, String) {
            (self.health.error_count(), self.health.last_error())
        }
    }

    impl Drop for AudioHandle {
        fn drop(&mut self) {
            if let Some(tx) = self.stop.take() {
                let _ = tx.send(());
            }
            if let Some(j) = self.join.take() {
                let _ = j.join();
            }
        }
    }

    /// Open both streams; the device pickers do not apply on Android.
    pub fn start_audio(_i: Option<String>, _o: Option<String>, ep: AudioEndpoints) -> Result<AudioHandle, String> {
        let health = Arc::new(Health::default());
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        let (stop_tx, stop_rx) = channel::<()>();
        let h = Arc::clone(&health);
        let join = thread::Builder::new()
            .name("rr-audio".into())
            .spawn(move || {
                let AudioEndpoints { input, output } = ep;
                rr_audio_oboe::run(input, output, &h, &ready_tx, &stop_rx);
            })
            .map_err(|e| e.to_string())?;
        ready_rx.recv().map_err(|e| e.to_string())??;
        Ok(AudioHandle { stop: Some(stop_tx), join: Some(join), health })
    }
}

#[cfg(not(target_os = "android"))]
mod audio {
    use rr_client::AudioEndpoints;

    use super::Params;

    /// Fixed 48 kHz stereo.
    #[allow(clippy::unnecessary_wraps, reason = "same signature as the desktop version")]
    pub fn params(_input: Option<&str>, _output: Option<&str>) -> Result<(Params, Params), String> {
        let p = Params { rate: 48_000, channels: 2 };
        Ok((p, p))
    }

    /// Placeholder owner of the streams.
    #[derive(Debug)]
    pub struct AudioHandle;

    impl AudioHandle {
        /// No streams, no errors.
        pub fn health(&self) -> (u64, String) {
            (0, String::new())
        }
    }

    /// Not available on this platform.
    pub fn start_audio(_i: Option<String>, _o: Option<String>, _ep: AudioEndpoints) -> Result<AudioHandle, String> {
        Err("the audio backend for this platform is not available yet".into())
    }
}

pub use audio::{params, start_audio, AudioHandle};
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

/// No idle input monitor on mobile (it would hold the microphone open).
#[derive(Debug)]
pub struct Monitor;

impl Monitor {
    /// Always unavailable on mobile.
    pub fn start(_device: Option<&str>, _gain: f32, _map: rr_audio::capture::ChannelMap) -> Result<Self, String> {
        Err("not available on this platform".into())
    }

    /// Never called.
    pub fn take(&self) -> (f32, f32, u64) {
        (0.0, 0.0, 0)
    }
}

impl Monitor {
    /// Never called.
    pub fn set(&self, _gain: f32, _map: rr_audio::capture::ChannelMap) {}
}
