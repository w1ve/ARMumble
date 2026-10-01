//! Real screen capture via the `xcap` crate.
//!
//! This is the portable baseline backend (Windows, macOS, Linux/X11). It is
//! feature-gated (`screen-capture`) so headless CI and the Client — which never
//! captures — do not need a display server or the `xcap` dependency.
//!
//! Known limitations, inherited from `xcap`:
//! - Linux/Wayland is not fully supported.
//! - `capture_image` performs a full-surface copy each call, so it is the
//!   correctness baseline rather than the lowest-latency path. A Windows
//!   DXGI/`WGC` backend with damage regions is planned for the
//!   latency-optimisation phase; it will implement the same
//!   [`ScreenCapturer`](crate::ScreenCapturer) trait.

use std::time::Instant;

use tracing::debug;
use xcap::Monitor;

use crate::{CaptureError, Frame, MonitorInfo, PixelFormat, Result, ScreenCapturer};

/// Translate an `xcap` error into our own error type.
fn backend_err(e: impl std::fmt::Display) -> CaptureError {
    CaptureError::Backend(e.to_string())
}

/// Screen capturer backed by `xcap`.
#[derive(Debug, Default)]
pub struct XcapCapturer {
    /// Monitor id selected for capture; `None` means "primary at first grab".
    selected: Option<u32>,
}

impl XcapCapturer {
    /// Create a capturer that will target the primary monitor until
    /// [`ScreenCapturer::select_monitor`] says otherwise.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve the `xcap` monitor matching the current selection.
    fn resolve(&self) -> Result<Monitor> {
        let monitors = Monitor::all().map_err(backend_err)?;
        if monitors.is_empty() {
            return Err(CaptureError::NoMonitors);
        }

        match self.selected {
            Some(want) => {
                for m in monitors {
                    // `id()` is fallible; skip monitors we cannot identify
                    // rather than aborting the whole capture.
                    if m.id().map_err(backend_err)? == want {
                        return Ok(m);
                    }
                }
                Err(CaptureError::UnknownMonitor(want))
            }
            None => {
                for m in &monitors {
                    if m.is_primary().unwrap_or(false) {
                        return Ok(m.clone());
                    }
                }
                // No monitor claims to be primary: fall back to the first.
                monitors.into_iter().next().ok_or(CaptureError::NoMonitors)
            }
        }
    }
}

impl ScreenCapturer for XcapCapturer {
    fn monitors(&self) -> Result<Vec<MonitorInfo>> {
        let monitors = Monitor::all().map_err(backend_err)?;
        let mut out = Vec::with_capacity(monitors.len());
        for m in monitors {
            out.push(MonitorInfo {
                id: m.id().map_err(backend_err)?,
                name: m.friendly_name().map_err(backend_err)?,
                width: m.width().map_err(backend_err)?,
                height: m.height().map_err(backend_err)?,
                primary: m.is_primary().map_err(backend_err)?,
            });
        }
        if out.is_empty() {
            return Err(CaptureError::NoMonitors);
        }
        Ok(out)
    }

    fn select_monitor(&mut self, id: u32) -> Result<()> {
        // Verify the id exists now, so a bad selection fails here rather than
        // on every later grab.
        let monitors = Monitor::all().map_err(backend_err)?;
        for m in monitors {
            if m.id().map_err(backend_err)? == id {
                self.selected = Some(id);
                return Ok(());
            }
        }
        Err(CaptureError::UnknownMonitor(id))
    }

    fn grab(&mut self) -> Result<Frame> {
        let monitor = self.resolve()?;
        let sequence_source = Instant::now();

        // `capture_image` yields an RGBA8 image with tightly packed rows.
        let image = monitor.capture_image().map_err(backend_err)?;
        let width = image.width();
        let height = image.height();
        let data = image.into_raw();
        let stride = width as usize * PixelFormat::Rgba8.bytes_per_pixel();

        debug!(width, height, stride, "captured frame");

        let frame = Frame {
            data,
            width,
            height,
            stride,
            format: PixelFormat::Rgba8,
            captured_at: sequence_source,
            // xcap exposes no frame counter; the caller's pipeline assigns
            // sequence numbers when it needs drop detection.
            sequence: 0,
        };
        frame.validate()?;
        Ok(frame)
    }
}
