//! The running client: connection, audio streams, rig control and status.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rr_audio::codec::EncoderSettings;
use rr_client::{ClientSettings, Event, RadioClient, Scope};
use rr_radio::LocalPtt;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};

use crate::platform::{self, AudioHandle, Devices};
use crate::routing::{audible, effective_input, InputSel, OutputSel, Routing};
use crate::settings::Settings;
use crate::status::{ChannelDto, ConnInfo, Meters, Phase, PlanDto, SpeakerDto, Status, UserDto};

/// Shared application state registered with Tauri.
#[derive(Debug, Default)]
pub struct AppState {
    /// Saved settings.
    pub settings: Mutex<Settings>,
    /// Where settings are stored.
    pub settings_path: Mutex<std::path::PathBuf>,
    /// The running client, if connected.
    pub engine: Mutex<Option<Engine>>,
    /// Meter-only input stream while not connected.
    pub monitor: Mutex<Option<platform::Monitor>>,
    /// Live two-radio routing, read by the audio callbacks.
    pub routing: Arc<Routing>,
    /// The OTRSP serial listener, when a port is configured.
    pub otrsp: Mutex<Option<platform::Otrsp>>,
    /// Why the OTRSP port could not be opened, if it failed.
    pub otrsp_error: Mutex<Option<String>>,
}

/// Lock helper that survives a poisoned mutex.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a hotkey does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    /// Push to talk.
    Ptt,
    /// Whisper to the configured channel.
    Whisper,
    /// Listen in on the configured channel.
    ListenIn,
}

/// A connected (or connecting) client with everything it owns.
#[derive(Debug)]
pub struct Engine {
    /// The client.
    pub client: RadioClient,
    conn: Arc<Mutex<ConnInfo>>,
    remote: Arc<Mutex<HashSet<u32>>>,
    audio: Option<AudioHandle>,
    tasks: Vec<JoinHandle<()>>,
    station: Option<tokio::task::JoinHandle<()>>,
    _local: Option<LocalPtt>,
}

impl Drop for Engine {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
        // Dropping the station task drops the rig keyer, which releases the key line.
        if let Some(s) = &self.station {
            s.abort();
        }
    }
}

fn client_settings(s: &Settings, in_p: &platform::Params, out_p: &platform::Params, out_b: Option<&platform::Params>) -> ClientSettings {
    let a = &s.audio;
    let capture = rr_audio::capture::CaptureConfig {
        device_rate: in_p.rate,
        device_channels: usize::from(in_p.channels),
        channel_map: a.map(),
        gain: a.input_gain,
        tx_mode: a.mode(),
        vox_threshold_db: a.vox_threshold_db,
        vox_hang_ms: a.vox_hang_ms,
        encoder: EncoderSettings { bitrate_bps: a.bitrate_kbps * 1000, frame_ms: 20, stereo: a.stereo },
    };
    ClientSettings {
        host: s.server.host.clone(),
        port: s.server.port,
        username: if s.server.username.trim().is_empty() { "RemoteRadio".into() } else { s.server.username.clone() },
        password: (!s.server.password.is_empty()).then(|| s.server.password.clone()),
        scope: if s.server.scope.trim().is_empty() { Scope::Everything } else { Scope::Channel(s.server.scope.clone()) },
        capture,
        output_rate: out_p.rate,
        output_rate_b: out_b.map(|p| p.rate),
        bitrate_bps: a.bitrate_kbps * 1000,
        max_frame_ms: a.max_frame_ms,
        force_tcp: a.force_tcp,
        ..ClientSettings::default()
    }
}

/// Track connection phase and remote PTT from client events.
async fn watch_events(client: RadioClient, conn: Arc<Mutex<ConnInfo>>, remote: Arc<Mutex<HashSet<u32>>>) {
    let mut rx = client.subscribe();
    loop {
        let ev = match rx.recv().await {
            Ok(ev) => ev,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };
        let mut c = lock(&conn);
        match ev {
            Event::Connecting => {
                c.phase = Phase::Connecting;
                c.scope_missing = false;
            }
            Event::Connected { .. } => {
                c.phase = Phase::Connected;
                c.error = None;
            }
            Event::Disconnected { reason } => {
                c.phase = Phase::Disconnected;
                c.error = Some(reason);
                lock(&remote).clear();
            }
            Event::Rejected { reason } => {
                c.phase = Phase::Disconnected;
                c.error = Some(if reason.is_empty() { "login rejected".into() } else { reason });
            }
            Event::ScopeNotFound(name) => {
                c.scope_missing = true;
                c.error = Some(format!("channel \"{name}\" not found on this server"));
            }
            Event::Transport { udp } => c.udp = Some(udp),
            Event::Denied(why) => c.error = Some(why),
            Event::RemotePtt { session, down } => {
                let mut r = lock(&remote);
                if down {
                    let _ = r.insert(session);
                } else {
                    let _ = r.remove(&session);
                }
            }
            Event::Roster | Event::Bandwidth(_) => {}
        }
    }
}

impl Engine {
    /// Build and start everything from `settings`.
    pub async fn start(app: &AppHandle, settings: &Settings, routing: &Arc<Routing>) -> Result<Self, String> {
        let a = &settings.audio;
        let devices = Devices::from_settings(a);
        let in_id = match effective_input(routing.input(), devices.input_b.is_some()) {
            InputSel::A => devices.input_a.as_deref(),
            InputSel::B => devices.input_b.as_deref(),
        };
        let in_p = platform::device_params(in_id, true).map_err(|e| format!("input device: {e}"))?;
        let out_p = platform::device_params(devices.output_a.as_deref(), false).map_err(|e| format!("output A device: {e}"))?;
        let out_b = match &devices.output_b {
            Some(id) => Some(platform::device_params(Some(id), false).map_err(|e| format!("output B device: {e}"))?),
            None => None,
        };
        let cs = client_settings(settings, &in_p, &out_p, out_b.as_ref());
        let (client, endpoints) = RadioClient::new(cs).map_err(|e| e.to_string())?;
        client.set_master_gain(a.master_gain);
        client.set_stereo_mode(if a.mono_output {
            rr_audio::playout::StereoMode::Mono
        } else {
            rr_audio::playout::StereoMode::Stereo
        });
        apply_outputs(&client, routing.output());
        let audio = platform::start_audio(devices, endpoints, Arc::clone(routing))?;
        let _connection = client.start();

        let conn = Arc::new(Mutex::new(ConnInfo { phase: Phase::Connecting, ..ConnInfo::default() }));
        let remote = Arc::new(Mutex::new(HashSet::new()));
        let mut tasks = vec![tauri::async_runtime::spawn(watch_events(client.clone(), Arc::clone(&conn), Arc::clone(&remote)))];

        let mut engine = Self { client: client.clone(), conn, remote, audio: Some(audio), tasks: Vec::new(), station: None, _local: None };
        if settings.ptt.station.enabled {
            match platform::open_station(&settings.ptt.station.clone().sanitised()) {
                Ok(station) => engine.station = Some(client.attach_station(station)),
                Err(e) => lock(&engine.conn).error = Some(format!("Station PTT: {e}")),
            }
        }
        if settings.ptt.local.enabled {
            let c = client.clone();
            match platform::open_local(&settings.ptt.local, move |m| c.set_ptt(m.down)) {
                Ok(l) => engine._local = Some(l),
                Err(e) => lock(&engine.conn).error = Some(format!("Local PTT: {e}")),
            }
        }
        tasks.push(tauri::async_runtime::spawn(status_ticker(app.clone(), engine.snapshot_source())));
        engine.tasks = tasks;
        Ok(engine)
    }

    fn snapshot_source(&self) -> SnapshotSource {
        SnapshotSource {
            client: self.client.clone(),
            conn: Arc::clone(&self.conn),
            remote: Arc::clone(&self.remote),
        }
    }

    /// Stop everything and release the sound card and the rig.
    pub async fn stop(self) {
        self.client.set_ptt(false);
        self.client.disconnect().await;
    }

    /// Current sound-card health.
    pub fn audio_health(&self) -> (u64, String) {
        self.audio.as_ref().map_or((0, String::new()), AudioHandle::health)
    }
}

/// Everything the ticker needs without owning the engine.
#[derive(Debug, Clone)]
struct SnapshotSource {
    client: RadioClient,
    conn: Arc<Mutex<ConnInfo>>,
    remote: Arc<Mutex<HashSet<u32>>>,
}

impl SnapshotSource {
    fn status(&self, audio: (u64, String)) -> Status {
        let c = self.client.capture();
        let conn = lock(&self.conn).clone();
        Status {
            phase: conn.phase,
            error: conn.error,
            scope_missing: conn.scope_missing,
            udp: conn.udp,
            own_session: self.client.own_session(),
            own_channel: self.client.own_channel(),
            users: self
                .client
                .users()
                .into_iter()
                .map(|u| UserDto { session: u.session, name: u.name, channel_id: u.channel_id, self_mute: u.self_mute, self_deaf: u.self_deaf })
                .collect(),
            channels: self
                .client
                .channels()
                .into_iter()
                .map(|ch| ChannelDto { id: ch.id, parent_id: ch.parent_id, name: ch.name })
                .collect(),
            speakers: self
                .client
                .speakers()
                .into_iter()
                .map(|s| SpeakerDto {
                    session: s.speaker,
                    active: s.active,
                    depth_ms: s.depth_ms,
                    target_ms: s.target_ms,
                    jitter_ms: s.jitter_ms,
                    ratio_ppm: s.ratio_ppm,
                    lost: s.lost,
                    late: s.late,
                    underruns: s.underruns,
                })
                .collect(),
            plan: self.client.bandwidth_plan().map(|p| PlanDto {
                bitrate_kbps: p.bitrate_bps as f32 / 1000.0,
                frame_ms: p.frame_ms,
                clamped: p.clamped,
                wire_kbps: p.wire_bps as f32 / 1000.0,
            }),
            transmitting: c.is_transmitting(),
            ptt_down: self.client.ptt_held(),
            sent: self.client.packet_counts().0,
            dropped: self.client.packet_counts().1,
            audio_errors: audio.0,
            audio_error: audio.1,
            remote_ptt: {
                let mut v: Vec<u32> = lock(&self.remote).iter().copied().collect();
                v.sort_unstable();
                v
            },
        }
    }
}

/// Ten status updates a second until the engine is dropped.
async fn status_ticker(app: AppHandle, src: SnapshotSource) {
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        let _ = tick.tick().await;
        let health = {
            let state = app.state::<AppState>();
            let eng = lock(&state.engine);
            eng.as_ref().map_or((0, String::new()), Engine::audio_health)
        };
        let _ = app.emit("status", src.status(health));
    }
}


/// Mute the output that is not selected (both play in Mix).
fn apply_outputs(client: &RadioClient, sel: OutputSel) {
    let b = client.playout_b();
    let (play_a, play_b) = audible(sel, b.is_some());
    client.playout().set_muted(!play_a);
    if let Some(b) = b {
        b.set_muted(!play_b);
    }
}

/// Current levels: from the running client, else from the idle input monitor.
fn levels(state: &AppState) -> Meters {
    if let Some(e) = lock(&state.engine).as_ref() {
        let c = e.client.capture();
        let (in_l, in_r) = c.take_peaks();
        let (mut out_l, mut out_r, mut out_clips) = (0.0_f32, 0.0_f32, 0_u64);
        for p in std::iter::once(e.client.playout()).chain(e.client.playout_b()) {
            let (l, r) = p.take_peaks();
            out_l = out_l.max(l);
            out_r = out_r.max(r);
            out_clips += p.clip_count();
        }
        return Meters { in_l, in_r, out_l, out_r, in_clips: c.clip_count(), out_clips, live: true };
    }
    match lock(&state.monitor).as_ref() {
        Some(m) => {
            let (in_l, in_r, in_clips) = m.take();
            Meters { in_l, in_r, in_clips, live: true, ..Meters::default() }
        }
        None => Meters::default(),
    }
}

/// Level meters, 25 times a second, for the life of the app.
pub async fn levels_ticker(app: AppHandle) {
    let mut tick = tokio::time::interval(Duration::from_millis(40));
    loop {
        let _ = tick.tick().await;
        let m = levels(&app.state::<AppState>());
        let _ = app.emit("levels", m);
    }
}

/// Open the meter-only input monitor (used while not connected), replacing any
/// previous one. Failures are logged; the meter then just stays idle.
pub fn restart_monitor(state: &AppState) {
    let mut slot = lock(&state.monitor);
    *slot = None;
    if lock(&state.engine).is_some() {
        return;
    }
    let a = lock(&state.settings).audio.clone();
    let d = Devices::from_settings(&a);
    let device = match effective_input(state.routing.input(), d.input_b.is_some()) {
        InputSel::A => d.input_a,
        InputSel::B => d.input_b,
    };
    match platform::Monitor::start(device.as_deref(), a.input_gain, a.map()) {
        Ok(m) => *slot = Some(m),
        Err(e) => tracing::info!("input level monitor unavailable: {e}"),
    }
}

/// The routing as shown in the UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RouteDto {
    /// Selected input.
    pub input: InputSel,
    /// Selected output.
    pub output: OutputSel,
    /// OTRSP is listening on its port.
    pub otrsp_open: bool,
    /// Why OTRSP could not be opened, if it failed.
    pub otrsp_error: Option<String>,
}

/// Snapshot of the routing for the UI.
pub fn route_dto(state: &AppState) -> RouteDto {
    RouteDto {
        input: state.routing.input(),
        output: state.routing.output(),
        otrsp_open: lock(&state.otrsp).is_some(),
        otrsp_error: lock(&state.otrsp_error).clone(),
    }
}

/// Apply a routing change to the running audio and tell the UI.
pub fn route_changed(app: &AppHandle, input_changed: bool) {
    let state = app.state::<AppState>();
    if let Some(e) = lock(&state.engine).as_ref() {
        apply_outputs(&e.client, state.routing.output());
    }
    if input_changed {
        restart_monitor(&state);
    }
    let _ = app.emit("route", route_dto(&state));
}

/// Handle an OTRSP command from the logger.
fn on_otrsp(app: &AppHandle, cmd: &rr_radio::OtrspCommand) {
    let state = app.state::<AppState>();
    if !lock(&state.settings).audio.otrsp_enabled {
        return;
    }
    let (input, output) = state.routing.apply_otrsp(cmd);
    if input || output {
        route_changed(app, input);
    }
}

/// Open (or close) the OTRSP listener to match the settings.
pub fn restart_otrsp(app: &AppHandle) {
    let state = app.state::<AppState>();
    let port = lock(&state.settings).audio.otrsp_port.trim().to_owned();
    *lock(&state.otrsp) = None;
    *lock(&state.otrsp_error) = None;
    if !port.is_empty() {
        let handle = app.clone();
        match platform::open_otrsp(&port, move |cmd| on_otrsp(&handle, &cmd)) {
            Ok(l) => *lock(&state.otrsp) = Some(l),
            Err(e) => {
                tracing::warn!("{e}");
                *lock(&state.otrsp_error) = Some(e);
            }
        }
    }
    let _ = app.emit("route", route_dto(&state));
}

/// Run a hotkey press or release.
pub fn hotkey(app: &AppHandle, action: HotkeyAction, down: bool) {
    let state = app.state::<AppState>();
    let Some(client) = lock(&state.engine).as_ref().map(|e| e.client.clone()) else {
        return;
    };
    let hk = lock(&state.settings).hotkeys.clone();
    match action {
        HotkeyAction::Ptt => client.set_ptt(down),
        HotkeyAction::Whisper => {
            if let Some(t) = hk.whisper_target {
                let _task = tauri::async_runtime::spawn(async move {
                    let _ = client.whisper(t, down).await;
                });
            }
        }
        HotkeyAction::ListenIn => {
            if let Some(t) = hk.listen_target {
                let _task = tauri::async_runtime::spawn(async move {
                    let _ = client.listen_in(t, down, hk.mute_home_on_listen).await;
                });
            }
        }
    }
}
