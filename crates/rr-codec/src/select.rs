//! Choosing between the hardware and software encoders.
//!
//! The Station should not care which encoder it has, only that it has one. This
//! module is the single place that decision is made, because the trade-off is
//! stark and machine-dependent:
//!
//! | | 1920x1200 | 3840x2160 |
//! |---|---|---|
//! | `OpenH264` (software) | ~17–21 ms | **56–114 ms** |
//! | Quick Sync (hardware) | a few ms | a few ms |
//!
//! At 1080p-class resolutions software is adequate. At 4K it is not close, so on a
//! machine with a hardware encoder we must use it, and on a machine without one
//! (a VM, or older hardware) we must still work.
//!
//! # Why selection filters on the hardware flag, not on speed
//!
//! Media Foundation will substitute a *software* H.264 MFT without saying so, and
//! such an MFT is still faster than `OpenH264` — fast enough to look like a
//! hardware win while delivering none of the CPU saving. [`VideoEncoder::new`]
//! therefore asks `rr-hwcodec` whether a *hardware* encoder exists and declines
//! the hardware path otherwise, rather than inferring success from throughput.

use rr_capture::Frame;
// warn! is only reached on the Windows hardware path, so it is qualified at the
// call site rather than imported and left unused everywhere else.
use tracing::info;

use crate::{EncodedFrame, Encoder, EncoderSettings, Result};

/// An encoder, hardware if the machine has one.
///
/// Construct with [`VideoEncoder::new`] and feed it frames; it behaves the same
/// either way, except that `encode` may return `None` while a hardware pipeline
/// fills.
pub enum VideoEncoder {
    /// Media Foundation driving Quick Sync, NVENC or VCN.
    #[cfg(windows)]
    Hardware(Box<rr_hwcodec::HwEncoder>),
    /// `OpenH264` on the CPU.
    Software(Box<Encoder>),
}

impl std::fmt::Debug for VideoEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoEncoder")
            .field("backend", &self.name())
            .field("hardware", &self.is_hardware())
            .finish_non_exhaustive()
    }
}

impl VideoEncoder {
    /// Pick the best encoder available for `width` x `height`.
    ///
    /// Prefers hardware, and falls back to software if there is none, if it will
    /// not initialise, or if this is not Windows. A fallback is logged at warn
    /// level because the performance consequence at high resolutions is severe
    /// enough to want in the logs.
    ///
    /// # Errors
    /// Returns [`crate::CodecError`] only if *both* paths fail; a hardware failure
    /// alone is absorbed into the fallback.
    pub fn new(width: u32, height: u32, settings: EncoderSettings) -> Result<Self> {
        #[cfg(windows)]
        {
            match Self::try_hardware(width, height, settings) {
                Ok(encoder) => return Ok(encoder),
                Err(reason) => tracing::warn!(
                    reason,
                    "no hardware encoder; falling back to software, which is much \
                     slower above 1080p"
                ),
            }
        }
        Self::new_software(width, height, settings)
    }

    /// Build the software encoder regardless of what hardware exists.
    ///
    /// Useful for comparison and for forcing a known-good path.
    ///
    /// # Errors
    /// Returns [`crate::CodecError`] if `OpenH264` rejects the geometry or
    /// settings.
    pub fn new_software(width: u32, height: u32, settings: EncoderSettings) -> Result<Self> {
        let encoder = Encoder::new(width, height, settings)?;
        info!(width, height, "using software OpenH264 encoder");
        Ok(Self::Software(Box::new(encoder)))
    }

    /// Try the hardware path, reporting why it was unavailable rather than
    /// returning a codec error — the caller's next move is to fall back, not to
    /// fail.
    #[cfg(windows)]
    fn try_hardware(
        width: u32,
        height: u32,
        settings: EncoderSettings,
    ) -> std::result::Result<Self, String> {
        // Ask for a *hardware* encoder specifically. See the module docs.
        match rr_hwcodec::has_hardware_h264() {
            Ok(true) => {}
            Ok(false) => return Err("no hardware H.264 encoder on this system".to_owned()),
            Err(e) => return Err(format!("could not enumerate encoders: {e}")),
        }

        let hw_settings = rr_hwcodec::HwEncoderSettings {
            bitrate_bps: settings.bitrate_bps,
            // Rate control needs a whole number of frames per second; the software
            // encoder takes a float, so round rather than truncate.
            fps: settings.max_fps.round().max(1.0) as u32,
            prefer_bgra: true,
        };
        let encoder = rr_hwcodec::HwEncoder::new(width, height, hw_settings)
            .map_err(|e| format!("hardware encoder would not start: {e}"))?;
        info!(
            encoder = %encoder.info().name,
            layout = ?encoder.input_layout(),
            is_async = encoder.is_async(),
            "using hardware H.264 encoder"
        );
        Ok(Self::Hardware(Box::new(encoder)))
    }

    /// Whether the selected encoder runs on dedicated video hardware.
    #[must_use]
    pub fn is_hardware(&self) -> bool {
        match self {
            #[cfg(windows)]
            Self::Hardware(_) => true,
            Self::Software(_) => false,
        }
    }

    /// A human-readable name for logs and diagnostics.
    #[must_use]
    pub fn name(&self) -> String {
        match self {
            #[cfg(windows)]
            Self::Hardware(e) => e.info().name.clone(),
            Self::Software(_) => "OpenH264 (software)".to_owned(),
        }
    }

    /// Frame dimensions this encoder was built for.
    #[must_use]
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            #[cfg(windows)]
            Self::Hardware(e) => e.dimensions(),
            Self::Software(e) => e.dimensions(),
        }
    }

    /// Ask for the next encoded frame to be an IDR.
    pub fn request_keyframe(&mut self) {
        match self {
            #[cfg(windows)]
            Self::Hardware(e) => e.request_keyframe(),
            Self::Software(e) => e.request_keyframe(),
        }
    }

    /// Encode one captured frame.
    ///
    /// Returns `None` when a hardware pipeline has accepted the frame but has no
    /// output yet; the result arrives from a later call. The software encoder
    /// always returns `Some`.
    ///
    /// # Errors
    /// Returns [`crate::CodecError`] if the frame does not match the encoder or
    /// the encoder fails.
    pub fn encode(&mut self, frame: &Frame) -> Result<Option<EncodedFrame>> {
        match self {
            #[cfg(windows)]
            Self::Hardware(e) => {
                let produced = e
                    .encode(frame)
                    .map_err(|err| crate::CodecError::Encode(err.to_string()))?;
                Ok(produced.map(|hw| EncodedFrame {
                    data: hw.data,
                    is_keyframe: hw.is_keyframe,
                    sequence: hw.sequence,
                }))
            }
            Self::Software(e) => e.encode(frame).map(Some),
        }
    }

    /// Drain anything the encoder is still holding.
    ///
    /// The software encoder buffers nothing and returns an empty vector.
    ///
    /// # Errors
    /// Returns [`crate::CodecError`] if the drain fails.
    pub fn flush(&mut self) -> Result<Vec<EncodedFrame>> {
        match self {
            #[cfg(windows)]
            Self::Hardware(e) => {
                let frames = e
                    .flush()
                    .map_err(|err| crate::CodecError::Encode(err.to_string()))?;
                Ok(frames
                    .into_iter()
                    .map(|hw| EncodedFrame {
                        data: hw.data,
                        is_keyframe: hw.is_keyframe,
                        sequence: hw.sequence,
                    })
                    .collect())
            }
            Self::Software(_) => Ok(Vec::new()),
        }
    }
}
