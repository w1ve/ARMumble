//! Audio encoder trait and real Opus implementation.
//!
//! An [`AudioEncoder`] takes raw PCM [`AudioFrame`]s and produces
//! compressed packets ready for network transmission. The trait is
//! codec-agnostic - swap in any encoder by implementing the trait.

use crate::audio::sample::{AudioFormat, AudioFrame};
use crate::error::Result;

#[cfg(feature = "opus-codec")]
use crate::error::Error;

/// A compressed audio packet ready for network transmission.
#[derive(Debug, Clone)]
pub struct EncodedPacket {
    /// The compressed payload bytes.
    pub data: Vec<u8>,
    /// Monotonically increasing sequence number.
    pub sequence: u64,
    /// Duration of audio this packet represents, in samples.
    pub frame_samples: u32,
}

/// Encodes raw PCM frames into compressed packets.
pub trait AudioEncoder: Send + 'static {
    /// The audio format this encoder expects as input.
    fn input_format(&self) -> AudioFormat;

    /// Encode a single PCM frame into a compressed packet.
    fn encode(&mut self, frame: &AudioFrame) -> Result<EncodedPacket>;

    /// Reset encoder state (e.g. at the start of a new transmission).
    fn reset(&mut self);
}

// ---------------------------------------------------------------------------
//  Opus encoder (requires the `opus-codec` feature)
// ---------------------------------------------------------------------------

/// Opus application mode.
#[cfg(feature = "opus-codec")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpusApplication {
    /// Optimised for voice / `VoIP` (lower latency, better speech quality).
    ///
    /// Uses the SILK codec for narrowband-to-wideband speech and a
    /// SILK+CELT hybrid for super-wideband, matching the official Mumble
    /// desktop client.  Preferred for all Mumble use-cases.
    Voip,
    /// Optimised for general audio (higher quality music).
    ///
    /// Uses the CELT codec (full-band).  Not recommended for voice -
    /// wastes bits on empty high-frequency bands and can produce
    /// artifacts with narrow-bandwidth microphone input.
    Audio,
    /// Lowest possible latency (at the cost of quality).
    LowDelay,
}

#[cfg(feature = "opus-codec")]
impl From<OpusApplication> for opus::Application {
    fn from(app: OpusApplication) -> Self {
        match app {
            OpusApplication::Voip => opus::Application::Voip,
            OpusApplication::Audio => opus::Application::Audio,
            OpusApplication::LowDelay => opus::Application::LowDelay,
        }
    }
}

/// How many channels Opus should *code*, independent of the input format.
///
/// Separate from the input channel count on purpose. Opus is free to code a
/// stereo input as one channel when it judges the two to be near-identical,
/// which saves bits and is the right call for ordinary stereo material.
///
/// It is the wrong call for a transceiver's **main and sub receiver**: those are
/// two unrelated signals that merely *happen* to look alike when one of them is
/// squelched, and letting the encoder collapse them means a receiver silently
/// disappears and comes back. [`ForceChannels::Stereo`] pins it.
#[cfg(feature = "opus-codec")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ForceChannels {
    /// Let Opus choose. Matches `OPUS_AUTO`, i.e. the behaviour when this
    /// control is never set.
    #[default]
    Auto,
    /// Always code a single channel.
    Mono,
    /// Always code two channels.
    Stereo,
}

#[cfg(feature = "opus-codec")]
impl From<ForceChannels> for Option<opus::Channels> {
    fn from(f: ForceChannels) -> Self {
        match f {
            ForceChannels::Auto => None,
            ForceChannels::Mono => Some(opus::Channels::Mono),
            ForceChannels::Stereo => Some(opus::Channels::Stereo),
        }
    }
}

/// Configuration for [`OpusEncoder`].
#[cfg(feature = "opus-codec")]
#[derive(Debug, Clone)]
pub struct OpusEncoderConfig {
    /// Bit-rate in bits/s (e.g. 72 000).
    pub bitrate: i32,
    /// Frame size in samples per channel.
    ///
    /// Must be one of: 120, 240, 480, 960, 1920, 2880.
    /// 960 = 20 ms @ 48 kHz (Mumble default).
    pub frame_size: usize,
    /// Encoder application mode.
    pub application: OpusApplication,
    /// Enable variable bit-rate.
    pub vbr: bool,
    /// Encoder complexity (0-10). Higher = better quality, more CPU.
    pub complexity: i32,
    /// Enable in-band forward error correction.
    pub fec: bool,
    /// Expected packet loss percentage (0-100). Tunes FEC redundancy.
    pub packet_loss_percent: i32,
    /// Enable discontinuous transmission (saves bandwidth in silence
    /// by sending very small "comfort-noise" packets).
    ///
    /// **Disabled by default** - DTX can cause robotic-sounding artefacts
    /// when the signal is near the noise-gate threshold, because the
    /// encoder alternates between normal and comfort-noise modes.
    pub dtx: bool,
    /// Pin the coded channel count. See [`ForceChannels`].
    ///
    /// Defaults to [`ForceChannels::Auto`], which is what Opus does when the
    /// control is never set, so the default changes nothing.
    pub force_channels: ForceChannels,
    /// Disable phase inversion for intensity stereo.
    ///
    /// Opus normally uses phase inversion when coupling stereo channels, which
    /// can partially **cancel** when the stream is later downmixed to mono.
    /// That matters here more than usual: Mumble has no stereo support, so every
    /// stock client builds a mono decoder and hears a downmix of anything stereo
    /// we send. Setting this trades a little stereo quality for a downmix that
    /// does not lose level.
    ///
    /// Defaults to `false` (the Opus default), so mono senders are unaffected.
    pub phase_inversion_disabled: bool,
}

#[cfg(feature = "opus-codec")]
impl Default for OpusEncoderConfig {
    fn default() -> Self {
        let application = if cfg!(target_os = "android") {
            OpusApplication::Voip
        } else {
            OpusApplication::Audio
        };

        Self {
            bitrate: 72_000,
            frame_size: 960,
            application,
            vbr: true,
            complexity: 8,
            fec: true,
            packet_loss_percent: 3,
            dtx: false,
            // Both of these match Opus's own defaults, so a mono caller that
            // never touches them gets byte-identical behaviour to before they
            // existed.
            force_channels: ForceChannels::Auto,
            phase_inversion_disabled: false,
        }
    }
}

/// Opus encoder wrapping [`opus::Encoder`].
///
/// Accepts F32 frames at the format supplied on construction.
/// The sample rate and channel count must match the Opus encoder's
/// configuration (typically mono 48 kHz for Mumble).
#[cfg(feature = "opus-codec")]
pub struct OpusEncoder {
    config: OpusEncoderConfig,
    format: AudioFormat,
    inner: opus::Encoder,
    /// Pre-allocated output buffer (avoids per-frame allocation).
    out_buf: Vec<u8>,
    sequence: u64,
}

#[cfg(feature = "opus-codec")]
impl std::fmt::Debug for OpusEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpusEncoder")
            .field("config", &self.config)
            .field("format", &self.format)
            .field("sequence", &self.sequence)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "opus-codec")]
impl OpusEncoder {
    /// Construct a new Opus encoder.
    ///
    /// `format` describes the PCM input; its `sample_rate` and `channels`
    /// are also used to configure the underlying Opus codec.
    pub fn new(config: OpusEncoderConfig, format: AudioFormat) -> Result<Self> {
        let channels = match format.channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            n => {
                return Err(Error::InvalidState(format!(
                    "Opus only supports 1 or 2 channels, got {n}"
                )));
            }
        };

        let mut inner = opus::Encoder::new(
            format.sample_rate,
            channels,
            config.application.into(),
        )
        .map_err(|e| Error::OpusCodec(e.to_string()))?;

        Self::configure_encoder(&mut inner, &config)?;

        Ok(Self {
            config,
            format,
            inner,
            out_buf: vec![0u8; 4000],
            sequence: 0,
        })
    }

    /// Apply all quality-relevant settings to an Opus encoder instance.
    fn configure_encoder(
        enc: &mut opus::Encoder,
        config: &OpusEncoderConfig,
    ) -> Result<()> {
        enc.set_bitrate(opus::Bitrate::Bits(config.bitrate))
            .map_err(|e| Error::OpusCodec(e.to_string()))?;
        enc.set_vbr(config.vbr)
            .map_err(|e| Error::OpusCodec(e.to_string()))?;
        enc.set_complexity(config.complexity)
            .map_err(|e| Error::OpusCodec(e.to_string()))?;
        enc.set_inband_fec(config.fec)
            .map_err(|e| Error::OpusCodec(e.to_string()))?;
        enc.set_packet_loss_perc(config.packet_loss_percent)
            .map_err(|e| Error::OpusCodec(e.to_string()))?;
        enc.set_dtx(config.dtx)
            .map_err(|e| Error::OpusCodec(e.to_string()))?;
        // Applied unconditionally: `Auto` and `false` are Opus's own defaults, so
        // setting them is a no-op rather than a behaviour change, and keeping the
        // calls unconditional means a stale value cannot survive a `reset()`.
        enc.set_force_channels(config.force_channels.into())
            .map_err(|e| Error::OpusCodec(e.to_string()))?;
        enc.set_phase_inversion_disabled(config.phase_inversion_disabled)
            .map_err(|e| Error::OpusCodec(e.to_string()))?;
        Ok(())
    }
}

#[cfg(feature = "opus-codec")]
impl AudioEncoder for OpusEncoder {
    fn input_format(&self) -> AudioFormat {
        self.format
    }

    fn encode(&mut self, frame: &AudioFrame) -> Result<EncodedPacket> {
        let samples = frame.as_f32_samples();
        // Max Opus packet is ~4000 bytes per the RFC.
        let written = self
            .inner
            .encode_float(samples, &mut self.out_buf)
            .map_err(|e| Error::OpusCodec(e.to_string()))?;

        let packet = EncodedPacket {
            data: self.out_buf[..written].to_vec(),
            sequence: self.sequence,
            frame_samples: self.config.frame_size as u32,
        };
        // Mumble counts `frame_number` in 10 ms units (480 samples/channel at
        // 48 kHz), NOT in frames. A 20 ms (960-sample) frame therefore advances
        // the sequence by TWO, a 40 ms frame by four, etc. Stepping by a flat 1
        // made the receiver's jitter buffer see the timeline advance at half
        // the true rate and repad/overlap every frame — heard as echo, and on a
        // steady tone like CW sidetone the characters smear together. The
        // receive mixer already accounts in these units (SAMPLES_PER_SEQ_UNIT
        // = 480), as does the stereo-test path (SEQ_UNITS_PER_FRAME).
        const SAMPLES_PER_SEQ_UNIT: u64 = 480;
        self.sequence += (self.config.frame_size as u64 / SAMPLES_PER_SEQ_UNIT).max(1);
        Ok(packet)
    }

    fn reset(&mut self) {
        self.sequence = 0;
        // Reset by creating a fresh encoder.
        let channels = if self.format.channels == 1 {
            opus::Channels::Mono
        } else {
            opus::Channels::Stereo
        };
        if let Ok(mut fresh) = opus::Encoder::new(
            self.format.sample_rate,
            channels,
            self.config.application.into(),
        ) {
            let _ = OpusEncoder::configure_encoder(&mut fresh, &self.config);
            self.inner = fresh;
        }
    }
}

#[cfg(all(test, feature = "opus-codec"))]
mod tests {
    use super::*;
    use crate::audio::sample::AudioFormat;

    fn silent_frame(format: AudioFormat, frame_size: usize) -> AudioFrame {
        let bytes = frame_size
            * format.channels as usize
            * format.sample_format.byte_width();
        AudioFrame {
            data: vec![0u8; bytes],
            format,
            sequence: 0,
            is_silent: false,
        }
    }

    #[test]
    fn encodes_silent_frame_f32() -> Result<()> {
        let fmt = AudioFormat::MONO_48KHZ_F32;
        let config = OpusEncoderConfig::default();
        let frame_size = config.frame_size;
        let mut enc = OpusEncoder::new(config, fmt)?;
        let frame = silent_frame(fmt, frame_size);
        let packet = enc.encode(&frame)?;
        assert!(!packet.data.is_empty(), "Opus should produce at least 1 byte");
        assert_eq!(packet.sequence, 0);
        Ok(())
    }

    #[test]
    fn encodes_silent_frame_large() -> Result<()> {
        let fmt = AudioFormat::MONO_48KHZ_F32;
        let config = OpusEncoderConfig {
            frame_size: 960, // 20 ms
            ..OpusEncoderConfig::default()
        };
        let frame_size = config.frame_size;
        let mut enc = OpusEncoder::new(config, fmt)?;
        let frame = silent_frame(fmt, frame_size);
        let packet = enc.encode(&frame)?;
        assert!(!packet.data.is_empty());
        assert_eq!(packet.frame_samples, 960);
        Ok(())
    }

    #[test]
    fn sequence_increments() -> Result<()> {
        // Mumble `frame_number` is in 10 ms (480-sample) units, so a 20 ms
        // default frame advances the sequence by 2, not 1. Stepping by 1 made
        // receivers repad/overlap every frame (echo; CW smears).
        let fmt = AudioFormat::MONO_48KHZ_F32;
        let config = OpusEncoderConfig::default();
        let frame_size = config.frame_size;
        assert_eq!(frame_size, 960, "test assumes the 20 ms default frame");
        let mut enc = OpusEncoder::new(config, fmt)?;
        let frame = silent_frame(fmt, frame_size);
        let p0 = enc.encode(&frame)?;
        let p1 = enc.encode(&frame.clone())?;
        assert_eq!(p0.sequence, 0);
        assert_eq!(p1.sequence, 2);
        Ok(())
    }

    #[test]
    fn reset_clears_sequence() -> Result<()> {
        let fmt = AudioFormat::MONO_48KHZ_F32;
        let config = OpusEncoderConfig::default();
        let frame_size = config.frame_size;
        let mut enc = OpusEncoder::new(config, fmt)?;
        let frame = silent_frame(fmt, frame_size);
        let _ = enc.encode(&frame)?;
        enc.reset();
        let p = enc.encode(&frame)?;
        assert_eq!(p.sequence, 0);
        Ok(())
    }
}
