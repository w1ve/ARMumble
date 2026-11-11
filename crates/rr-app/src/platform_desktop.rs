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

/// Peaks gathered by the idle input monitor.
#[derive(Debug, Default)]
struct MonitorLevels {
    l: std::sync::atomic::AtomicU32,
    r: std::sync::atomic::AtomicU32,
    clips: std::sync::atomic::AtomicU64,
    gain: std::sync::atomic::AtomicU32,
    map: std::sync::atomic::AtomicU8,
}

fn map_code(m: rr_audio::capture::ChannelMap) -> u8 {
    use rr_audio::capture::ChannelMap;
    match m {
        ChannelMap::Stereo => 0,
        ChannelMap::Left => 1,
        ChannelMap::Right => 2,
        ChannelMap::Mix => 3,
    }
}

/// Peak per side and clip count of one device buffer, after channel map and gain.
fn measure(data: &[f32], channels: usize, gain: f32, map: rr_audio::capture::ChannelMap) -> (f32, f32, u64) {
    use rr_audio::capture::ChannelMap;
    let (mut pl, mut pr, mut clips) = (0.0_f32, 0.0_f32, 0_u64);
    for f in data.chunks_exact(channels) {
        let c0 = f[0];
        let c1 = f.get(1).copied().unwrap_or(c0);
        let (l, r) = match map {
            ChannelMap::Stereo => (c0, c1),
            ChannelMap::Left => (c0, c0),
            ChannelMap::Right => (c1, c1),
            ChannelMap::Mix => (0.5 * (c0 + c1), 0.5 * (c0 + c1)),
        };
        let (l, r) = ((l * gain).abs(), (r * gain).abs());
        clips += u64::from(l > 1.0) + u64::from(r > 1.0);
        pl = pl.max(l.min(1.0));
        pr = pr.max(r.min(1.0));
    }
    (pl, pr, clips)
}

/// Meter-only input stream used while not connected, so the input level can
/// be set before going on the air. It applies the same channel map and gain as
/// the transmit path and sends nothing anywhere.
pub struct Monitor {
    stop: Option<Sender<()>>,
    join: Option<JoinHandle<()>>,
    levels: Arc<MonitorLevels>,
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor").finish_non_exhaustive()
    }
}

impl Monitor {
    /// Open `device` and start metering.
    pub fn start(device: Option<&str>, gain: f32, map: rr_audio::capture::ChannelMap) -> Result<Self, String> {
        use std::sync::atomic::Ordering;
        use rr_audio::capture::ChannelMap;

        let channels = usize::from(input_params(device).map_err(|e| e.to_string())?.channels).max(1);
        let levels = Arc::new(MonitorLevels::default());
        levels.gain.store(gain.to_bits(), Ordering::Relaxed);
        levels.map.store(map_code(map), Ordering::Relaxed);
        let sink_levels = Arc::clone(&levels);
        let device = device.map(str::to_owned);
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        let (stop_tx, stop_rx) = channel::<()>();
        let join = thread::Builder::new()
            .name("rr-monitor".into())
            .spawn(move || {
                let sink = move |data: &[f32]| {
                    let gain = f32::from_bits(sink_levels.gain.load(Ordering::Relaxed));
                    let map = match sink_levels.map.load(Ordering::Relaxed) {
                        1 => ChannelMap::Left,
                        2 => ChannelMap::Right,
                        3 => ChannelMap::Mix,
                        _ => ChannelMap::Stereo,
                    };
                    let (pl, pr, clips) = measure(data, channels, gain, map);
                    let _ = sink_levels.l.fetch_max(pl.to_bits(), Ordering::Relaxed);
                    let _ = sink_levels.r.fetch_max(pr.to_bits(), Ordering::Relaxed);
                    if clips > 0 {
                        let _ = sink_levels.clips.fetch_add(clips, Ordering::Relaxed);
                    }
                };
                match rr_audio_cpal::start_input_with(device.as_deref(), sink) {
                    Ok(stream) => {
                        let _ = ready_tx.send(Ok(()));
                        let _ = stop_rx.recv();
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        ready_rx.recv().map_err(|e| e.to_string())??;
        Ok(Self { stop: Some(stop_tx), join: Some(join), levels })
    }

    /// Follow the input level and channel-map settings live.
    pub fn set(&self, gain: f32, map: rr_audio::capture::ChannelMap) {
        use std::sync::atomic::Ordering;
        self.levels.gain.store(gain.to_bits(), Ordering::Relaxed);
        self.levels.map.store(map_code(map), Ordering::Relaxed);
    }

    /// Peaks since the last call, and the total clip count.
    pub fn take(&self) -> (f32, f32, u64) {
        use std::sync::atomic::Ordering;
        (
            f32::from_bits(self.levels.l.swap(0, Ordering::Relaxed)),
            f32::from_bits(self.levels.r.swap(0, Ordering::Relaxed)),
            self.levels.clips.load(Ordering::Relaxed),
        )
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}
