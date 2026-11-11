//! Desktop platform glue: sound cards, serial ports and system-wide hotkeys.

use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use rr_audio_cpal::{input_params, output_params, start_input_with, start_output, Health};
use rr_client::{AudioEndpoints, InputPipe};
use rr_radio::{LocalPtt, LocalPttSettings, StationPtt, StationPttSettings};
use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::engine::{hotkey, HotkeyAction};
use crate::routing::{effective_input, InputSel, Routing};
use crate::settings::HotkeySettings;

pub use crate::devices::Devices;

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

/// The format a device will run at.
pub fn device_params(id: Option<&str>, input: bool) -> Result<Params, String> {
    let p = if input { input_params(id) } else { output_params(id) }.map_err(|e| e.to_string())?;
    Ok(Params { rate: p.rate, channels: p.channels })
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

/// Feed one input device into the shared capture pipe, but only while it is
/// the selected input. The pipe switches format when the device changes.
fn input_sink(
    pipe: &Arc<Mutex<InputPipe>>,
    routing: &Arc<Routing>,
    me: InputSel,
    has_b: bool,
    p: Params,
) -> impl FnMut(&[f32]) + Send + 'static {
    let pipe = Arc::clone(pipe);
    let routing = Arc::clone(routing);
    move |samples: &[f32]| {
        if effective_input(routing.input(), has_b) != me {
            return;
        }
        let Ok(mut pipe) = pipe.lock() else { return };
        if pipe.device_format() != (p.rate, usize::from(p.channels)) {
            pipe.set_device_format(p.rate, usize::from(p.channels));
        }
        pipe.process(samples);
    }
}

/// Open every configured device on a dedicated audio thread. Fails, naming the
/// device, if any of them cannot be opened.
pub fn start_audio(d: Devices, ep: AudioEndpoints, routing: Arc<Routing>) -> Result<AudioHandle, String> {
    let (ready_tx, ready_rx) = channel::<Result<Vec<Arc<Health>>, String>>();
    let (stop_tx, stop_rx) = channel::<()>();
    let join = thread::Builder::new()
        .name("rr-audio".into())
        .spawn(move || {
            let opened = open_all(&d, ep, &routing);
            match opened {
                Ok((streams, health)) => {
                    let _ = ready_tx.send(Ok(health));
                    let _ = stop_rx.recv();
                    drop(streams);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        })
        .map_err(|e| e.to_string())?;
    let health = ready_rx.recv().map_err(|e| e.to_string())??;
    Ok(AudioHandle { stop: Some(stop_tx), join: Some(join), health })
}

/// Streams that must stay alive (and on this thread) while audio runs.
type Streams = (Vec<rr_audio_cpal::RunningInput>, Vec<rr_audio_cpal::RunningOutput>);

fn open_all(d: &Devices, ep: AudioEndpoints, routing: &Arc<Routing>) -> Result<(Streams, Vec<Arc<Health>>), String> {
    let AudioEndpoints { input, output, output_b } = ep;
    let pipe = Arc::new(Mutex::new(input));
    let has_b = d.input_b.is_some();
    let mut inputs = Vec::new();
    let mut health = Vec::new();
    let mut sources = vec![(InputSel::A, d.input_a.clone(), "input A")];
    if let Some(b) = &d.input_b {
        sources.push((InputSel::B, Some(b.clone()), "input B"));
    }
    for (me, id, what) in sources {
        let p = device_params(id.as_deref(), true).map_err(|e| format!("{what} device: {e}"))?;
        let sink = input_sink(&pipe, routing, me, has_b, p);
        let s = start_input_with(id.as_deref(), sink).map_err(|e| format!("{what} device: {e}"))?;
        health.push(Arc::clone(&s.health));
        inputs.push(s);
    }
    let mut outputs = Vec::new();
    let a = start_output(d.output_a.as_deref(), output).map_err(|e| format!("output A device: {e}"))?;
    health.push(Arc::clone(&a.health));
    outputs.push(a);
    if let (Some(id), Some(play)) = (&d.output_b, output_b) {
        let b = start_output(Some(id), play).map_err(|e| format!("output B device: {e}"))?;
        health.push(Arc::clone(&b.health));
        outputs.push(b);
    }
    Ok(((inputs, outputs), health))
}

/// The OTRSP serial listener.
pub type Otrsp = rr_radio::serial::OtrspListener;

/// Start listening for OTRSP commands on `port`.
pub fn open_otrsp<F>(port: &str, on_command: F) -> Result<Otrsp, String>
where
    F: FnMut(rr_radio::OtrspCommand) + Send + 'static,
{
    rr_radio::serial::OtrspListener::open(port, on_command)
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
                match start_input_with(device.as_deref(), sink) {
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
