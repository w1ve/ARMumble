//! Desktop platform glue: sound cards, serial ports and system-wide hotkeys.

use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use rr_audio::playout::Playout;
use rr_audio_cpal::{input_params, output_params, start_input_with, start_output_with, Health};
use rr_client::{AudioEndpoints, InputPipe};
use rr_radio::{LocalPtt, LocalPttSettings};
use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::engine::{hotkey, HotkeyAction};

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
/// The client's audio endpoints, shared so the streams can be reopened on
/// other devices without reconnecting. Only the running streams use them, and
/// only one set of streams runs at a time, so the locks are never contended.
#[derive(Debug, Clone)]
struct Shared {
    pipe: Arc<Mutex<InputPipe>>,
    out_a: Arc<Mutex<Playout>>,
    out_b: Option<Arc<Mutex<Playout>>>,
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The streams' thread (streams must stay on the thread that created them).
#[derive(Debug)]
struct Running {
    stop: Option<Sender<()>>,
    join: Option<JoinHandle<()>>,
    health: Vec<Arc<Health>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Owns the sound-card streams.
#[derive(Debug)]
pub struct AudioHandle {
    shared: Shared,
    devices: Devices,
    running: Option<Running>,
}

impl AudioHandle {
    /// Total stream errors and the latest message.
    pub fn health(&self) -> (u64, String) {
        let Some(r) = &self.running else {
            return (0, String::new());
        };
        let errors = r.health.iter().map(|h| h.error_count()).sum();
        let last = r.health.iter().map(|h| h.last_error()).find(|s| !s.is_empty()).unwrap_or_default();
        (errors, last)
    }

    /// Move the streams to other devices, keeping the connection. Returns
    /// `Ok(false)` if this needs a reconnect instead (a second output device
    /// added or removed). On failure the old devices are reopened if possible.
    pub fn switch(&mut self, d: Devices) -> Result<bool, String> {
        if d == self.devices {
            return Ok(true);
        }
        if d.output_b.is_some() != self.shared.out_b.is_some() {
            return Ok(false);
        }
        self.running = None;
        match configure(&self.shared, &d).and_then(|()| spawn_streams(&d, &self.shared)) {
            Ok(r) => {
                self.running = Some(r);
                self.devices = d;
                Ok(true)
            }
            Err(e) => {
                let old = self.devices.clone();
                if configure(&self.shared, &old).is_ok() {
                    self.running = spawn_streams(&old, &self.shared).ok();
                }
                Err(e)
            }
        }
    }
}

/// Set the pipes to the formats the devices will run at.
fn configure(s: &Shared, d: &Devices) -> Result<(), String> {
    let pa = device_params(d.input_a.as_deref(), true).map_err(|e| format!("input device: {e}"))?;
    // Two cards are combined into one stereo signal at card A's rate.
    let channels = if d.input_b.is_some() { 2 } else { usize::from(pa.channels) };
    locked(&s.pipe).set_device_format(pa.rate, channels);
    let oa = device_params(d.output_a.as_deref(), false).map_err(|e| format!("output device: {e}"))?;
    locked(&s.out_a).set_out_rate(oa.rate);
    if let (Some(id), Some(b)) = (&d.output_b, &s.out_b) {
        let ob = device_params(Some(id), false).map_err(|e| format!("output B device: {e}"))?;
        locked(b).set_out_rate(ob.rate);
    }
    Ok(())
}

/// Open every configured device on a dedicated audio thread. Fails, naming the
/// device, if any of them cannot be opened.
pub fn start_audio(d: Devices, ep: AudioEndpoints) -> Result<AudioHandle, String> {
    let AudioEndpoints { input, output, output_b } = ep;
    let shared = Shared {
        pipe: Arc::new(Mutex::new(input)),
        out_a: Arc::new(Mutex::new(output)),
        out_b: output_b.map(|p| Arc::new(Mutex::new(p))),
    };
    let running = spawn_streams(&d, &shared)?;
    Ok(AudioHandle { shared, devices: d, running: Some(running) })
}

fn spawn_streams(d: &Devices, s: &Shared) -> Result<Running, String> {
    let (ready_tx, ready_rx) = channel::<Result<Vec<Arc<Health>>, String>>();
    let (stop_tx, stop_rx) = channel::<()>();
    let (d, s) = (d.clone(), s.clone());
    let join = thread::Builder::new()
        .name("rr-audio".into())
        .spawn(move || match open_all(&d, &s) {
            Ok((streams, health)) => {
                let _ = ready_tx.send(Ok(health));
                let _ = stop_rx.recv();
                drop(streams);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        })
        .map_err(|e| e.to_string())?;
    let health = ready_rx.recv().map_err(|e| e.to_string())??;
    Ok(Running { stop: Some(stop_tx), join: Some(join), health })
}

/// Streams that must stay alive (and on this thread) while audio runs.
type Streams = (Vec<rr_audio_cpal::RunningInput>, Vec<rr_audio_cpal::RunningOutput>);

fn open_all(d: &Devices, s: &Shared) -> Result<(Streams, Vec<Arc<Health>>), String> {
    let mut inputs = Vec::new();
    let mut health = Vec::new();
    let pa = device_params(d.input_a.as_deref(), true).map_err(|e| format!("input A device: {e}"))?;
    if let Some(b_id) = &d.input_b {
        // One card per radio: radio 1 (A) on the left, radio 2 (B) on the
        // right, with B drift-locked to A's clock.
        let pb = device_params(Some(b_id), true).map_err(|e| format!("input B device: {e}"))?;
        let (mut comb, feed) =
            rr_audio::combine::stereo_combiner(pa.rate, usize::from(pa.channels), pb.rate, usize::from(pb.channels));
        let b = start_input_with(Some(b_id), move |s| feed.push(s)).map_err(|e| format!("input B device: {e}"))?;
        health.push(Arc::clone(&b.health));
        inputs.push(b);
        let mut stereo = Vec::new();
        let pipe = Arc::clone(&s.pipe);
        let a = start_input_with(d.input_a.as_deref(), move |x| {
            stereo.clear();
            comb.process(x, &mut stereo);
            locked(&pipe).process(&stereo);
        })
        .map_err(|e| format!("input A device: {e}"))?;
        health.push(Arc::clone(&a.health));
        inputs.push(a);
    } else {
        let pipe = Arc::clone(&s.pipe);
        let a = start_input_with(d.input_a.as_deref(), move |x| locked(&pipe).process(x)).map_err(|e| format!("input device: {e}"))?;
        health.push(Arc::clone(&a.health));
        inputs.push(a);
    }
    let mut outputs = Vec::new();
    let out_a = Arc::clone(&s.out_a);
    let a = start_output_with(d.output_a.as_deref(), move |o| locked(&out_a).render(o)).map_err(|e| format!("output A device: {e}"))?;
    health.push(Arc::clone(&a.health));
    outputs.push(a);
    if let (Some(id), Some(play)) = (&d.output_b, &s.out_b) {
        let play = Arc::clone(play);
        let b = start_output_with(Some(id), move |o| locked(&play).render(o)).map_err(|e| format!("output B device: {e}"))?;
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
pub use crate::cat::{open_keyers, status as cat_status, test as test_rig, test_key as test_rig_key, CatLinks};

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
