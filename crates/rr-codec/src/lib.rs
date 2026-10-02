//! `RemoteRadio` video codec: latency-tuned H.264 for screen content.
//!
//! The Station encodes captured frames and the Client decodes them. Everything
//! here is tuned for **interactivity, not file size** — a remote desktop is
//! unusable if frames arrive smoothly but half a second late.
//!
//! # Latency choices
//!
//! | Setting | Value | Why |
//! |---|---|---|
//! | `usage_type` | `ScreenContentRealTime` | Tells `OpenH264` this is screen content (sharp edges, large flat areas), not camera video. |
//! | `intra_frame_period` | `0` | No periodic keyframes. Keyframes cost a large bitrate spike; we send them only [`Encoder::request_keyframe`] when a viewer actually needs one. |
//! | `rate_control_mode` | `Bitrate` | Hold a predictable bitrate so the link does not build a queue, which is what turns into latency. |
//! | `max_frame_rate` | caller's target | Lets rate control budget per frame correctly. |
//! | B-frames | none | `OpenH264` emits no B-frames in realtime usage; they would add reordering delay by design. |
//!
//! # Keyframe-on-demand
//!
//! With periodic keyframes disabled, a late-joining viewer would otherwise wait
//! forever for a decodable picture. The control channel carries
//! `ControlMsg::RequestKeyframe`, and the Station answers by calling
//! [`Encoder::request_keyframe`] so the next encode is an IDR.

#![forbid(unsafe_code)]

mod decoder;
mod encoder;
mod select;

pub use decoder::{DecodedFrame, Decoder};
pub use encoder::{EncodedFrame, Encoder, EncoderSettings};
pub use select::VideoEncoder;

/// Errors from encoding or decoding.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// The encoder could not be created with the requested settings.
    #[error("encoder init failed: {0}")]
    EncoderInit(String),
    /// Encoding a frame failed.
    #[error("encode failed: {0}")]
    Encode(String),
    /// The decoder could not be created.
    #[error("decoder init failed: {0}")]
    DecoderInit(String),
    /// Decoding a bitstream failed.
    #[error("decode failed: {0}")]
    Decode(String),
    /// A frame's geometry is unusable (e.g. odd dimensions for 4:2:0).
    #[error("unsupported frame geometry: {width}x{height} ({reason})")]
    Geometry {
        /// Frame width.
        width: u32,
        /// Frame height.
        height: u32,
        /// What specifically is wrong.
        reason: &'static str,
    },
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, CodecError>;

/// Validate that a frame can be encoded as H.264 4:2:0.
///
/// # Errors
/// Returns [`CodecError::Geometry`] for zero or odd dimensions.
pub(crate) fn check_geometry(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 {
        return Err(CodecError::Geometry {
            width,
            height,
            reason: "dimensions must be non-zero",
        });
    }
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(CodecError::Geometry {
            width,
            height,
            reason: "4:2:0 chroma subsampling requires even dimensions",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
