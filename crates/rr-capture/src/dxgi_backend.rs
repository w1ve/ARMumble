//! Low-latency Windows screen capture via DXGI Desktop Duplication.
//!
//! This is the backend the Station actually uses on Windows. It replaces the
//! `xcap` baseline for one measured reason: `xcap` does a GDI `BitBlt` plus
//! `GetDIBits` readback of the entire framebuffer on every call, which measured
//! **33.6 ms** per 1920x1200 frame — more than the whole 33.3 ms budget for
//! 30 fps, and ~36x the memory-bandwidth floor for 8.8 MiB. No amount of
//! tuning around that API reaches an interactive frame rate. See
//! `doc/remote-radio/P2-BASELINE.md`.
//!
//! Desktop Duplication is structurally different in two ways that matter:
//!
//! - **Change-driven.** `AcquireNextFrame` blocks until the desktop actually
//!   changes. A static screen costs nothing instead of costing a full readback
//!   30 times a second, and a change is delivered the moment it happens rather
//!   than at the next poll tick.
//! - **GPU-side composition.** The desktop is already composited in video
//!   memory; we copy it to a staging texture and map that, instead of asking
//!   GDI to re-render and read back.
//!
//! # Why there is a worker thread
//!
//! Desktop Duplication hangs off a Direct3D 11 device, and the `windows` crate
//! deliberately does not mark COM interfaces `Send` — they are tied to the
//! thread's COM apartment. [`ScreenCapturer`] requires `Send`, because the
//! Station runs capture on a blocking task.
//!
//! So the duplication session lives on one dedicated thread for its whole life,
//! and [`DxgiCapturer`] is a `Send` handle that posts jobs to it. That is not a
//! workaround: a duplication session *wants* a stable home thread, and owning
//! one gives us the obvious place to add latest-frame coalescing later. The
//! cost is two channel hops per grab, which is sub-microsecond against a frame
//! budget measured in milliseconds.
//!
//! # Pixel format
//!
//! We ask for BGRA8, which is what the Windows desktop is natively composited
//! in, so DXGI hands it over with no conversion. The encoder has a BGRA path
//! for exactly this reason. Asking for RGBA instead would buy a prettier
//! [`PixelFormat`] at the cost of a full-frame channel swizzle every frame.
//!
//! # Things that end a duplication session
//!
//! `AcquireNextFrame` reports `AccessLost` when the display mode changes, when
//! the user hits a secure desktop (Ctrl+Alt+Del, UAC), on fast user switching,
//! and when a fullscreen app flips DWM. All of those are *normal* during a
//! remote session, so [`DxgiCapturer::grab`] rebuilds the session and retries
//! once rather than surfacing an error the caller cannot act on.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tracing::{debug, warn};
use windows_capture::d3d11::StagingTexture;
use windows_capture::dxgi_duplication_api::{
    DxgiDuplicationApi, DxgiDuplicationFormat, Error as DupError,
};
use windows_capture::monitor::Monitor;

use crate::{CaptureError, Frame, MonitorInfo, PixelFormat, Result, ScreenCapturer};

/// How long a grab waits for the desktop to change before reporting
/// [`CaptureError::NoNewFrame`].
///
/// Blocking here is the cheap, low-latency thing to do: the thread sleeps until
/// Windows has something new, so a still screen burns no CPU and a change is
/// picked up immediately. The timeout exists only so the caller regains control
/// periodically (to notice shutdown, re-read settings, or switch monitors).
pub const DEFAULT_TIMEOUT_MS: u32 = 100;

/// A unit of work for the capture thread.
///
/// Boxing a closure rather than enumerating every request keeps the protocol in
/// one place: each job carries its own reply channel, so adding an operation
/// does not mean touching a request enum, a response enum and two `match` arms.
type Job = Box<dyn FnOnce(&mut Engine) + Send>;

/// Translate a duplication error, preserving the two cases callers act on.
fn dup_err(e: DupError) -> CaptureError {
    match e {
        DupError::Timeout => CaptureError::NoNewFrame,
        other => CaptureError::Backend(other.to_string()),
    }
}

/// Translate any other backend error, matching `xcap_backend`'s helper.
fn backend_err(e: impl std::fmt::Display) -> CaptureError {
    CaptureError::Backend(e.to_string())
}

/// Map DXGI's format to ours.
///
/// `Rgba16F` only appears if a caller asks for it; we request BGRA8 and the
/// wrapper always appends BGRA8 as a fallback, so an HDR surface is converted
/// for us rather than arriving as half-floats. It is still rejected explicitly
/// instead of being silently reinterpreted as 8-bit.
fn map_format(f: DxgiDuplicationFormat) -> Result<PixelFormat> {
    match f {
        DxgiDuplicationFormat::Bgra8 => Ok(PixelFormat::Bgra8),
        DxgiDuplicationFormat::Rgba8 => Ok(PixelFormat::Rgba8),
        DxgiDuplicationFormat::Rgba16F => Err(CaptureError::UnsupportedPixelFormat(
            "DXGI_FORMAT_R16G16B16A16_FLOAT (HDR); request BGRA8 instead".to_owned(),
        )),
    }
}

/// Enumerate monitors paired with the id we expose for them.
///
/// The id is the one-based display number Windows itself uses (the `1` in
/// `\\.\DISPLAY1`), which is stable for a given display configuration and
/// readable in logs — unlike an `HMONITOR`, which is a handle that changes run
/// to run. Monitors whose index cannot be read are skipped: without an id there
/// is no way for a caller to ask for them anyway.
fn enumerate() -> Result<Vec<(u32, Monitor)>> {
    let monitors = Monitor::enumerate().map_err(backend_err)?;
    let mut out = Vec::with_capacity(monitors.len());
    for m in monitors {
        match m.index() {
            Ok(idx) => match u32::try_from(idx) {
                Ok(id) => out.push((id, m)),
                Err(_) => warn!(index = idx, "skipping monitor with out-of-range index"),
            },
            Err(e) => warn!(error = %e, "skipping monitor whose index could not be read"),
        }
    }
    if out.is_empty() {
        return Err(CaptureError::NoMonitors);
    }
    Ok(out)
}

/// An open duplication session: the DXGI side plus the staging texture we map.
struct Session {
    /// Monitor this session duplicates, so a selection change is detected.
    monitor_id: u32,
    /// The duplication itself.
    dup: DxgiDuplicationApi,
    /// Reused CPU-readable texture. Allocating one per frame would mean a
    /// `CreateTexture2D` 30+ times a second for a buffer whose size never
    /// changes.
    staging: StagingTexture,
}

impl Session {
    /// Open a duplication session for `monitor_id`.
    fn open(monitor_id: u32) -> Result<Self> {
        let monitor = enumerate()?
            .into_iter()
            .find_map(|(id, m)| (id == monitor_id).then_some(m))
            .ok_or(CaptureError::UnknownMonitor(monitor_id))?;

        // BGRA8 first: it is the desktop's native composition format, so DXGI
        // has nothing to convert. The wrapper appends BGRA8 regardless, so this
        // is really about not asking for anything more exotic.
        let dup = DxgiDuplicationApi::new_options(monitor, &[DxgiDuplicationFormat::Bgra8])
            .map_err(dup_err)?;

        // Match the staging texture to the duplication exactly; `buffer_with`
        // validates geometry and format and would otherwise reject it.
        let desc = dup.duplication_desc();
        let staging = StagingTexture::new(
            dup.device(),
            dup.width(),
            dup.height(),
            desc.ModeDesc.Format,
        )
        .map_err(backend_err)?;

        debug!(
            monitor_id,
            width = dup.width(),
            height = dup.height(),
            format = ?dup.format(),
            "opened DXGI duplication session"
        );

        Ok(Self {
            monitor_id,
            dup,
            staging,
        })
    }
}

/// The non-`Send` half: everything that touches Direct3D, confined to one
/// thread.
#[derive(Default)]
struct Engine {
    /// Monitor id requested by the caller; `None` means "primary".
    selected: Option<u32>,
    /// Current session, opened lazily and rebuilt after `AccessLost`.
    session: Option<Session>,
    /// Frames successfully delivered, so a consumer can detect drops.
    sequence: u64,
    /// How long `grab` waits for a change.
    timeout_ms: u32,
    /// Time the last successful grab spent blocked in `AcquireNextFrame`.
    ///
    /// Reported through [`ScreenCapturer::last_wait`] so a caller can separate
    /// "the screen was still" from "capture is slow". Without this, any timing
    /// of `grab` on a quiet desktop measures the user, not the code.
    last_wait: Option<Duration>,
}

impl Engine {
    /// Build an engine with the given acquire timeout.
    fn new(timeout_ms: u32) -> Self {
        Self {
            timeout_ms,
            ..Self::default()
        }
    }

    /// Resolve the monitor id to use, defaulting to the primary display.
    fn target_id(&self) -> Result<u32> {
        if let Some(id) = self.selected {
            return Ok(id);
        }
        let primary = Monitor::primary().map_err(backend_err)?;
        let all = enumerate()?;
        all.iter()
            .find_map(|&(id, m)| (m == primary).then_some(id))
            // The primary monitor should always be in the enumeration, but if
            // its index was unreadable, fall back to the lowest id rather than
            // failing a capture over a naming problem.
            .or_else(|| all.iter().map(|&(id, _)| id).min())
            .ok_or(CaptureError::NoMonitors)
    }

    /// List monitors.
    fn monitors(&self) -> Result<Vec<MonitorInfo>> {
        let primary = Monitor::primary().ok();
        let mut out = Vec::new();
        for (id, m) in enumerate()? {
            // Prefer the friendly name ("V505-G9"); fall back to the device
            // name ("\\.\DISPLAY2") so a monitor is never nameless in the UI.
            let name = m
                .name()
                .or_else(|_| m.device_name())
                .unwrap_or_else(|_| format!("Display {id}"));
            out.push(MonitorInfo {
                id,
                name,
                width: m.width().map_err(backend_err)?,
                height: m.height().map_err(backend_err)?,
                primary: primary.is_some_and(|p| p == m),
            });
        }
        Ok(out)
    }

    /// Select a monitor, verifying it exists now rather than at the next grab.
    fn select_monitor(&mut self, id: u32) -> Result<()> {
        let known = enumerate()?.iter().any(|&(found, _)| found == id);
        if !known {
            return Err(CaptureError::UnknownMonitor(id));
        }
        if self.selected != Some(id) {
            self.selected = Some(id);
            // Drop any session for the previous monitor; the next grab opens
            // one for the new target.
            self.session = None;
        }
        Ok(())
    }

    /// Ensure a session exists for the current selection.
    fn ensure_session(&mut self) -> Result<()> {
        let want = self.target_id()?;
        if self
            .session
            .as_ref()
            .is_some_and(|s| s.monitor_id == want)
        {
            return Ok(());
        }
        self.session = Some(Session::open(want)?);
        Ok(())
    }

    /// Grab the next changed frame.
    fn grab(&mut self) -> Result<Frame> {
        // Two attempts: the second exists so a session invalidated by a
        // resolution change or a trip through the secure desktop is rebuilt
        // transparently, which is precisely when a viewer most wants a frame.
        for attempt in 0..2_u8 {
            self.ensure_session()?;
            match self.grab_once() {
                Err(CaptureError::Backend(ref msg)) if is_access_lost(msg) && attempt == 0 => {
                    warn!("DXGI duplication access lost; rebuilding session");
                    self.session = None;
                }
                other => return other,
            }
        }
        Err(CaptureError::Backend(
            "DXGI duplication access lost twice in a row".to_owned(),
        ))
    }

    /// One acquire + map, with no retry logic.
    fn grab_once(&mut self) -> Result<Frame> {
        let timeout = self.timeout_ms;
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| CaptureError::Backend("no duplication session".to_owned()))?;
        // Split the borrow: the frame borrows `dup`, the map borrows `staging`.
        let Session { dup, staging, .. } = session;

        let wait_start = Instant::now();
        let mut frame = dup.acquire_next_frame(timeout).map_err(dup_err)?;
        // Timestamp after the acquire returns: that is when the pixels became
        // available to us, which is what latency should be measured from.
        let captured_at = Instant::now();
        let wait = captured_at.saturating_duration_since(wait_start);

        // The staging texture is built from the duplication's declared mode, but
        // the texture actually handed over can differ — observed on a 4K display,
        // where it produced "Invalid staging texture: format must match the
        // frame" and killed the capture outright. Trust the frame, not the mode
        // description, and rebuild when they disagree. This costs one
        // `CreateTexture2D` on the rare frame that changes shape, and nothing
        // otherwise.
        let want = *frame.texture_desc();
        let have = staging.desc();
        if have.Width != want.Width || have.Height != want.Height || have.Format != want.Format {
            warn!(
                from_w = have.Width,
                from_h = have.Height,
                to_w = want.Width,
                to_h = want.Height,
                "rebuilding staging texture to match the acquired frame"
            );
            *staging = StagingTexture::new(frame.device(), want.Width, want.Height, want.Format)
                .map_err(backend_err)?;
        }

        let mut buffer = frame.buffer_with(staging).map_err(dup_err)?;
        let width = buffer.width();
        let height = buffer.height();
        let stride = buffer.row_pitch() as usize;
        let format = map_format(buffer.format())?;
        // One copy out of the mapped texture. The map is unwound when `frame`
        // drops, so the bytes cannot outlive it and must be owned here.
        let data = buffer.as_raw_buffer().to_vec();

        self.sequence = self.sequence.wrapping_add(1);
        self.last_wait = Some(wait);
        let frame = Frame {
            data,
            width,
            height,
            stride,
            format,
            captured_at,
            sequence: self.sequence,
        };
        frame.validate()?;
        Ok(frame)
    }
}

/// Whether a backend message describes a lost duplication.
///
/// The wrapper's `AccessLost` is flattened into [`CaptureError::Backend`] by
/// [`dup_err`], so recovery keys off the message. Matching on the variant
/// directly would mean leaking the wrapper's error type into our public API.
fn is_access_lost(msg: &str) -> bool {
    msg.contains("access lost")
}

/// Screen capturer backed by DXGI Desktop Duplication.
///
/// A `Send` handle to a dedicated capture thread. Dropping it shuts that thread
/// down and releases the Direct3D device.
pub struct DxgiCapturer {
    /// Job queue. `None` only while dropping, so the worker sees the channel
    /// close before we join it.
    jobs: Option<Sender<Job>>,
    /// The capture thread, joined on drop so Direct3D teardown completes before
    /// the process moves on.
    worker: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for DxgiCapturer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DxgiCapturer")
            .field("running", &self.jobs.is_some())
            .finish_non_exhaustive()
    }
}

impl DxgiCapturer {
    /// Start a capturer with the default acquire timeout.
    ///
    /// # Errors
    /// Returns [`CaptureError::Backend`] if the capture thread cannot be
    /// spawned. Opening the duplication is deferred to the first grab, so a
    /// missing or busy display surfaces there, not here.
    pub fn new() -> Result<Self> {
        Self::with_timeout_ms(DEFAULT_TIMEOUT_MS)
    }

    /// Start a capturer that waits `timeout_ms` for the screen to change before
    /// a grab reports [`CaptureError::NoNewFrame`].
    ///
    /// # Errors
    /// Returns [`CaptureError::Backend`] if the capture thread cannot be
    /// spawned.
    pub fn with_timeout_ms(timeout_ms: u32) -> Result<Self> {
        let (tx, rx): (Sender<Job>, Receiver<Job>) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("rr-capture-dxgi".to_owned())
            .spawn(move || {
                // The engine is built here, on the thread that will own the
                // Direct3D device for its entire life.
                let mut engine = Engine::new(timeout_ms);
                // Ends when the last sender drops, i.e. when `DxgiCapturer` is
                // dropped.
                while let Ok(job) = rx.recv() {
                    job(&mut engine);
                }
                debug!("DXGI capture thread exiting");
            })
            .map_err(|e| CaptureError::Backend(format!("cannot spawn capture thread: {e}")))?;

        Ok(Self {
            jobs: Some(tx),
            worker: Some(worker),
        })
    }

    /// Run `f` on the capture thread and wait for its result.
    fn call<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Engine) -> T + Send + 'static,
    {
        let Some(jobs) = self.jobs.as_ref() else {
            return Err(CaptureError::Backend(
                "capture thread already shut down".to_owned(),
            ));
        };
        let (reply_tx, reply_rx) = mpsc::channel();
        let job: Job = Box::new(move |engine| {
            // The receiver is gone only if the caller was cancelled; nothing to
            // report in that case.
            let _ = reply_tx.send(f(engine));
        });
        jobs.send(job).map_err(|_| {
            CaptureError::Backend("capture thread has stopped accepting work".to_owned())
        })?;
        reply_rx.recv().map_err(|_| {
            CaptureError::Backend("capture thread died without replying".to_owned())
        })
    }
}

impl Drop for DxgiCapturer {
    fn drop(&mut self) {
        // Close the queue first so the worker's `recv` returns and its loop
        // ends; only then is the join guaranteed not to block.
        drop(self.jobs.take());
        if let Some(worker) = self.worker.take() {
            // A panicked capture thread is already reported by the panic
            // handler; failing to join is not worth panicking again in a drop.
            let _ = worker.join();
        }
    }
}

impl ScreenCapturer for DxgiCapturer {
    fn monitors(&self) -> Result<Vec<MonitorInfo>> {
        self.call(|engine| engine.monitors())?
    }

    fn select_monitor(&mut self, id: u32) -> Result<()> {
        self.call(move |engine| engine.select_monitor(id))?
    }

    fn grab(&mut self) -> Result<Frame> {
        self.call(Engine::grab)?
    }

    fn last_wait(&self) -> Option<Duration> {
        // A dead worker thread means no timing to report, which is exactly what
        // `None` already means to the caller.
        self.call(|engine| engine.last_wait).ok().flatten()
    }
}
