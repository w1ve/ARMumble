//! `RemoteRadio` hardware video encoding.
//!
//! Software H.264 cannot carry a 4K desktop. Measured on a 4-core mobile i7,
//! `OpenH264` costs 56–114 ms per 3840x2160 frame against a 33.3 ms budget for
//! 30 fps, and no encoder setting moves that by more than 6% (see
//! `doc/remote-radio/P2-BASELINE.md`). The same machine's Quick Sync encoder
//! does 4K at 70–85 fps. The gap is not tuning, it is silicon.
//!
//! This crate reaches that silicon through the **Windows Media Foundation H.264
//! encoder MFT**. One API covers Intel Quick Sync, NVIDIA NVENC and AMD VCN, so
//! a single code path serves whatever hardware a Station happens to have, with
//! `rr-codec`'s `OpenH264` encoder as the fallback where there is none.
//!
//! # Why this crate is allowed `unsafe`, and what keeps that contained
//!
//! The workspace sets `unsafe_code = "deny"`. Media Foundation is Win32 COM:
//! there is no safe path to it, and the alternative — shelling out to a tool or
//! vendoring a C++ library — trades a reviewable exception for a much larger
//! unreviewable one.
//!
//! So the exception lives here, in one crate, deliberately:
//!
//! - **This is the only `RemoteRadio` (`rr-*`) crate that may use `unsafe`.** If a
//!   second one needs it, that is a conversation, not a copy-paste; CI enforces it.
//! - **It is the only crate anywhere in the workspace that takes the exception
//!   crate-wide.** The inherited `FancyMumble` crates (`mumble-tauri`,
//!   `mumble-protocol`) carry ten pre-existing *per-item* `#[allow(unsafe_code)]`
//!   attributes for platform FFI — WASAPI, `oboe`, `glib`, `WebView2` COM, byte
//!   reinterpretation and the `signal-bridge` FFI. Those are narrower than this
//!   one and out of scope here, but the workspace is not `unsafe`-free and it would
//!   be wrong to imply otherwise.
//! - **Nothing `unsafe` escapes.** Every public item takes and returns ordinary
//!   Rust values. No COM interface, raw pointer or `windows` type appears in the
//!   public API, so callers cannot misuse what they cannot see.
//! - **Every `unsafe` block carries a comment stating the invariant** that makes
//!   it sound — what must be true about pointers, lengths and lifetimes, and why
//!   it is true here.
//! - **COM resources are owned by RAII types.** Nothing relies on a caller
//!   remembering to release, unlock or free.
//! - **The conversion code uses no `unsafe` at all** ([`convert`]), despite living
//!   in the crate that is allowed it. Being permitted `unsafe` is not a reason to
//!   use it.
//! - The crate is Windows-only. On other targets it compiles to a stub that
//!   reports no hardware encoder, so callers need no `cfg` of their own.
//!
//! # What this crate does not do
//!
//! It does not pick an encoder for the Station; `rr-codec`'s `VideoEncoder` does
//! that, because it owns the software fallback.
//!
//! And it does not yet take frames directly from the GPU. Frames arrive as CPU
//! bytes, and staging them into a Media Foundation buffer — a 33 MiB copy at 4K —
//! is **66% of the remaining encode time**. Feeding the capture texture straight to
//! the encoder would remove that *and* the capture readback above it. That is the
//! next optimisation, and it is now the only large one left.
//!
//! # Measured result
//!
//! On an i7-1165G7 with Iris Xe, capturing its 3840x2160 display with DXGI Desktop
//! Duplication and encoding through this crate:
//!
//! | 3840x2160 | capture | encode | total | fps |
//! |---|---|---|---|---|
//! | **hardware (this crate)** | 14.27 ms | **10.01 ms** | 24.27 ms | **41.2** |
//! | software (`OpenH264`) | 14.81 ms | 61.41 ms | 76.22 ms | 13.1 |
//!
//! 6.1x faster on encode, and it did it with the CPU at **45% of its nominal
//! clock** against 109–123% for the software run — the work moved off the cores
//! entirely, which matters for a laptop Station doing anything else at the same
//! time. Output is verified decodable by `rr-codec`'s `OpenH264` decoder, not just
//! assumed from its size.
//!
//! # Three things the MFT contract requires that are easy to get wrong
//!
//! Each of these produced a failure that looked like something else entirely:
//!
//! 1. **Set the output type, negotiate input, then set the output type again.**
//!    Agreeing an input type invalidates the output type, and Quick Sync then
//!    raises `MF_E_TRANSFORM_STREAM_CHANGE` on the first `ProcessOutput`.
//! 2. **When renegotiating, overlay frame size, rate and bitrate onto the MFT's
//!    preferred type.** Handing back its bare preference leaves it in a state where
//!    the next call fails with an undiagnosable `E_UNEXPECTED`.
//! 3. **Draining is event-driven too.** Polling `ProcessOutput` after
//!    `MFT_MESSAGE_COMMAND_DRAIN` is an out-of-turn call on an async MFT and fails
//!    the flush with `E_UNEXPECTED` — after every frame had encoded perfectly well.
//!
//! And one performance trap: [`HwEncoder::encode`] must **not** wait for the output
//! of the frame it just submitted. Doing so serialises a pipelined encoder and cost
//! 41.9 ms a frame at 4K against 10 ms of real throughput.

// The one permitted exception in the workspace. See the module docs above for
// the rules that keep it contained.
#![allow(
    unsafe_code,
    reason = "Media Foundation is Win32 COM and has no safe binding; this is the \
              single crate in the workspace permitted to use unsafe, and no unsafe \
              construct appears in its public API"
)]

mod convert;
#[cfg(windows)]
mod encoder;
#[cfg(windows)]
mod mf;

pub use convert::{bgra_to_nv12, nv12_len, rgba_to_nv12};
#[cfg(windows)]
pub use encoder::{EncodeStats, HwEncoder, HwEncoderSettings};

/// Errors from hardware encoding.
#[derive(Debug, thiserror::Error)]
pub enum HwCodecError {
    /// Media Foundation itself could not be started.
    #[error("Media Foundation startup failed: {0}")]
    Startup(String),
    /// No encoder matching the request was found.
    #[error("no {0} encoder available on this system")]
    NoEncoder(&'static str),
    /// The encoder rejected the requested configuration.
    #[error("encoder configuration rejected: {0}")]
    Configuration(String),
    /// A Media Foundation call failed.
    #[error("Media Foundation error in {operation}: {source_message}")]
    Mf {
        /// Which step failed, so the message is actionable.
        operation: &'static str,
        /// The underlying Windows error.
        source_message: String,
    },
    /// The frame's geometry or format cannot be encoded.
    #[error("unsupported frame: {0}")]
    UnsupportedFrame(String),
    /// This platform has no hardware encoder support in this crate.
    #[error("hardware encoding is only implemented on Windows")]
    UnsupportedPlatform,
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, HwCodecError>;

/// Which pixel layout the encoder accepts, after negotiation.
///
/// Worth knowing because it decides whether we convert colour on the CPU. An
/// MFT that takes BGRA directly converts on the GPU for free; one that insists on
/// NV12 makes us pay for the conversion ourselves, which at 4K is not cheap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputLayout {
    /// The MFT accepts 32-bit BGRA directly, so no conversion is needed.
    Bgra32,
    /// The MFT requires NV12, so frames are converted before submission.
    Nv12,
}

impl InputLayout {
    /// Whether using this layout costs a CPU colour conversion per frame.
    #[must_use]
    pub const fn needs_cpu_conversion(self) -> bool {
        match self {
            Self::Bgra32 => false,
            Self::Nv12 => true,
        }
    }
}

/// One encoded access unit, as an Annex-B H.264 byte stream.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    /// H.264 Annex-B bytes for this frame.
    pub data: Vec<u8>,
    /// True when this frame is an IDR, i.e. decodable with no prior frames.
    pub is_keyframe: bool,
    /// Sequence number copied from the source frame.
    pub sequence: u64,
}

/// A video encoder the system can offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderInfo {
    /// Friendly name reported by the driver, e.g. `Intel® Quick Sync Video H.264
    /// Encoder MFT`.
    pub name: String,
    /// Whether Media Foundation classifies it as a hardware MFT.
    ///
    /// This matters more than it looks: Media Foundation will happily substitute
    /// a *software* encoder without saying so, and a software MFT that is merely
    /// faster than `OpenH264` is easy to mistake for a hardware win. Selection
    /// filters on this rather than trusting throughput.
    pub hardware: bool,
}

/// List the H.264 encoders Media Foundation can offer, hardware first.
///
/// # Errors
/// Returns [`HwCodecError`] if Media Foundation cannot be started or the
/// enumeration call fails. An empty list is a successful result, not an error:
/// plenty of machines have no hardware encoder.
#[cfg(windows)]
pub fn available_h264_encoders() -> Result<Vec<EncoderInfo>> {
    mf::enumerate_h264_encoders()
}

/// Non-Windows stub: there is no Media Foundation, so there are no encoders.
#[cfg(not(windows))]
pub fn available_h264_encoders() -> Result<Vec<EncoderInfo>> {
    Ok(Vec::new())
}

/// Whether this system has a hardware H.264 encoder.
///
/// Convenience over [`available_h264_encoders`] for callers that only need to
/// decide between the hardware and software paths.
///
/// # Errors
/// Returns [`HwCodecError`] if the enumeration itself fails.
pub fn has_hardware_h264() -> Result<bool> {
    Ok(available_h264_encoders()?.iter().any(|e| e.hardware))
}

#[cfg(test)]
mod tests;
