//! `RemoteRadio` screen capture.
//!
//! The Station captures its screen and hands raw frames to the encoder. This
//! crate keeps that behind a [`ScreenCapturer`] trait for two reasons:
//!
//! - **Testability.** [`SyntheticCapturer`] produces deterministic frames with
//!   no display server, so the whole capture → encode → transport path is
//!   exercised in CI on a headless machine.
//! - **Swappable backends.** `DxgiCapturer` is the real Windows path;
//!   `XcapCapturer` is the portable fallback. Callers depend on the trait, not
//!   the backend.
//!
//! # Which backend
//!
//! | Backend | Feature | Per-frame cost at 1920x1200 | Use |
//! |---|---|---|---|
//! | `DxgiCapturer` | `dxgi-capture` | see `doc/remote-radio/P2-BASELINE.md` | Windows Station |
//! | `XcapCapturer` | `screen-capture` | 33.6 ms | portable fallback, non-Windows |
//! | `SyntheticCapturer` | — | none | tests, headless CI |
//!
//! `xcap` reads the whole framebuffer back through GDI on every call, which is
//! both slow and indifferent to whether anything changed. DXGI Desktop
//! Duplication blocks until the desktop actually changes and hands over a
//! GPU-composited surface. Both are kept: `xcap` is the only option on
//! macOS/Linux and a useful cross-check on Windows.
//!
//! # Frame metadata
//!
//! A real pipeline needs stride (DXGI rows are padded to the texture's row
//! pitch), an explicit pixel format (DXGI gives BGRA, `xcap` gives RGBA), and a
//! capture timestamp for pacing and latency measurement — so [`Frame`] carries
//! all of them rather than leaving callers to assume.

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

#[cfg(all(windows, feature = "dxgi-capture"))]
mod dxgi_backend;
mod synthetic;
#[cfg(feature = "screen-capture")]
mod xcap_backend;

#[cfg(all(windows, feature = "dxgi-capture"))]
pub use dxgi_backend::{DxgiCapturer, DEFAULT_TIMEOUT_MS};
pub use synthetic::SyntheticCapturer;
#[cfg(feature = "screen-capture")]
pub use xcap_backend::XcapCapturer;

/// Errors produced while enumerating monitors or grabbing frames.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// The platform capture API failed.
    #[error("capture backend error: {0}")]
    Backend(String),
    /// The requested monitor id does not exist.
    #[error("unknown monitor id: {0}")]
    UnknownMonitor(u32),
    /// No monitors were found at all.
    #[error("no monitors available")]
    NoMonitors,
    /// The screen did not change within the backend's wait window, so there is
    /// no new frame to deliver.
    ///
    /// **Not a failure.** Change-driven backends (DXGI Desktop Duplication)
    /// report this for a still desktop, which is the normal and desirable case:
    /// nothing moved, so nothing needs capturing, encoding or sending. Callers
    /// should treat it as "no work this tick" and loop, not as an error to
    /// surface. The `xcap` backend never produces it, because it re-reads the
    /// whole framebuffer whether or not anything changed.
    #[error("no new frame: the screen has not changed")]
    NoNewFrame,
    /// The backend produced a pixel layout with no conversion path.
    #[error("unsupported pixel format: {0}")]
    UnsupportedPixelFormat(String),
    /// The backend returned a buffer whose size disagrees with its geometry.
    #[error("inconsistent frame: expected at least {expected} bytes, got {actual}")]
    InconsistentFrame {
        /// Bytes implied by width/height/stride.
        expected: usize,
        /// Bytes actually returned.
        actual: usize,
    },
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, CaptureError>;

/// Byte layout of a captured frame.
///
/// Only the formats our backends actually produce are listed; adding one is a
/// deliberate change so the encoder's conversion path stays exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 8 bits per channel, red first, 4 bytes per pixel (`xcap`'s output).
    Rgba8,
    /// 8 bits per channel, blue first, 4 bytes per pixel (Windows-native).
    Bgra8,
}

impl PixelFormat {
    /// Bytes occupied by one pixel.
    #[must_use]
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 | Self::Bgra8 => 4,
        }
    }
}

/// A monitor available for sharing.
///
/// # Coordinate space
///
/// `x`/`y` place the monitor's top-left corner on the **virtual desktop**, in
/// physical pixels, matching `rr_proto::MonitorInfo` so the Station can advertise
/// one from the other without converting. Origins are signed: only the primary
/// monitor sits at `(0, 0)`, and displays arranged to its left or above it have
/// negative coordinates.
///
/// The Station needs these to turn the monitor-relative coordinates in a wire
/// input event into the absolute position it injects at. Enumerating a monitor
/// without its origin would leave the only options as guessing an arrangement or
/// refusing to inject, so every backend supplies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Stable identifier used to select this monitor.
    pub id: u32,
    /// Human-readable label for the UI.
    pub name: String,
    /// Virtual-desktop X of the monitor's left edge. May be negative.
    pub x: i32,
    /// Virtual-desktop Y of the monitor's top edge. May be negative.
    pub y: i32,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// Whether this is the OS primary monitor.
    pub primary: bool,
}

/// One captured frame.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Pixel data, `stride * height` bytes (may include row padding).
    pub data: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes per row, including any padding. Never assume `width * bpp`.
    pub stride: usize,
    /// Byte layout of `data`.
    pub format: PixelFormat,
    /// When the frame was grabbed, for pacing and latency measurement.
    pub captured_at: Instant,
    /// Monotonically increasing counter, so a consumer can detect drops.
    pub sequence: u64,
}

impl Frame {
    /// Validate that `data` is large enough for the declared geometry.
    ///
    /// # Errors
    /// Returns [`CaptureError::InconsistentFrame`] when the buffer is short.
    pub fn validate(&self) -> Result<()> {
        let expected = self
            .stride
            .saturating_mul(self.height as usize);
        if self.data.len() < expected {
            return Err(CaptureError::InconsistentFrame {
                expected,
                actual: self.data.len(),
            });
        }
        Ok(())
    }

    /// Row `y` without its padding, i.e. exactly `width * bpp` bytes.
    ///
    /// Returns `None` when `y` is out of range.
    #[must_use]
    pub fn row(&self, y: u32) -> Option<&[u8]> {
        if y >= self.height {
            return None;
        }
        let bpp = self.format.bytes_per_pixel();
        let start = self.stride.checked_mul(y as usize)?;
        let end = start.checked_add(self.width as usize * bpp)?;
        self.data.get(start..end)
    }

    /// Age of this frame, useful for dropping stale frames under load.
    #[must_use]
    pub fn age(&self) -> Duration {
        self.captured_at.elapsed()
    }
}

/// A source of screen frames.
///
/// Implementations are synchronous: capture APIs are blocking, so callers run
/// them on a dedicated thread or a blocking task.
pub trait ScreenCapturer: Send {
    /// List the monitors this capturer can serve.
    ///
    /// # Errors
    /// Returns a [`CaptureError`] if the platform query fails.
    fn monitors(&self) -> Result<Vec<MonitorInfo>>;

    /// Select which monitor subsequent [`ScreenCapturer::grab`] calls target.
    ///
    /// # Errors
    /// Returns [`CaptureError::UnknownMonitor`] for an unknown id.
    fn select_monitor(&mut self, id: u32) -> Result<()>;

    /// Grab the current contents of the selected monitor.
    ///
    /// # Errors
    /// Returns a [`CaptureError`] if the grab fails, or
    /// [`CaptureError::NoNewFrame`] if the backend is change-driven and nothing
    /// has changed.
    fn grab(&mut self) -> Result<Frame>;

    /// How much of the last [`ScreenCapturer::grab`] was spent *waiting* for the
    /// screen to change, rather than doing work.
    ///
    /// Change-driven backends block until the desktop moves, so timing `grab`
    /// with a stopwatch on a quiet screen measures how still the screen was, not
    /// what the capturer costs. Subtracting this leaves the part that scales
    /// with resolution and actually caps the frame rate — which is the number
    /// worth comparing between backends and worth optimising.
    ///
    /// Returns `None` for backends that never wait, where the whole `grab` is
    /// work.
    fn last_wait(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests;
