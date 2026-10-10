//! Audio device hotplug (PnP) watcher.
//!
//! cpal has no portable device-change callback, so we poll the input and
//! output device lists on a background thread and emit a Tauri event when the
//! set changes (device plugged in or removed). The frontend re-enumerates on
//! that event so the dropdowns stay in sync with the OS.
//!
//! The poll also detects when the *currently selected* capture/playback device
//! has disappeared and asks the audio state to fall back to "(No device)" and
//! reset the affected chain.

#[cfg(not(target_os = "android"))]
use std::time::Duration;

/// Tauri event emitted when the available device set changes.
pub const DEVICES_CHANGED_EVENT: &str = "audio-devices-changed";

/// Poll interval for the device watcher.
#[cfg(not(target_os = "android"))]
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Snapshot of the current input + output device name sets, used to detect
/// changes between polls.
#[cfg(not(target_os = "android"))]
#[derive(Default, PartialEq, Eq)]
struct DeviceSnapshot {
    inputs: Vec<String>,
    outputs: Vec<String>,
}

#[cfg(not(target_os = "android"))]
fn snapshot() -> DeviceSnapshot {
    let mut inputs: Vec<String> = crate::commands::audio::enumerate_input_device_names();
    let mut outputs: Vec<String> = crate::commands::audio::enumerate_output_device_names();
    inputs.sort();
    outputs.sort();
    DeviceSnapshot { inputs, outputs }
}

/// Start the background device watcher. Safe to call once at startup.
#[cfg(not(target_os = "android"))]
pub fn start(app: tauri::AppHandle) {
    use tauri::{Emitter, Manager};

    let spawn_result = std::thread::Builder::new()
        .name("audio-device-watcher".into())
        .spawn(move || {
            let mut last = snapshot();
            loop {
                std::thread::sleep(POLL_INTERVAL);
                let current = snapshot();
                if current != last {
                    // Let the audio state react to the selected device
                    // vanishing (fall back to No device + reset the chain).
                    let state = app.state::<crate::state::AppState>();
                    state.handle_device_set_changed(&current.inputs, &current.outputs);

                    // Notify the frontend to re-enumerate.
                    let _ = app.emit(DEVICES_CHANGED_EVENT, ());
                    last = current;
                }
            }
        });
    if let Err(e) = spawn_result {
        tracing::warn!("failed to start audio device watcher: {e}");
    }
}

/// On Android there is no device hotplug watcher.
#[cfg(target_os = "android")]
pub fn start(_app: tauri::AppHandle) {}
