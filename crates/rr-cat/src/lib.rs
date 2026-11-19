//! CAT PTT: key a radio with its own CAT commands instead of an RS-232
//! control line.
//!
//! [`CatPtt`] implements [`rr_ptt::PttSink`], so it plugs into the same
//! fail-safe watchdog, VOX, hang and two-radio logic as a serial key line. A
//! background thread owns the connection: it reconnects on its own, keeps
//! network links alive, and reports what the radio says about transmit.
//!
//! Radio models: Elecraft K4 ([`k4`]). A new model adds a config struct, a
//! variant of [`RigModel`], and its command set.

pub mod k4;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rr_ptt::{PttError, PttSink};
use serde::{Deserialize, Serialize};

/// Supported radio models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RigModel {
    /// Elecraft K4.
    #[default]
    K4,
}

impl RigModel {
    /// Name shown to the user.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::K4 => "Elecraft K4",
        }
    }
}

/// CAT settings for one radio. Each model keeps its own section, so switching
/// model and back does not lose settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RigConfig {
    /// Which radio this is.
    pub model: RigModel,
    /// Elecraft K4 settings.
    pub k4: k4::K4Config,
}

/// What the CAT link is doing, for the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CatStatus {
    /// Radio model name.
    pub model: String,
    /// The connection is open.
    pub connected: bool,
    /// Last problem, if any.
    pub error: Option<String>,
    /// We are asking the radio to transmit.
    pub keyed: bool,
    /// What the radio last reported: transmitting, receiving, or unknown.
    pub radio_tx: Option<bool>,
}

/// Wait between reconnect attempts.
const RETRY: Duration = Duration::from_secs(2);
/// How often PING (network) and TQ (while keyed) are sent.
const POLL: Duration = Duration::from_secs(1);

enum Cmd {
    Key(bool),
    Stop,
}

#[derive(Debug, Default)]
struct Shared {
    status: Mutex<CatStatus>,
    connected: AtomicBool,
}

impl Shared {
    fn edit(&self, f: impl FnOnce(&mut CatStatus)) {
        f(&mut self.status.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

/// Read-only view of a [`CatPtt`] for status display.
#[derive(Debug, Clone)]
pub struct CatMonitor(Arc<Shared>);

impl CatMonitor {
    /// Current status.
    #[must_use]
    pub fn status(&self) -> CatStatus {
        self.0.status.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// A radio keyed by CAT. Dropping it sends the unkey command (if this
/// program keyed the radio) and closes the connection.
#[derive(Debug)]
pub struct CatPtt {
    tx: Sender<Cmd>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl CatPtt {
    /// Start talking to the radio in the background. Never fails: an
    /// unreachable radio is retried and reported through [`CatPtt::monitor`].
    ///
    /// # Errors
    /// Only if the worker thread cannot be started.
    pub fn open(cfg: &RigConfig) -> std::io::Result<Self> {
        let shared = Arc::new(Shared::default());
        shared.edit(|s| s.model = cfg.model.label().to_owned());
        let (tx, rx) = channel();
        let worker_shared = Arc::clone(&shared);
        let cfg = cfg.clone();
        let thread = thread::Builder::new()
            .name("rr-cat".into())
            .spawn(move || run(&cfg, &rx, &worker_shared))?;
        Ok(Self { tx, shared, thread: Some(thread) })
    }

    /// A handle for reading the status.
    #[must_use]
    pub fn monitor(&self) -> CatMonitor {
        CatMonitor(Arc::clone(&self.shared))
    }
}

impl PttSink for CatPtt {
    fn set_keyed(&mut self, down: bool) -> rr_ptt::Result<()> {
        // Keying a radio we cannot reach is an error, so the watchdog fails
        // closed. Unkeying always succeeds: it is sent once the link is back.
        if down && !self.shared.connected.load(Ordering::Relaxed) {
            return Err(PttError::Io("radio not connected over CAT".into()));
        }
        self.tx
            .send(Cmd::Key(down))
            .map_err(|_| PttError::Io("CAT worker stopped".into()))
    }
}

impl Drop for CatPtt {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Key(false));
        let _ = self.tx.send(Cmd::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The commands of a model.
struct Commands {
    key_on: &'static str,
    key_off: &'static str,
    query_tx: &'static str,
    ping: &'static str,
}

fn commands(model: RigModel) -> Commands {
    match model {
        RigModel::K4 => Commands { key_on: k4::KEY_ON, key_off: k4::KEY_OFF, query_tx: k4::QUERY_TX, ping: k4::PING },
    }
}

fn connect(cfg: &RigConfig) -> Result<Box<dyn k4::Conn>, String> {
    match cfg.model {
        RigModel::K4 => k4::connect(&cfg.k4),
    }
}

fn transmit_state(model: RigModel, reply: &str) -> Option<bool> {
    match model {
        RigModel::K4 => k4::wire::transmit_state(reply),
    }
}

/// Wanted key state, and whether this program keyed the radio (so we only
/// ever send unkey for a transmission we started).
struct KeyState {
    want: bool,
    keyed_by_us: bool,
}

/// The worker: connect, keep the radio in the wanted state, reconnect.
fn run(cfg: &RigConfig, rx: &Receiver<Cmd>, shared: &Shared) {
    let cmds = commands(cfg.model);
    let mut key = KeyState { want: false, keyed_by_us: false };
    loop {
        match connect(cfg) {
            Ok(mut conn) => {
                shared.connected.store(true, Ordering::Relaxed);
                shared.edit(|s| {
                    s.connected = true;
                    s.error = None;
                });
                let reason = session(conn.as_mut(), &cmds, cfg.model, rx, shared, &mut key);
                shared.connected.store(false, Ordering::Relaxed);
                shared.edit(|s| {
                    s.connected = false;
                    s.radio_tx = None;
                });
                match reason {
                    End::Stop => return,
                    End::Lost(e) => {
                        tracing::warn!("CAT link lost: {e}");
                        shared.edit(|s| s.error = Some(e));
                    }
                }
            }
            Err(e) => shared.edit(|s| s.error = Some(e)),
        }
        // Wait before retrying, still taking commands (and Stop).
        let until = Instant::now() + RETRY;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(Cmd::Key(d)) => {
                    // A new key-down cannot start while offline; a release
                    // always counts and is sent once the link is back.
                    key.want = d && key.want;
                    let want = key.want;
                    shared.edit(|s| s.keyed = want);
                }
                Ok(Cmd::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
    }
}

enum End {
    Stop,
    Lost(String),
}

fn send_key(conn: &mut dyn k4::Conn, cmds: &Commands, down: bool, key: &mut KeyState, shared: &Shared) -> std::io::Result<()> {
    if down {
        conn.send(cmds.key_on)?;
        key.keyed_by_us = true;
    } else if key.keyed_by_us {
        conn.send(cmds.key_off)?;
        key.keyed_by_us = false;
    }
    conn.send(cmds.query_tx)?;
    shared.edit(|s| s.keyed = down);
    Ok(())
}

fn session(
    conn: &mut dyn k4::Conn,
    cmds: &Commands,
    model: RigModel,
    rx: &Receiver<Cmd>,
    shared: &Shared,
    key: &mut KeyState,
) -> End {
    let io = |e: std::io::Error| End::Lost(e.to_string());
    // Back after a drop: restore what is wanted (or release what we keyed).
    if key.want || key.keyed_by_us {
        if let Err(e) = send_key(conn, cmds, key.want, key, shared) {
            return io(e);
        }
    }
    if let Err(e) = conn.send(cmds.query_tx) {
        return io(e);
    }
    let mut next_poll = Instant::now() + POLL;
    loop {
        loop {
            match rx.try_recv() {
                Ok(Cmd::Key(d)) => {
                    key.want = d;
                    if let Err(e) = send_key(conn, cmds, d, key, shared) {
                        return io(e);
                    }
                }
                Ok(Cmd::Stop) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if key.keyed_by_us {
                        let _ = conn.send(cmds.key_off);
                    }
                    return End::Stop;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
            }
        }
        match conn.recv() {
            Ok(replies) => {
                for r in replies {
                    if let Some(tx) = transmit_state(model, &r) {
                        shared.edit(|s| s.radio_tx = Some(tx));
                    }
                }
            }
            Err(e) => return io(e),
        }
        if Instant::now() >= next_poll {
            next_poll = Instant::now() + POLL;
            let r = if conn.needs_keepalive() { conn.send(cmds.ping) } else { Ok(()) };
            let r = r.and_then(|()| if key.want { conn.send(cmds.query_tx) } else { Ok(()) });
            if let Err(e) = r {
                return io(e);
            }
        }
    }
}

/// Connect, ask the radio who it is, and report what came back. Does not key
/// the radio.
///
/// # Errors
/// The connection or login problem, or no answer within three seconds.
pub fn test_connection(cfg: &RigConfig) -> Result<String, String> {
    let mut conn = connect(cfg)?;
    let query = match cfg.model {
        RigModel::K4 => k4::QUERY_ID,
    };
    conn.send(query).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        let replies = conn.recv().map_err(|e| e.to_string())?;
        if let Some(r) = replies.into_iter().find(|r| r.starts_with("ID")) {
            return Ok(r);
        }
    }
    Err("connected, but the radio did not answer the identity query".into())
}

/// What a key test saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyTestResult {
    /// The radio reported transmit while keyed.
    pub transmitted: bool,
    /// Everything the radio sent during the test, for the user.
    pub replies: String,
}

/// Key the radio for `hold`, then release it, and report whether the radio
/// said it was transmitting. Always sends the unkey command, even on error.
///
/// # Errors
/// The connection or login problem.
pub fn test_key(cfg: &RigConfig, hold: Duration) -> Result<KeyTestResult, String> {
    let cmds = commands(cfg.model);
    let mut conn = connect(cfg)?;
    let mut seen: Vec<String> = Vec::new();
    let mut transmitted = false;
    let watch = |conn: &mut dyn k4::Conn, until: Instant, seen: &mut Vec<String>, tx: &mut bool| -> Result<(), String> {
        while Instant::now() < until {
            for r in conn.recv().map_err(|e| e.to_string())? {
                if transmit_state(cfg.model, &r) == Some(true) {
                    *tx = true;
                }
                if seen.len() < 40 {
                    seen.push(r);
                }
            }
        }
        Ok(())
    };
    let keyed = conn.send(cmds.key_on).map_err(|e| e.to_string());
    let mut result = keyed.and_then(|()| {
        let end = Instant::now() + hold;
        while Instant::now() < end {
            conn.send(cmds.query_tx).map_err(|e| e.to_string())?;
            watch(conn.as_mut(), (Instant::now() + Duration::from_millis(300)).min(end), &mut seen, &mut transmitted)?;
        }
        Ok(())
    });
    let released = conn.send(cmds.key_off).map_err(|e| e.to_string());
    if result.is_ok() {
        result = released.and_then(|()| watch(conn.as_mut(), Instant::now() + Duration::from_millis(400), &mut seen, &mut transmitted));
    }
    result.map(|()| KeyTestResult { transmitted, replies: seen.join(" ") })
}

#[cfg(test)]
mod tests;
