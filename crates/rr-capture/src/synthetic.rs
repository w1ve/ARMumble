//! Deterministic synthetic frame source.
//!
//! Generates a moving test pattern with no display server, so the
//! capture → encode → transport pipeline can be tested on headless CI and the
//! output verified frame-by-frame. Because the pattern is a pure function of
//! the frame counter, a test can assert on exact pixel values.

use std::time::Instant;

use crate::{CaptureError, Frame, MonitorInfo, PixelFormat, Result, ScreenCapturer};

/// Synthetic monitor id reported by [`SyntheticCapturer`].
const SYNTHETIC_MONITOR_ID: u32 = 1;

/// A [`ScreenCapturer`] that renders a moving test pattern.
///
/// The pattern deliberately contains both flat regions and a sharp moving edge:
/// flat areas compress almost to nothing while the edge forces real residual
/// data, which makes encoder behaviour (and keyframe handling) observable
/// instead of degenerate.
#[derive(Debug)]
pub struct SyntheticCapturer {
    width: u32,
    height: u32,
    sequence: u64,
    /// Extra bytes appended to each row, to prove consumers honour `stride`
    /// rather than assuming `width * bpp`.
    row_padding: usize,
}

impl SyntheticCapturer {
    /// Create a source producing `width` x `height` frames.
    ///
    /// Dimensions are forced to be even because H.264 4:2:0 chroma
    /// subsampling requires it; an odd size would fail inside the encoder with
    /// a far less obvious error.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: width & !1,
            height: height & !1,
            sequence: 0,
            row_padding: 0,
        }
    }

    /// Add `bytes` of padding to every row, so tests can verify that consumers
    /// read via `stride` and not `width * bpp`.
    #[must_use]
    pub fn with_row_padding(mut self, bytes: usize) -> Self {
        self.row_padding = bytes;
        self
    }

    /// Bytes per row including padding.
    #[must_use]
    pub fn stride(&self) -> usize {
        self.width as usize * PixelFormat::Rgba8.bytes_per_pixel() + self.row_padding
    }

    /// Expected colour at `(x, y)` for frame `sequence`.
    ///
    /// Exposed so tests can assert exact pixels without duplicating the
    /// pattern logic.
    #[must_use]
    pub fn expected_pixel(width: u32, x: u32, y: u32, sequence: u64) -> [u8; 4] {
        // A vertical bar sweeps left-to-right, one pixel per frame.
        let bar_x = (sequence % u64::from(width.max(1))) as u32;
        if x == bar_x {
            return [255, 255, 255, 255];
        }
        // Background: a static gradient, so motion is confined to the bar.
        let r = (x % 256) as u8;
        let g = (y % 256) as u8;
        let b = 64;
        [r, g, b, 255]
    }
}

impl ScreenCapturer for SyntheticCapturer {
    fn monitors(&self) -> Result<Vec<MonitorInfo>> {
        Ok(vec![MonitorInfo {
            id: SYNTHETIC_MONITOR_ID,
            name: "Synthetic".to_owned(),
            width: self.width,
            height: self.height,
            primary: true,
        }])
    }

    fn select_monitor(&mut self, id: u32) -> Result<()> {
        if id == SYNTHETIC_MONITOR_ID {
            Ok(())
        } else {
            Err(CaptureError::UnknownMonitor(id))
        }
    }

    fn grab(&mut self) -> Result<Frame> {
        let stride = self.stride();
        let mut data = vec![0u8; stride * self.height as usize];

        for y in 0..self.height {
            let row_start = stride * y as usize;
            for x in 0..self.width {
                let px = Self::expected_pixel(self.width, x, y, self.sequence);
                let offset = row_start + x as usize * 4;
                // The buffer was sized from the same stride/height, so this
                // range is always valid; copy_from_slice keeps it bounds-checked.
                if let Some(dst) = data.get_mut(offset..offset + 4) {
                    dst.copy_from_slice(&px);
                }
            }
        }

        let frame = Frame {
            data,
            width: self.width,
            height: self.height,
            stride,
            format: PixelFormat::Rgba8,
            captured_at: Instant::now(),
            sequence: self.sequence,
        };
        self.sequence = self.sequence.saturating_add(1);
        Ok(frame)
    }
}
