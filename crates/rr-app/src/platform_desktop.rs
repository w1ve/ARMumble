//! Desktop platform glue: sound cards, serial ports and system-wide hotkeys.

use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use rr_audio_cpal::{input_params, output_params, start_input, start_output, Health};
use rr_client::AudioEndpoints;
use rr_radio::{LocalPtt, LocalPttSettings, StationPtt, StationPttSettings};
use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::engine::{hotkey, HotkeyAction};
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

/// Sound devices on this machine.
pub fn devices() -> Vec<DeviceDto> {
    rr_audio_cpal::list_devices()
        .into_iter()
        .map(|d| DeviceDto { id: d.id, name: d.name, input: d.input, output: d.output, is_default: d.is_default })
        .collect()
}

/// Serial ports on this machine.
pub fn serial_ports() -> Vec<PortDto> {
    rr_radio::serial::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|(name, label)| PortDto { name, label })
        .collect()
}

/// The formats the chosen devices will use.
pub fn params(input: Option<&str>, output: Option<&str>) -> Result<(Params, Params), String> {
    let i = input_params(input).map_err(|e| e.to_string())?;
    let o = output_params(output).map_err(|e| e.to_string())?;
    Ok((Params { rate: i.rate, channels: i.channels }, Params { rate: o.rate, channels: o.channels }))
}

/// Owns the sound-card streams on a dedicated thread (streams must stay on the
/// thread that created them).
#[derive(Debug)]
pub struct AudioHandle {
    stop: Option<Sender<()>>,
    join: Option<JoinHandle<()>>,
    health: Vec<Arc<Health>>,
}

impl AudioHandle {
    /// Total stream errors and the latest message.
    pub fn health(&self) -> (u64, String) {
        let errors = self.health.iter().map(|h| h.error_count()).sum();
        let last = self.health.iter().map(|h| h.last_error()).find(|s| !s.is_empty()).unwrap_or_default();
        (errors, last)
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

/// Open both streams; fails if either device cannot be opened.
pub fn start_audio(input: Option<String>, output: Option<String>, ep: AudioEndpoints) -> Result<AudioHandle, String> {
    let (ready_tx, ready_rx) = channel::<Result<Vec<Arc<Health>>, String>>();
    let (stop_tx, stop_rx) = channel::<()>();
    let join = thread::Builder::new()
        .name("rr-audio".into())
        .spawn(move || {
            let AudioEndpoints { input: pipe, output: play } = ep;
            let inp = start_input(input.as_deref(), pipe);
            let out = start_output(output.as_deref(), play);
            match (inp, out) {
                (Ok(i), Ok(o)) => {
                    let _ = ready_tx.send(Ok(vec![Arc::clone(&i.health), Arc::clone(&o.health)]));
                    let _ = stop_rx.recv();
                    drop((i, o));
                }
                (Err(e), _) => {
                    let _ = ready_tx.send(Err(format!("input device: {e}")));
                }
                (_, Err(e)) => {
                    let _ = ready_tx.send(Err(format!("output device: {e}")));
                }
            }
        })
        .map_err(|e| e.to_string())?;
    let health = ready_rx.recv().map_err(|e| e.to_string())??;
    Ok(AudioHandle { stop: Some(stop_tx), join: Some(join), health })
}

/// Open the rig key line.
pub fn open_station(s: &StationPttSettings) -> Result<StationPtt, String> {
    rr_radio::serial::open_station(s)
}

/// Open the footswitch input.
pub fn open_local<F>(s: &LocalPttSettings, on_message: F) -> Result<LocalPtt, String>
where
    F: Fn(rr_radio::PttMessage) + Send + 'static,
{
    rr_radio::serial::open_local(s, on_message)
}

/// Register the system-wide hotkeys, replacing any previous ones.
pub fn apply_hotkeys(app: &AppHandle, hk: &HotkeySettings) {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    for (accel, action) in [
        (&hk.ptt, HotkeyAction::Ptt),
        (&hk.whisper, HotkeyAction::Whisper),
        (&hk.listen_in, HotkeyAction::ListenIn),
    ] {
        register(app, accel, action);
    }
}

fn register(app: &AppHandle, accel: &str, action: HotkeyAction) {
    if accel.trim().is_empty() {
        return;
    }
    let shortcut: Shortcut = match accel.parse() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("bad hotkey {accel:?}: {e}");
            return;
        }
    };
    let handle = app.clone();
    let result = app.global_shortcut().on_shortcut(shortcut, move |_app, _sc, ev| {
        hotkey(&handle, action, ev.state() == ShortcutState::Pressed);
    });
    if let Err(e) = result {
        tracing::warn!("cannot register hotkey {accel:?}: {e}");
    }
}

/// Add the plugin to the builder.
pub fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_plugin_global_shortcut::Builder::new().build()
}
