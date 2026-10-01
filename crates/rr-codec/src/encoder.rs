//! Latency-tuned H.264 encoder.

use openh264::encoder::{
    Encoder as OpenH264Encoder, EncoderConfig, RateControlMode, UsageType,
};
use openh264::formats::{RgbaSliceU8, YUVBuffer};
use openh264::OpenH264API;
use rr_capture::{Frame, PixelFormat};
use tracing::debug;

use crate::{check_geometry, CodecError, Result};

/// Encoder tuning knobs.
#[derive(Debug, Clone, Copy)]
pub struct EncoderSettings {
    /// Target bitrate in bits per second.
    pub bitrate_bps: u32,
    /// Target frame rate, used by rate control to budget each frame.
    pub max_fps: f32,
}

impl Default for EncoderSettings {
    fn default() -> Self {
        Self {
            // 8 Mbit/s is a reasonable 1080p screen-content starting point;
            // adaptive bitrate per viewer arrives with the multi-viewer phase.
            bitrate_bps: 8_000_000,
            max_fps: 30.0,
        }
    }
}

/// One encoded access unit (frame) ready for transport.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    /// H.264 Annex-B byte stream for this frame.
    pub data: Vec<u8>,
    /// True when this frame is an IDR, i.e. decodable with no prior frames.
    ///
    /// A viewer joining mid-stream must wait for one of these.
    pub is_keyframe: bool,
    /// Sequence number copied from the source [`Frame`], so a consumer can
    /// correlate encoded output with capture timing.
    pub sequence: u64,
}

/// H.264 encoder for screen content.
pub struct Encoder {
    inner: OpenH264Encoder,
    /// Reused YUV scratch buffer: allocating per frame at 30 fps would churn
    /// several hundred MB/s through the allocator for no reason.
    yuv: YUVBuffer,
    width: u32,
    height: u32,
    /// Set by [`Encoder::request_keyframe`]; cleared once honoured.
    force_keyframe: bool,
    /// True until the first frame is encoded, so the stream always opens with
    /// an IDR even if no viewer asked.
    first_frame: bool,
}

impl std::fmt::Debug for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("force_keyframe", &self.force_keyframe)
            .finish_non_exhaustive()
    }
}

impl Encoder {
    /// Create an encoder for `width` x `height` frames.
    ///
    /// # Errors
    /// Returns [`CodecError::Geometry`] for odd/zero dimensions or
    /// [`CodecError::EncoderInit`] if `OpenH264` rejects the configuration.
    pub fn new(width: u32, height: u32, settings: EncoderSettings) -> Result<Self> {
        check_geometry(width, height)?;

        let config = EncoderConfig::new()
            // Screen content: sharp text/edges and large static areas.
            .usage_type(UsageType::ScreenContentRealTime)
            // Hold a predictable bitrate; a growing send queue *is* latency.
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(openh264::encoder::BitRate::from_bps(settings.bitrate_bps))
            .max_frame_rate(openh264::encoder::FrameRate::from_hz(settings.max_fps))
            // 0 disables periodic intra frames: keyframes are on demand only,
            // which avoids a recurring bitrate spike that shows up as a stall.
            .intra_frame_period(openh264::encoder::IntraFramePeriod::from_num_frames(0));

        let inner = OpenH264Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|e| CodecError::EncoderInit(e.to_string()))?;

        Ok(Self {
            inner,
            yuv: YUVBuffer::new(width as usize, height as usize),
            width,
            height,
            force_keyframe: false,
            first_frame: true,
        })
    }

    /// Frame dimensions this encoder was built for.
    #[must_use]
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Ask for the next encoded frame to be an IDR.
    ///
    /// Called when a viewer sends `ControlMsg::RequestKeyframe` — on subscribe,
    /// or after packet loss has made its reference frames unusable.
    pub fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }

    /// Whether a keyframe is pending.
    #[must_use]
    pub fn keyframe_pending(&self) -> bool {
        self.force_keyframe || self.first_frame
    }

    /// Encode one captured frame.
    ///
    /// # Errors
    /// Returns [`CodecError`] when the frame's geometry or format does not match
    /// this encoder, or when `OpenH264` fails.
    pub fn encode(&mut self, frame: &Frame) -> Result<EncodedFrame> {
        if frame.width != self.width || frame.height != self.height {
            return Err(CodecError::Geometry {
                width: frame.width,
                height: frame.height,
                reason: "frame size does not match encoder configuration",
            });
        }
        if frame.format != PixelFormat::Rgba8 {
            // BGRA has its own OpenH264 source type; wire it up when a backend
            // that produces BGRA (e.g. the planned DXGI path) lands.
            return Err(CodecError::UnsupportedFormat(frame.format));
        }

        // `Frame` rows may be padded, but the RGBA source expects tightly
        // packed rows, so repack when the stride is not exactly width * 4.
        let packed_len = self.width as usize * 4 * self.height as usize;
        let packed: Vec<u8> = if frame.stride == self.width as usize * 4 {
            frame.data.get(..packed_len).unwrap_or(&frame.data).to_vec()
        } else {
            let mut buf = Vec::with_capacity(packed_len);
            for y in 0..frame.height {
                let row = frame.row(y).ok_or(CodecError::Geometry {
                    width: frame.width,
                    height: frame.height,
                    reason: "row index out of range while repacking",
                })?;
                buf.extend_from_slice(row);
            }
            buf
        };

        let source = RgbaSliceU8::new(&packed, (self.width as usize, self.height as usize));
        self.yuv.read_rgba8(source);

        let want_keyframe = self.force_keyframe || self.first_frame;
        if want_keyframe {
            self.inner.force_intra_frame();
        }

        let bitstream = self
            .inner
            .encode(&self.yuv)
            .map_err(|e| CodecError::Encode(e.to_string()))?;
        let data = bitstream.to_vec();

        // Only clear the request once encoding actually succeeded, so a failed
        // encode does not silently swallow a viewer's keyframe request.
        self.force_keyframe = false;
        self.first_frame = false;

        debug!(
            bytes = data.len(),
            keyframe = want_keyframe,
            sequence = frame.sequence,
            "encoded frame"
        );

        Ok(EncodedFrame {
            data,
            is_keyframe: want_keyframe,
            sequence: frame.sequence,
        })
    }
}
