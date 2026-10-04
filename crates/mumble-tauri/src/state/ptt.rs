//! Rig-PTT controller: owns the `rr-ptt` watchdog and serial I/O on a dedicated
//! worker thread, and bridges it to the Mumble plugin-message channel.
//!
//! # Why a worker thread
//!
//! `serialport`'s handle is not guaranteed `Send`-safe to use across threads,
//! and the PTT loop must run continuously (the watchdog deadman and the
//! operator's footswitch poll both need a steady tick) independent of the Tauri
//! command threads. So one thread owns the serial sink/source for its whole
//! life; [`PttController`] is a cheap `Send` handle that posts commands to it.
//!
//! # Two roles (CONTEXT §14d)
//!
//! - **Station** drives an output line to key the rig. It keys on a remote
//!   operator command ([`PttController::set_remote_key`]) and/or inbound-audio
//!   VOX ([`PttController::set_vox_active`]). A lead-in and hang-time smooth the
//!   transmitter's T/R switch. Every key passes through the
//!   [`Watchdog`](rr_ptt::Watchdog), so a stuck command cannot hold the rig.
//! - **Operator** senses a footswitch on an input line and reports key/unkey
//!   intent through the `on_intent` callback, which the caller turns into an
//!   outbound `remote-radio/ptt` plugin message.
//!
//! Only desktop builds have serial PTT; the whole module is gated out on
//! Android.
#![cfg(not(target_os = "android"))]

use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use rr_ptt::{
    ControlLine, PttIntent, PttSource, SenseLine, SerialPttConfig, SerialPttSink, SerialPttSource,
    Watchdog, WatchdogConfig,
};
use tracing::{info, warn};

use super::rr_plugin::Role;
use super::types::{PttConfig, PttLine};

/// How often the worker ticks: watchdog timers + footswitch poll. 15 ms is
/// imperceptible for PTT and keeps serial-line churn negligible.
const TICK: Duration = Duration::from_millis(15);

/// Commands posted to the worker thread.
enum Cmd {
    /// Station: a remote operator asked to key/unkey.
    RemoteKey(bool),
    /// Station: the inbound-audio VOX gate changed.
    VoxActive(bool),
    /// Stop the worker and release the line.
    Stop,
}

/// A `Send` handle to the PTT worker thread. Dropping it stops the worker,
/// which releases the key line.
pub(crate) struct PttController {
    tx: Sender<Cmd>,
    handle: Option<thread::JoinHandle<()>>,
}

impl PttController {
    /// Start a worker for `config` in `role`.
    ///
    /// `on_intent` is invoked (on the worker thread) with the operator's
    /// footswitch intent each time it changes; for a Station it is never
    /// called. The caller turns an `Operator` intent into an outbound
    /// `remote-radio/ptt` plugin message.
    ///
    /// # Errors
    /// Returns an error string if the serial port cannot be opened.
    pub(crate) fn start(
        config: &PttConfig,
        role: Role,
        on_intent: impl Fn(PttIntent) + Send + 'static,
    ) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let serial_cfg = SerialPttConfig {
            port: config.port.clone(),
            active_low: config.active_low,
        };
        let wd_cfg = WatchdogConfig {
            deadman: Duration::from_millis(u64::from(config.deadman_ms)),
            max_transmit: Duration::from_secs(u64::from(config.max_tx_secs)),
        };
        let line = config.line;
        let hang = Duration::from_millis(u64::from(config.hang_ms));
        let vox_hang = Duration::from_millis(u64::from(config.vox_hang_ms));

        // Open the serial resource up front so an open error is reported to the
        // caller synchronously, before the thread is spawned.
        match role {
            Role::Station => {
                let ctrl_line = control_line(line)
                    .ok_or_else(|| format!("{line:?} is not a writable output line"))?;
                let sink = SerialPttSink::open(&serial_cfg, ctrl_line)
                    .map_err(|e| format!("open PTT output: {e}"))?;
                let handle = thread::Builder::new()
                    .name("rr-ptt-station".into())
                    .spawn(move || {
                        station_loop(Watchdog::new(sink, wd_cfg), hang, vox_hang, &rx);
                    })
                    .map_err(|e| format!("spawn PTT thread: {e}"))?;
                Ok(Self { tx, handle: Some(handle) })
            }
            Role::Operator => {
                let sense = sense_line(line)
                    .ok_or_else(|| format!("{line:?} is not a readable input line"))?;
                let source = SerialPttSource::open(&serial_cfg, sense)
                    .map_err(|e| format!("open PTT input: {e}"))?;
                let handle = thread::Builder::new()
                    .name("rr-ptt-operator".into())
                    .spawn(move || operator_loop(source, &rx, on_intent))
                    .map_err(|e| format!("spawn PTT thread: {e}"))?;
                Ok(Self { tx, handle: Some(handle) })
            }
        }
    }

    /// Station: a remote operator command keyed (`true`) or unkeyed (`false`).
    pub(crate) fn set_remote_key(&self, down: bool) {
        let _ = self.tx.send(Cmd::RemoteKey(down));
    }

    /// Station: the inbound-audio VOX gate opened (`true`) or closed (`false`).
    pub(crate) fn set_vox_active(&self, active: bool) {
        let _ = self.tx.send(Cmd::VoxActive(active));
    }
}

impl Drop for PttController {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Station worker: key the rig when a remote command OR VOX wants it, through
/// the safety watchdog, honouring hang-time on release.
///
/// `vox_hang` debounces VOX specifically: when inbound audio stops, VOX is held
/// "active" for this long so inter-syllable gaps in the operator's speech do not
/// drop PTT. `hang` is the separate overall release hold.
fn station_loop(
    mut wd: Watchdog<SerialPttSink>,
    hang: Duration,
    vox_hang: Duration,
    rx: &mpsc::Receiver<Cmd>,
) {
    info!("PTT station worker started");
    let mut remote = false;
    let mut vox_raw = false;
    // While VOX was recently active, hold it until this instant (vox_hang), so a
    // brief gap between syllables does not drop the key.
    let mut vox_hold_until: Option<Instant> = None;
    // When the combined intent drops, hold the key until this instant (hang).
    let mut release_at: Option<Instant> = None;

    loop {
        // Drain any pending commands without blocking the tick cadence.
        loop {
            match rx.try_recv() {
                Ok(Cmd::RemoteKey(d)) => remote = d,
                Ok(Cmd::VoxActive(a)) => {
                    vox_raw = a;
                    if a {
                        vox_hold_until = None;
                    } else if vox_hang > Duration::ZERO {
                        // Start the VOX hold on the drop.
                        vox_hold_until = Some(Instant::now() + vox_hang);
                    } else {
                        vox_hold_until = None;
                    }
                }
                Ok(Cmd::Stop) | Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = wd.unkey(Instant::now());
                    info!("PTT station worker stopped");
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }

        let now = Instant::now();
        // Effective VOX: raw-on, or within the post-drop hold window.
        let vox = vox_raw || vox_hold_until.is_some_and(|t| now < t);
        if !vox {
            vox_hold_until = None;
        }
        let want = remote || vox;
        if want {
            release_at = None;
            if let Err(e) = wd.key(now) {
                warn!("PTT key failed (watchdog failed closed): {e}");
            } else {
                wd.keepalive(now);
            }
        } else {
            // Intent dropped: start/continue the hang-time countdown.
            match release_at {
                None if wd.is_keyed() => release_at = Some(now + hang),
                Some(at) if now >= at => {
                    release_at = None;
                    if let Err(e) = wd.unkey(now) {
                        warn!("PTT unkey failed (watchdog failed closed): {e}");
                    }
                }
                _ => {
                    // Within hang window: keep the key alive so the deadman does
                    // not release it before the hang elapses.
                    if wd.is_keyed() {
                        wd.keepalive(now);
                    }
                }
            }
        }

        if let Err(e) = wd.tick(now) {
            warn!("PTT watchdog tick failed closed: {e}");
        }
        thread::sleep(TICK);
    }
}

/// Operator worker: poll the footswitch line and report each change of intent.
fn operator_loop(
    mut source: SerialPttSource,
    rx: &mpsc::Receiver<Cmd>,
    on_intent: impl Fn(PttIntent),
) {
    info!("PTT operator worker started");
    let mut last: Option<PttIntent> = None;
    loop {
        match rx.try_recv() {
            Ok(Cmd::Stop) | Err(mpsc::TryRecvError::Disconnected) => {
                info!("PTT operator worker stopped");
                return;
            }
            // The operator role ignores remote-key / vox commands.
            Ok(_) | Err(mpsc::TryRecvError::Empty) => {}
        }
        if let Some(intent) = source.poll() {
            if last != Some(intent) {
                last = Some(intent);
                on_intent(intent);
            }
        }
        thread::sleep(TICK);
    }
}

use super::rr_plugin::PttPayload;
use super::AppState;
use std::sync::atomic::{AtomicU64, Ordering};

impl AppState {
    /// (Re)start the live PTT controller for the active session from its stored
    /// [`PttConfig`]. Stops any existing controller first (which releases the
    /// key line). A disabled or portless config just stops PTT.
    pub(crate) fn restart_ptt(&self) {
        // Tear down the old controller + VOX poller before opening the port again.
        self.stop_vox_poller();
        if let Ok(mut slot) = self.ptt_controller.lock() {
            *slot = None;
        }

        let config = self.ptt_config_for(None);
        if !config.enabled || config.port.is_empty() {
            return;
        }

        let role = match config.role {
            super::types::PttRole::Operator => Role::Operator,
            super::types::PttRole::Station => Role::Station,
        };

        // Operator: each footswitch change becomes an outbound remote-radio/ptt
        // plugin message to the channel. Spawn onto the Tauri async runtime via
        // the app handle (the worker thread is not in the runtime).
        let app = self.app_handle();
        let seq = std::sync::Arc::new(AtomicU64::new(0));
        let on_intent = move |intent: PttIntent| {
            let Some(app) = app.clone() else { return };
            let seq = seq.fetch_add(1, Ordering::Relaxed);
            let down = intent.is_key();
            // Fire-and-forget the outbound send on the Tauri runtime; the
            // JoinHandle is intentionally dropped (named, not `let _`, so
            // clippy does not read it as dropping the future itself).
            let _send_task = tauri::async_runtime::spawn(async move {
                use tauri::Manager;
                let state = app.state::<AppState>();
                let channel = state.current_channel();
                let payload = match super::rr_plugin::encode(&PttPayload { down, seq }) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("PTT: encode outbound intent failed: {e}");
                        return;
                    }
                };
                if let Err(e) = state
                    .send_plugin_message(
                        super::rr_plugin::PLUGIN_NAME.to_owned(),
                        super::rr_plugin::PAYLOAD_TYPE_PTT.to_owned(),
                        payload,
                        Vec::new(),
                        channel,
                    )
                    .await
                {
                    warn!("PTT: send outbound intent failed: {e}");
                }
            });
        };

        match PttController::start(&config, role, on_intent) {
            Ok(ctrl) => {
                if let Ok(mut slot) = self.ptt_controller.lock() {
                    *slot = Some(ctrl);
                }
                info!("PTT controller started ({:?})", config.role);
            }
            Err(e) => {
                warn!("PTT controller failed to start: {e}");
                return;
            }
        }

        // Mode B (§14d): a Station with VOX enabled keys on inbound-audio
        // activity. `talking_sessions` on the active session is the inbound-talk
        // truth (maintained by the event handler), so a light poller mirrors it
        // onto the controller's VOX gate. Operator role has no VOX.
        if matches!(role, Role::Station) && config.vox {
            self.start_vox_poller();
        }
    }

    /// Spawn (replacing any existing) a task that mirrors inbound talk activity
    /// onto the Station PTT controller's VOX gate.
    fn start_vox_poller(&self) {
        let Some(app) = self.app_handle() else { return };
        self.stop_vox_poller();
        let handle = tauri::async_runtime::spawn(async move {
            use tauri::Manager;
            let state = app.state::<AppState>();
            let mut last = false;
            loop {
                tokio::time::sleep(TICK).await;
                let active = state
                    .inner
                    .snapshot()
                    .lock()
                    .map(|s| !s.audio.talking_sessions.is_empty())
                    .unwrap_or(false);
                if active != last {
                    last = active;
                    state.set_ptt_vox_active(active);
                }
                // Stop when the controller is gone (PTT disabled/reconfigured).
                let running = state
                    .ptt_controller
                    .lock()
                    .map(|g| g.is_some())
                    .unwrap_or(false);
                if !running {
                    break;
                }
            }
        });
        if let Ok(mut slot) = self.ptt_vox_task.lock() {
            if let Some(old) = slot.replace(handle) {
                old.abort();
            }
        }
    }

    /// Abort the VOX poller task, if any.
    fn stop_vox_poller(&self) {
        if let Ok(mut slot) = self.ptt_vox_task.lock() {
            if let Some(h) = slot.take() {
                h.abort();
            }
        }
    }

    /// Stop the live PTT controller (releases the key line) and its VOX poller.
    /// Called on disconnect so a vanished link cannot leave a transmitter keyed.
    pub(crate) fn stop_ptt(&self) {
        self.stop_vox_poller();
        if let Ok(mut slot) = self.ptt_controller.lock() {
            *slot = None;
        }
    }

    /// Station: apply a remote operator's PTT intent to the live controller.
    /// A no-op when this session is not a running Station controller.
    pub(crate) fn apply_remote_ptt(&self, down: bool) {
        if let Ok(slot) = self.ptt_controller.lock() {
            if let Some(ctrl) = slot.as_ref() {
                ctrl.set_remote_key(down);
            }
        }
    }

    /// Station: forward the inbound-audio VOX gate to the live controller.
    pub(crate) fn set_ptt_vox_active(&self, active: bool) {
        if let Ok(slot) = self.ptt_controller.lock() {
            if let Some(ctrl) = slot.as_ref() {
                ctrl.set_vox_active(active);
            }
        }
    }
}

/// Map the settings-layer [`PttLine`] onto a writable [`ControlLine`].
fn control_line(line: PttLine) -> Option<ControlLine> {
    match line {
        PttLine::Dtr => Some(ControlLine::Dtr),
        PttLine::Rts => Some(ControlLine::Rts),
        _ => None,
    }
}

/// Map the settings-layer [`PttLine`] onto a readable [`SenseLine`].
fn sense_line(line: PttLine) -> Option<SenseLine> {
    match line {
        PttLine::Cts => Some(SenseLine::Cts),
        PttLine::Dsr => Some(SenseLine::Dsr),
        PttLine::Cd => Some(SenseLine::Cd),
        PttLine::Ri => Some(SenseLine::Ri),
        _ => None,
    }
}
