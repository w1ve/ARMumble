//! Microphone / line-in capture: channel mapping, rate conversion, user gain,
//! transmit gating and Opus encoding.
//!
//! The engine is a pure function of its input: feed it device samples with
//! [`CaptureEngine::process`] and collect [`EncodedPacket`]s. It never touches
//! the signal beyond user gain and (if the device rate is not 48 kHz) a
//! high-quality sample-rate conversion. Gating only decides *whether* a frame is
//! sent, never what is in it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use crate::asrc::Resampler;
use crate::codec::{EncoderSettings, FrameEncoder, SAMPLE_RATE};
use crate::error::Result;
use crate::util::AtomicF32;

/// Highest user input gain. 1.0 (100 %) is unity and the default.
pub const MAX_INPUT_GAIN: f32 = 2.0;
/// Audio kept ahead of a VOX opening so the first syllable is not clipped.
const PREROLL_MS: u32 = 60;

/// One packet ready for the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedPacket {
    /// Sequence number in 10 ms units.
    pub seq: u64,
    /// Opus payload (empty for a terminator).
    pub data: Vec<u8>,
    /// True when this packet ends the talk-spurt.
    pub terminator: bool,
}

/// How the two device channels become the stereo pair that is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMap {
    /// Device channel 1 → left, channel 2 → right (a mono device is duplicated).
    Stereo,
    /// Device channel 1 to both sides.
    Left,
    /// Device channel 2 to both sides.
    Right,
    /// Average of channels 1 and 2 to both sides.
    Mix,
}

impl ChannelMap {
    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Left,
            2 => Self::Right,
            3 => Self::Mix,
            _ => Self::Stereo,
        }
    }
}

/// When the engine transmits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxMode {
    /// Always transmit (an always-open station feed).
    Continuous,
    /// Transmit while the input level is above the threshold, plus a hang time.
    Vox,
    /// Transmit while the PTT flag is held.
    Ptt,
}

impl TxMode {
    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Vox,
            2 => Self::Ptt,
            _ => Self::Continuous,
        }
    }
}

/// Live-adjustable capture controls, shared with the UI. All reads are
/// lock-free so the audio thread never blocks.
#[derive(Debug)]
pub struct CaptureControl {
    gain: AtomicF32,
    channel_map: AtomicU8,
    tx_mode: AtomicU8,
    ptt: AtomicBool,
    vox_threshold_db: AtomicF32,
    vox_hang_ms: AtomicU32,
    bitrate_bps: AtomicU32,
    frame_ms: AtomicU32,
    stereo: AtomicBool,
    peak_l: AtomicF32,
    peak_r: AtomicF32,
    clips: AtomicU64,
    transmitting: AtomicBool,
}

impl CaptureControl {
    fn new(cfg: &CaptureConfig) -> Self {
        Self {
            gain: AtomicF32::new(cfg.gain.clamp(0.0, MAX_INPUT_GAIN)),
            channel_map: AtomicU8::new(cfg.channel_map.to_u8()),
            tx_mode: AtomicU8::new(cfg.tx_mode.to_u8()),
            ptt: AtomicBool::new(false),
            vox_threshold_db: AtomicF32::new(cfg.vox_threshold_db),
            vox_hang_ms: AtomicU32::new(cfg.vox_hang_ms),
            bitrate_bps: AtomicU32::new(cfg.encoder.bitrate_bps),
            frame_ms: AtomicU32::new(cfg.encoder.frame_ms),
            stereo: AtomicBool::new(cfg.encoder.stereo),
            peak_l: AtomicF32::new(0.0),
            peak_r: AtomicF32::new(0.0),
            clips: AtomicU64::new(0),
            transmitting: AtomicBool::new(false),
        }
    }

    /// Set the input gain (linear, 0.0 to [`MAX_INPUT_GAIN`]).
    pub fn set_gain(&self, gain: f32) {
        self.gain.store(gain.clamp(0.0, MAX_INPUT_GAIN));
    }

    /// Current input gain.
    #[must_use]
    pub fn gain(&self) -> f32 {
        self.gain.load()
    }

    /// Choose how device channels map to left/right.
    pub fn set_channel_map(&self, map: ChannelMap) {
        self.channel_map.store(map.to_u8(), Ordering::Relaxed);
    }

    /// Choose the transmit mode.
    pub fn set_tx_mode(&self, mode: TxMode) {
        self.tx_mode.store(mode.to_u8(), Ordering::Relaxed);
    }

    /// Press or release the PTT flag (used in [`TxMode::Ptt`]).
    pub fn set_ptt(&self, down: bool) {
        self.ptt.store(down, Ordering::Relaxed);
    }

    /// Set the VOX threshold in dBFS (peak) and hang time in milliseconds.
    pub fn set_vox(&self, threshold_db: f32, hang_ms: u32) {
        self.vox_threshold_db.store(threshold_db);
        self.vox_hang_ms.store(hang_ms, Ordering::Relaxed);
    }

    /// Apply a new bit rate / frame size / channel count.
    pub fn set_encoder(&self, s: EncoderSettings) {
        self.bitrate_bps.store(s.bitrate_bps, Ordering::Relaxed);
        self.frame_ms.store(s.frame_ms, Ordering::Relaxed);
        self.stereo.store(s.stereo, Ordering::Relaxed);
    }

    /// Peak level (linear) per channel since the last call.
    #[must_use]
    pub fn take_peaks(&self) -> (f32, f32) {
        (self.peak_l.take(), self.peak_r.take())
    }

    /// Number of samples that exceeded full scale after gain. A non-zero and
    /// growing value means the gain is too high.
    #[must_use]
    pub fn clip_count(&self) -> u64 {
        self.clips.load(Ordering::Relaxed)
    }

    /// True while audio is currently being sent.
    #[must_use]
    pub fn is_transmitting(&self) -> bool {
        self.transmitting.load(Ordering::Relaxed)
    }

    fn encoder_settings(&self) -> EncoderSettings {
        EncoderSettings {
            bitrate_bps: self.bitrate_bps.load(Ordering::Relaxed),
            frame_ms: self.frame_ms.load(Ordering::Relaxed),
            stereo: self.stereo.load(Ordering::Relaxed),
        }
    }
}

/// Static capture configuration.
#[derive(Debug, Clone, Copy)]
pub struct CaptureConfig {
    /// Device sample rate in Hz.
    pub device_rate: u32,
    /// Interleaved channels delivered by the device.
    pub device_channels: usize,
    /// Initial channel mapping.
    pub channel_map: ChannelMap,
    /// Initial gain (1.0 = unity).
    pub gain: f32,
    /// Initial transmit mode.
    pub tx_mode: TxMode,
    /// Initial VOX threshold (dBFS).
    pub vox_threshold_db: f32,
    /// Initial VOX hang time (ms).
    pub vox_hang_ms: u32,
    /// Initial encoder settings.
    pub encoder: EncoderSettings,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            device_rate: SAMPLE_RATE,
            device_channels: 2,
            channel_map: ChannelMap::Stereo,
            gain: 1.0,
            tx_mode: TxMode::Ptt,
            vox_threshold_db: -40.0,
            vox_hang_ms: 400,
            encoder: EncoderSettings { bitrate_bps: 128_000, frame_ms: 20, stereo: true },
        }
    }
}

/// The capture pipeline.
#[derive(Debug)]
pub struct CaptureEngine {
    cfg: CaptureConfig,
    ctl: Arc<CaptureControl>,
    resampler: Option<Resampler>,
    stage: Vec<f32>,
    pending: Vec<f32>,
    encoder: FrameEncoder,
    preroll: VecDeque<Vec<f32>>,
    seq: u64,
    transmitting: bool,
    hang_frames: u32,
}

impl CaptureEngine {
    /// Build an engine; the returned control handle can be cloned freely.
    pub fn new(cfg: CaptureConfig) -> Result<(Self, Arc<CaptureControl>)> {
        let ctl = Arc::new(CaptureControl::new(&cfg));
        let resampler = (cfg.device_rate != SAMPLE_RATE)
            .then(|| Resampler::new(2, cfg.device_rate, SAMPLE_RATE));
        let engine = Self {
            cfg,
            ctl: Arc::clone(&ctl),
            resampler,
            stage: Vec::new(),
            pending: Vec::new(),
            encoder: FrameEncoder::new(cfg.encoder)?,
            preroll: VecDeque::new(),
            seq: 0,
            transmitting: false,
            hang_frames: 0,
        };
        Ok((engine, ctl))
    }

    /// Map channels, apply gain and meter one device buffer into `self.stage`
    /// (always interleaved stereo at the device rate).
    fn stage_input(&mut self, input: &[f32]) {
        let dch = self.cfg.device_channels.max(1);
        let gain = self.ctl.gain.load();
        let map = ChannelMap::from_u8(self.ctl.channel_map.load(Ordering::Relaxed));
        let (mut pl, mut pr) = (0.0_f32, 0.0_f32);
        let mut clips = 0_u64;
        self.stage.clear();
        for f in input.chunks_exact(dch) {
            let c0 = f[0];
            let c1 = if dch > 1 { f[1] } else { c0 };
            let (l, r) = match map {
                ChannelMap::Stereo => (c0, c1),
                ChannelMap::Left => (c0, c0),
                ChannelMap::Right => (c1, c1),
                ChannelMap::Mix => {
                    let m = 0.5 * (c0 + c1);
                    (m, m)
                }
            };
            let (l, r) = (l * gain, r * gain);
            clips += u64::from(l.abs() > 1.0) + u64::from(r.abs() > 1.0);
            let (l, r) = (l.clamp(-1.0, 1.0), r.clamp(-1.0, 1.0));
            pl = pl.max(l.abs());
            pr = pr.max(r.abs());
            self.stage.push(l);
            self.stage.push(r);
        }
        self.ctl.peak_l.max_assign(pl);
        self.ctl.peak_r.max_assign(pr);
        if clips > 0 {
            let _ = self.ctl.clips.fetch_add(clips, Ordering::Relaxed);
        }
    }

    /// Feed device samples; completed packets are appended to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<EncodedPacket>) {
        self.stage_input(input);
        match self.resampler.as_mut() {
            Some(rs) => rs.process(&self.stage, &mut self.pending),
            None => self.pending.extend_from_slice(&self.stage),
        }
        loop {
            let settings = self.ctl.encoder_settings();
            if settings != self.encoder.settings() && !self.apply_settings(settings) {
                // Keep running on the old encoder if the new one is invalid.
                self.ctl.set_encoder(self.encoder.settings());
            }
            let frame_len = self.encoder.settings().frame_samples() * 2;
            if self.pending.len() < frame_len {
                break;
            }
            let frame: Vec<f32> = self.pending.drain(..frame_len).collect();
            self.on_frame(frame, out);
        }
    }

    fn apply_settings(&mut self, s: EncoderSettings) -> bool {
        match FrameEncoder::new(s) {
            Ok(e) => {
                self.encoder = e;
                self.preroll.clear();
                true
            }
            Err(err) => {
                tracing::warn!("rejected encoder settings {s:?}: {err}");
                false
            }
        }
    }

    fn wants_tx(&mut self, frame: &[f32]) -> bool {
        match TxMode::from_u8(self.ctl.tx_mode.load(Ordering::Relaxed)) {
            TxMode::Continuous => true,
            TxMode::Ptt => self.ctl.ptt.load(Ordering::Relaxed),
            TxMode::Vox => {
                let peak = frame.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
                let thr = 10.0_f32.powf(self.ctl.vox_threshold_db.load() / 20.0);
                let frame_ms = self.encoder.settings().frame_ms;
                let hang = self.ctl.vox_hang_ms.load(Ordering::Relaxed).div_ceil(frame_ms);
                if peak >= thr {
                    self.hang_frames = hang;
                    true
                } else if self.hang_frames > 0 {
                    self.hang_frames -= 1;
                    true
                } else {
                    false
                }
            }
        }
    }

    fn on_frame(&mut self, frame: Vec<f32>, out: &mut Vec<EncodedPacket>) {
        let want = self.wants_tx(&frame);
        let step = u64::from(self.encoder.settings().frame_ms / 10);
        if want {
            if !self.transmitting {
                self.transmitting = true;
                self.ctl.transmitting.store(true, Ordering::Relaxed);
                if let Err(e) = self.encoder.reset() {
                    tracing::warn!("encoder reset failed: {e}");
                }
                let backlog: Vec<Vec<f32>> = self.preroll.drain(..).collect();
                for old in backlog {
                    self.encode_into(&old, step, out);
                }
            }
            self.encode_into(&frame, step, out);
        } else {
            if self.transmitting {
                self.transmitting = false;
                self.ctl.transmitting.store(false, Ordering::Relaxed);
                out.push(EncodedPacket { seq: self.seq, data: Vec::new(), terminator: true });
            }
            self.keep_preroll(frame);
        }
    }

    fn keep_preroll(&mut self, frame: Vec<f32>) {
        let tx_mode = TxMode::from_u8(self.ctl.tx_mode.load(Ordering::Relaxed));
        if tx_mode != TxMode::Vox {
            self.preroll.clear();
            return;
        }
        let keep = PREROLL_MS.div_ceil(self.encoder.settings().frame_ms) as usize;
        self.preroll.push_back(frame);
        while self.preroll.len() > keep {
            let _ = self.preroll.pop_front();
        }
    }

    fn encode_into(&mut self, frame: &[f32], step: u64, out: &mut Vec<EncodedPacket>) {
        let s = self.encoder.settings();
        let pcm: Vec<f32>;
        let input: &[f32] = if s.stereo {
            frame
        } else {
            pcm = frame.as_chunks::<2>().0.iter().map(|p| 0.5 * (p[0] + p[1])).collect();
            &pcm
        };
        if input.len() != s.frame_samples() * s.channels() {
            return;
        }
        match self.encoder.encode(input) {
            Ok(data) => {
                out.push(EncodedPacket { seq: self.seq, data, terminator: false });
                self.seq += step;
            }
            Err(e) => tracing::warn!("encode failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cfg: CaptureConfig, ctl_setup: impl FnOnce(&CaptureControl), blocks: usize) -> Vec<EncodedPacket> {
        let (mut e, ctl) = CaptureEngine::new(cfg).expect("engine");
        ctl_setup(&ctl);
        let mut out = Vec::new();
        let block = vec![0.2_f32; 480 * cfg.device_channels];
        for _ in 0..blocks {
            e.process(&block, &mut out);
        }
        out
    }

    #[test]
    fn continuous_emits_one_packet_per_frame() {
        let cfg = CaptureConfig { tx_mode: TxMode::Continuous, ..CaptureConfig::default() };
        let out = run(cfg, |_| {}, 100);
        assert_eq!(out.len(), 50);
        assert_eq!(out[1].seq - out[0].seq, 2);
    }

    #[test]
    fn ptt_sends_only_while_held_and_ends_with_terminator() {
        let cfg = CaptureConfig { tx_mode: TxMode::Ptt, ..CaptureConfig::default() };
        let (mut e, ctl) = CaptureEngine::new(cfg).expect("engine");
        let block = vec![0.2_f32; 960 * 2];
        let mut out = Vec::new();
        e.process(&block, &mut out);
        assert!(out.is_empty());
        ctl.set_ptt(true);
        e.process(&block, &mut out);
        e.process(&block, &mut out);
        assert_eq!(out.len(), 2);
        ctl.set_ptt(false);
        e.process(&block, &mut out);
        assert!(out.last().is_some_and(|p| p.terminator && p.data.is_empty()));
    }

    #[test]
    fn gain_is_unity_by_default_and_clips_are_counted() {
        let cfg = CaptureConfig { tx_mode: TxMode::Continuous, ..CaptureConfig::default() };
        let (mut e, ctl) = CaptureEngine::new(cfg).expect("engine");
        assert!((ctl.gain() - 1.0).abs() < f32::EPSILON);
        let mut out = Vec::new();
        e.process(&vec![0.9_f32; 960 * 2], &mut out);
        assert_eq!(ctl.clip_count(), 0);
        ctl.set_gain(2.0);
        e.process(&vec![0.9_f32; 960 * 2], &mut out);
        assert!(ctl.clip_count() > 0);
    }

    #[test]
    fn vox_opens_on_signal_with_preroll_and_closes_after_hang() {
        let cfg = CaptureConfig {
            tx_mode: TxMode::Vox,
            vox_hang_ms: 100,
            ..CaptureConfig::default()
        };
        let (mut e, _ctl) = CaptureEngine::new(cfg).expect("engine");
        let quiet = vec![0.0_f32; 960 * 2];
        let loud = vec![0.3_f32; 960 * 2];
        let mut out = Vec::new();
        for _ in 0..10 {
            e.process(&quiet, &mut out);
        }
        assert!(out.is_empty());
        e.process(&loud, &mut out);
        // 60 ms preroll = 3 frames, plus the loud frame.
        assert_eq!(out.len(), 4, "{out:?}");
        for _ in 0..10 {
            e.process(&quiet, &mut out);
        }
        assert!(out.iter().any(|p| p.terminator));
    }

    #[test]
    fn mono_encode_still_works() {
        let mut cfg = CaptureConfig { tx_mode: TxMode::Continuous, ..CaptureConfig::default() };
        cfg.encoder.stereo = false;
        cfg.encoder.bitrate_bps = 48_000;
        let out = run(cfg, |_| {}, 10);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn device_rate_is_converted() {
        let cfg = CaptureConfig {
            device_rate: 44_100,
            tx_mode: TxMode::Continuous,
            ..CaptureConfig::default()
        };
        let (mut e, _c) = CaptureEngine::new(cfg).expect("engine");
        let mut out = Vec::new();
        for _ in 0..100 {
            e.process(&vec![0.1_f32; 441 * 2], &mut out);
        }
        assert!((48..=50).contains(&out.len()), "{}", out.len());
    }
}
