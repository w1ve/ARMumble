//! Thin, fidelity-first wrappers around libopus.

use std::fmt;

use opus::{Application, Bandwidth, Bitrate, Channels, Decoder, Encoder, Signal};

use crate::error::{AudioError, Result};

/// Opus sample rate used on the wire.
pub const SAMPLE_RATE: u32 = 48_000;

/// Largest packet we will ever produce or accept (bytes).
const MAX_PACKET: usize = 4000;
/// Largest frame the decoder can emit (120 ms) in sample frames.
const MAX_DECODE_FRAMES: usize = 5760;

/// Encoder parameters that the user or the bandwidth planner may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderSettings {
    /// Target bit rate in bits per second (payload only).
    pub bitrate_bps: u32,
    /// Frame duration in milliseconds: 10, 20, 40 or 60.
    pub frame_ms: u32,
    /// Encode two channels (true) or a mono mix (false).
    pub stereo: bool,
}

impl EncoderSettings {
    /// Sample frames per packet.
    #[must_use]
    pub fn frame_samples(&self) -> usize {
        (self.frame_ms * SAMPLE_RATE / 1000) as usize
    }

    /// Interleaved channel count of the PCM handed to the encoder.
    #[must_use]
    pub fn channels(&self) -> usize {
        if self.stereo {
            2
        } else {
            1
        }
    }
}

/// Opus encoder tuned for clean music-grade speech: audio mode, constrained
/// VBR, full complexity, no FEC, no DTX, no packet-loss assumption.
pub struct FrameEncoder {
    enc: Encoder,
    settings: EncoderSettings,
    out: Vec<u8>,
}

impl fmt::Debug for FrameEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameEncoder")
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl FrameEncoder {
    /// Create an encoder.
    pub fn new(settings: EncoderSettings) -> Result<Self> {
        if !matches!(settings.frame_ms, 10 | 20 | 40 | 60) {
            return Err(AudioError::InvalidConfig(format!(
                "frame_ms {} (must be 10, 20, 40 or 60)",
                settings.frame_ms
            )));
        }
        let ch = if settings.stereo { Channels::Stereo } else { Channels::Mono };
        let mut enc = Encoder::new(SAMPLE_RATE, ch, Application::Audio)?;
        enc.set_bitrate(Bitrate::Bits(settings.bitrate_bps.clamp(6_000, 510_000) as i32))?;
        enc.set_vbr(true)?;
        enc.set_vbr_constraint(true)?;
        enc.set_complexity(10)?;
        enc.set_inband_fec(false)?;
        enc.set_dtx(false)?;
        enc.set_packet_loss_perc(0)?;
        enc.set_signal(Signal::Music)?;
        enc.set_max_bandwidth(Bandwidth::Fullband)?;
        enc.set_lsb_depth(24)?;
        if settings.stereo {
            enc.set_force_channels(Some(Channels::Stereo))?;
            enc.set_phase_inversion_disabled(true)?;
        }
        Ok(Self { enc, settings, out: vec![0; MAX_PACKET] })
    }

    /// Current settings.
    #[must_use]
    pub fn settings(&self) -> EncoderSettings {
        self.settings
    }

    /// Clear all prediction state (start of a talk-spurt).
    pub fn reset(&mut self) -> Result<()> {
        self.enc.reset_state()?;
        Ok(())
    }

    /// Encode exactly one frame of interleaved PCM.
    pub fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>> {
        let want = self.settings.frame_samples() * self.settings.channels();
        if pcm.len() != want {
            return Err(AudioError::InvalidConfig(format!(
                "frame has {} samples, expected {want}",
                pcm.len()
            )));
        }
        let n = self.enc.encode_float(pcm, &mut self.out)?;
        Ok(self.out[..n].to_vec())
    }
}

/// Stereo Opus decoder. Mono packets are up-mixed to identical L/R by libopus.
pub struct FrameDecoder {
    dec: Decoder,
    scratch: Vec<f32>,
}

impl fmt::Debug for FrameDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameDecoder").finish_non_exhaustive()
    }
}

impl FrameDecoder {
    /// Create a decoder.
    pub fn new() -> Result<Self> {
        Ok(Self {
            dec: Decoder::new(SAMPLE_RATE, Channels::Stereo)?,
            scratch: vec![0.0; MAX_DECODE_FRAMES * 2],
        })
    }

    /// Forget all state.
    pub fn reset(&mut self) -> Result<()> {
        self.dec.reset_state()?;
        Ok(())
    }

    /// Number of sample frames in `packet`.
    pub fn packet_frames(packet: &[u8]) -> Result<usize> {
        Ok(opus::packet::get_nb_samples(packet, SAMPLE_RATE)?)
    }

    /// Decode a packet, appending interleaved stereo to `out`. Returns the
    /// number of sample frames appended.
    pub fn decode(&mut self, packet: &[u8], out: &mut Vec<f32>) -> Result<usize> {
        let n = self.dec.decode_float(packet, &mut self.scratch, false)?;
        out.extend_from_slice(&self.scratch[..n * 2]);
        Ok(n)
    }

    /// Conceal one lost frame of `frames` sample frames, appending to `out`.
    pub fn conceal(&mut self, frames: usize, out: &mut Vec<f32>) -> Result<usize> {
        let frames = frames.min(MAX_DECODE_FRAMES);
        let n = self.dec.decode_float(&[], &mut self.scratch[..frames * 2], false)?;
        out.extend_from_slice(&self.scratch[..n * 2]);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_stereo_frame() {
        let s = EncoderSettings { bitrate_bps: 128_000, frame_ms: 20, stereo: true };
        let mut e = FrameEncoder::new(s).expect("enc");
        let mut d = FrameDecoder::new().expect("dec");
        let pcm: Vec<f32> = (0..960 * 2)
            .map(|i| 0.3 * (i as f32 * 0.05).sin())
            .collect();
        let pkt = e.encode(&pcm).expect("encode");
        assert_eq!(FrameDecoder::packet_frames(&pkt).expect("frames"), 960);
        let mut out = Vec::new();
        assert_eq!(d.decode(&pkt, &mut out).expect("decode"), 960);
        assert_eq!(out.len(), 1920);
    }

    #[test]
    fn rejects_bad_frame_size() {
        let s = EncoderSettings { bitrate_bps: 64_000, frame_ms: 25, stereo: true };
        assert!(FrameEncoder::new(s).is_err());
    }
}
