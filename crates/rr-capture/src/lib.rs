//! `RemoteRadio` screen capture.
//!
//! The Station captures its screen and hands raw frames to the encoder. This
//! crate keeps that behind a [`ScreenCapturer`] trait for two reasons:
//!
//! - **Testability.** [`SyntheticCapturer`] produces deterministic frames with
//!   no display server, so the whole capture → encode → transport path is
//!   exercised in CI on a headless machine.
//! - **Swappable backends.** `xcap` is the portable baseline; a Windows
//!   DXGI/`WGC` fast path is planned for the latency-optimisation phase. Callers
//!   depend on the trait, not the backend.
//!
//! # Frame metadata
//!
//! `xcap` hands back only width, height and pixel bytes. A real pipeline needs
//! stride (rows can be padded), an explicit pixel format, and a capture
//! timestamp for pacing and latency measurement — so [`Frame`] carries all of
//! them rather than leaving callers to assume.

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

mod synthetic;
#[cfg(feature = "screen-capture")]
mod xcap_backend;

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Stable identifier used to select this monitor.
    pub id: u32,
    /// Human-readable label for the UI.
    pub name: String,
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
    /// Returns a [`CaptureError`] if the grab fails.
    fn grab(&mut self) -> Result<Frame>;
}

#[cfg(test)]
mod tests;
