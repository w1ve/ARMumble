//! H.264 decoder for the Client side.

use openh264::decoder::{Decoder as OpenH264Decoder, DecoderConfig};
use openh264::formats::YUVSource;
use openh264::OpenH264API;
use tracing::debug;

use crate::{CodecError, Result};

/// A decoded picture, converted to RGBA for rendering.
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    /// RGBA8 pixels, tightly packed (`width * 4` bytes per row).
    pub rgba: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl DecodedFrame {
    /// Bytes per row.
    #[must_use]
    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }

    /// Pixel at `(x, y)` as RGBA, or `None` when out of range.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = (y as usize * self.stride()) + x as usize * 4;
        let px = self.rgba.get(offset..offset + 4)?;
        Some([px[0], px[1], px[2], px[3]])
    }
}

/// H.264 decoder.
pub struct Decoder {
    inner: OpenH264Decoder,
    /// Reused RGBA scratch buffer, to avoid reallocating every frame.
    rgba: Vec<u8>,
}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder").finish_non_exhaustive()
    }
}

impl Decoder {
    /// Create a decoder.
    ///
    /// # Errors
    /// Returns [`CodecError::DecoderInit`] if `OpenH264` cannot be initialised.
    pub fn new() -> Result<Self> {
        let inner =
            OpenH264Decoder::with_api_config(OpenH264API::from_source(), DecoderConfig::new())
                .map_err(|e| CodecError::DecoderInit(e.to_string()))?;
        Ok(Self {
            inner,
            rgba: Vec::new(),
        })
    }

    /// Decode one Annex-B access unit.
    ///
    /// Returns `Ok(None)` when the packet produced no picture yet — that is
    /// normal (e.g. parameter sets only) and **must not** be treated as an
    /// error. Likewise a viewer that starts mid-stream will see errors or empty
    /// results until the first IDR arrives, so callers should keep going rather
    /// than tearing down the session on the first failure.
    ///
    /// # Errors
    /// Returns [`CodecError::Decode`] if `OpenH264` rejects the bitstream.
    pub fn decode(&mut self, packet: &[u8]) -> Result<Option<DecodedFrame>> {
        let maybe_yuv = self
            .inner
            .decode(packet)
            .map_err(|e| CodecError::Decode(e.to_string()))?;

        let Some(yuv) = maybe_yuv else {
            debug!(bytes = packet.len(), "packet produced no picture");
            return Ok(None);
        };

        let (width, height) = yuv.dimensions();
        let needed = width * height * 4;
        if self.rgba.len() != needed {
            self.rgba.resize(needed, 0);
        }
        yuv.write_rgba8(&mut self.rgba);

        Ok(Some(DecodedFrame {
            rgba: self.rgba.clone(),
            width: u32::try_from(width).unwrap_or(u32::MAX),
            height: u32::try_from(height).unwrap_or(u32::MAX),
        }))
    }
}
