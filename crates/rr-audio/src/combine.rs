//! Combine two sound cards into one stereo signal: card A on the left, card B
//! on the right.
//!
//! A two-radio station often has one sound card per radio. The two cards run on
//! independent clocks, so card B's audio is buffered and continuously
//! resampled onto card A's clock. The conversion ratio is steered by the buffer
//! depth (a few hundred ppm at most), using the same windowed-sinc converter as
//! the rest of the engine, so radio 2 stays aligned for hours without glitches
//! and without any processing beyond the rate conversion.
//!
//! Each card is reduced to mono (its channels averaged), since each carries one
//! radio.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use crate::asrc::Resampler;

/// Card B audio buffered ahead of use, in card-A frames (about 40 ms at 48 kHz).
const TARGET_MS: f64 = 40.0;
/// Card B backlog beyond this (ms) is discarded (B ran while A stalled).
const OVERFLOW_MS: f64 = 400.0;
/// Smoothing time constant of the depth estimate (s).
const DEPTH_TAU_S: f64 = 1.0;
/// Time over which a depth error is worked off (s).
const CORRECTION_S: f64 = 5.0;
/// Largest rate adjustment (fraction).
const MAX_ADJUST: f64 = 0.005;
/// Longest card-B backlog kept before conversion (s).
const MAX_RAW_S: f64 = 1.0;

/// Writer half: call [`CombinerFeed::push`] from card B's input callback.
#[derive(Debug, Clone)]
pub struct CombinerFeed {
    raw: Arc<Mutex<VecDeque<f32>>>,
    channels: usize,
    cap: usize,
}

impl CombinerFeed {
    /// Append card B samples (interleaved in its own channel layout).
    pub fn push(&self, samples: &[f32]) {
        let ch = self.channels.max(1);
        let mut q = self.raw.lock().unwrap_or_else(PoisonError::into_inner);
        for f in samples.chunks_exact(ch) {
            q.push_back(f.iter().sum::<f32>() / ch as f32);
        }
        while q.len() > self.cap {
            let _ = q.pop_front();
        }
    }
}

/// Counters for the combined signal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CombinerStats {
    /// Times card B ran dry and silence was used.
    pub underruns: u64,
    /// Times card B had too much backlog and some was discarded.
    pub overflows: u64,
}

/// Reader half: call [`StereoCombiner::process`] from card A's input callback.
#[derive(Debug)]
pub struct StereoCombiner {
    raw: Arc<Mutex<VecDeque<f32>>>,
    rate_a: f64,
    a_channels: usize,
    rs: Resampler,
    nominal: f64,
    taken: Vec<f32>,
    converted: Vec<f32>,
    ready: VecDeque<f32>,
    primed: bool,
    depth_avg: f64,
    stats: CombinerStats,
}

/// Create a combiner for card A (`rate_a`, `a_channels`) and card B
/// (`rate_b`, `b_channels`).
#[must_use]
pub fn stereo_combiner(rate_a: u32, a_channels: usize, rate_b: u32, b_channels: usize) -> (StereoCombiner, CombinerFeed) {
    let raw = Arc::new(Mutex::new(VecDeque::new()));
    let rs = Resampler::new(1, rate_b, rate_a);
    let feed = CombinerFeed {
        raw: Arc::clone(&raw),
        channels: b_channels.max(1),
        cap: (f64::from(rate_b) * MAX_RAW_S) as usize,
    };
    let comb = StereoCombiner {
        raw,
        rate_a: f64::from(rate_a),
        a_channels: a_channels.max(1),
        nominal: rs.ratio(),
        rs,
        taken: Vec::new(),
        converted: Vec::new(),
        ready: VecDeque::new(),
        primed: false,
        depth_avg: 0.0,
        stats: CombinerStats::default(),
    };
    (comb, feed)
}

impl StereoCombiner {
    fn target_frames(&self) -> f64 {
        self.rate_a * TARGET_MS / 1000.0
    }

    /// Steer the conversion so the buffered card-B audio converges on the target.
    fn steer(&mut self, frames: usize) {
        let depth = self.ready.len() as f64 + self.rs.pending_input_frames() / self.rs.ratio();
        let dt = frames as f64 / self.rate_a;
        let alpha = 1.0 - (-dt / DEPTH_TAU_S).exp();
        self.depth_avg += (depth - self.depth_avg) * alpha;
        let err = self.depth_avg - self.target_frames();
        let adjust = (err / (self.rate_a * CORRECTION_S)).clamp(-MAX_ADJUST, MAX_ADJUST);
        // More card-B audio buffered than wanted: consume it slightly faster.
        self.rs.set_ratio(self.nominal * (1.0 + adjust));
    }

    /// Combine one card-A buffer (interleaved in its own layout) with the
    /// matching card-B audio, appending interleaved stereo to `out`.
    pub fn process(&mut self, a: &[f32], out: &mut Vec<f32>) {
        let frames = a.len() / self.a_channels;
        {
            let mut q = self.raw.lock().unwrap_or_else(PoisonError::into_inner);
            self.taken.clear();
            self.taken.extend(q.drain(..));
        }
        self.converted.clear();
        self.rs.process(&self.taken, &mut self.converted);
        self.ready.extend(self.converted.iter().copied());

        let target = self.target_frames();
        if self.ready.len() as f64 > target + self.rate_a * OVERFLOW_MS / 1000.0 {
            let excess = self.ready.len() - target as usize;
            let _ = self.ready.drain(..excess);
            self.stats.overflows += 1;
        }
        if !self.primed && self.ready.len() as f64 >= target {
            self.primed = true;
            self.depth_avg = self.ready.len() as f64;
        }
        if self.primed {
            self.steer(frames);
        }

        out.reserve(frames * 2);
        let mut short = false;
        for f in a.chunks_exact(self.a_channels) {
            let left = f.iter().sum::<f32>() / self.a_channels as f32;
            let right = if self.primed {
                self.ready.pop_front().unwrap_or_else(|| {
                    short = true;
                    0.0
                })
            } else {
                0.0
            };
            out.push(left);
            out.push(right);
        }
        if short {
            // Card B stopped or fell behind: wait for a full buffer again.
            self.stats.underruns += 1;
            self.primed = false;
            self.ready.clear();
        }
    }

    /// Underrun and overflow counts so far.
    #[must_use]
    pub fn stats(&self) -> CombinerStats {
        self.stats
    }

    /// The current drift correction in parts per million.
    #[must_use]
    pub fn correction_ppm(&self) -> f64 {
        (self.rs.ratio() / self.nominal - 1.0) * 1e6
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulate both cards in virtual time with independent clock errors and
    /// callback sizes; return (stereo output, combiner).
    fn run(rate_a: u32, ppm_a: f64, rate_b: u32, ppm_b: f64, secs: f64) -> (Vec<f32>, StereoCombiner) {
        let (mut comb, feed) = stereo_combiner(rate_a, 2, rate_b, 1);
        let block_a = 480_usize;
        let block_b = 441_usize;
        let dt_a = block_a as f64 / (f64::from(rate_a) * (1.0 + ppm_a * 1e-6));
        let dt_b = block_b as f64 / (f64::from(rate_b) * (1.0 + ppm_b * 1e-6));
        let (mut ka, mut kb, mut nb) = (0_u64, 0_u64, 0_u64);
        let mut out = Vec::new();
        let tone = |n: u64, rate: u32, hz: f64| (0.5 * (2.0 * std::f64::consts::PI * hz * n as f64 / f64::from(rate)).sin()) as f32;
        loop {
            let (ta, tb) = (ka as f64 * dt_a, kb as f64 * dt_b);
            if ta >= secs {
                break;
            }
            if tb <= ta {
                let b: Vec<f32> = (0..block_b).map(|i| tone(nb + i as u64, rate_b, 1_500.0)).collect();
                nb += block_b as u64;
                feed.push(&b);
                kb += 1;
            } else {
                let a: Vec<f32> = (0..block_a).flat_map(|_| [0.25_f32, 0.25]).collect();
                comb.process(&a, &mut out);
                ka += 1;
            }
        }
        (out, comb)
    }

    fn right(out: &[f32]) -> Vec<f32> {
        out.iter().skip(1).step_by(2).copied().collect()
    }

    /// Frequency of a sine from its zero crossings.
    fn freq(x: &[f32], rate: f64) -> f64 {
        let crossings = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        crossings as f64 * rate / x.len() as f64
    }

    #[test]
    fn locks_two_drifting_cards_without_glitches() {
        let (out, comb) = run(48_000, -120.0, 44_100, 150.0, 120.0);
        assert_eq!(comb.stats(), CombinerStats { underruns: 0, overflows: 0 }, "{:?}", comb.stats());
        let r = right(&out);
        let steady = &r[48_000 * 10..];
        // No pops: a 0.5 amplitude 1.5 kHz tone moves at most ~0.098 per sample.
        let worst = steady.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0_f32, f32::max);
        assert!(worst < 0.11, "step {worst}");
        // Pitch correct on A's clock: B's 1.5 kHz, scaled by the clock ratio.
        let expected = 1_500.0 * (1.0 + 150e-6) / (1.0 - 120e-6);
        let f = freq(steady, 48_000.0);
        assert!((f - expected).abs() < 0.2, "f {f} expected {expected}");
        // Left is card A untouched.
        assert!(out.iter().step_by(2).skip(1000).all(|l| (l - 0.25).abs() < 1e-6));
    }

    #[test]
    fn recovers_after_card_b_stops() {
        let (mut comb, feed) = stereo_combiner(48_000, 1, 48_000, 1);
        let mut out = Vec::new();
        for _ in 0..100 {
            feed.push(&[0.1; 480]);
            comb.process(&[0.0; 480], &mut out);
        }
        for _ in 0..10 {
            comb.process(&[0.0; 480], &mut out);
        }
        assert!(comb.stats().underruns >= 1);
        for _ in 0..100 {
            feed.push(&[0.1; 480]);
            comb.process(&[0.0; 480], &mut out);
        }
        let tail = right(&out[out.len() - 960..]);
        assert!(tail.iter().all(|s| (s - 0.1).abs() < 1e-3), "{:?}", &tail[..4]);
    }
}
