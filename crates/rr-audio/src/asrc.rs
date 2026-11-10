//! Asynchronous sample-rate converter.
//!
//! A windowed-sinc (Kaiser) polyphase interpolator with a variable ratio. It is
//! used for two jobs: converting between the device rate and the 48 kHz Opus
//! rate, and absorbing the tiny clock difference between sender and receiver
//! (by nudging the ratio a few hundred ppm). Unlike linear interpolation it has
//! a real anti-alias / anti-image filter, so it adds no audible distortion.

/// Half-width of the prototype filter, in prototype samples.
const HALF: usize = 32;
/// Table resolution: entries per prototype sample.
const PHASES: usize = 256;
/// Prototype low-pass cutoff in cycles per sample (Nyquist = 0.5).
const CUTOFF: f64 = 0.45;
/// Kaiser window shape parameter (about 90 dB stop-band attenuation).
const BETA: f64 = 9.0;
/// Largest supported ratio (input frames consumed per output frame).
const MAX_RATIO: f64 = 8.0;

/// Zeroth-order modified Bessel function of the first kind.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    let mut k = 1.0;
    while term > sum * 1e-17 {
        term *= q / (k * k);
        sum += term;
        k += 1.0;
    }
    sum
}

/// Streaming variable-ratio resampler for interleaved `f32` audio.
#[derive(Debug)]
pub struct Resampler {
    channels: usize,
    ratio: f64,
    /// Prototype filter sampled at `PHASES` points per unit, plus guard entries.
    table: Vec<f32>,
    /// Interleaved input history and not-yet-consumed input.
    buf: Vec<f32>,
    /// Position of the next output sample, in input frames from `buf[0]`.
    pos: f64,
}

impl Resampler {
    /// Create a converter for `channels` interleaved channels that consumes
    /// `in_rate` frames per second and produces `out_rate` frames per second.
    #[must_use]
    pub fn new(channels: usize, in_rate: u32, out_rate: u32) -> Self {
        let channels = channels.max(1);
        let n = HALF * PHASES + 2;
        let beta_norm = bessel_i0(BETA);
        let table = (0..n)
            .map(|i| {
                let v = i as f64 / PHASES as f64;
                if v >= HALF as f64 {
                    return 0.0;
                }
                let x = 2.0 * CUTOFF * v;
                let sinc = if x.abs() < 1e-12 {
                    1.0
                } else {
                    (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
                };
                let r = v / HALF as f64;
                let win = bessel_i0(BETA * (1.0 - r * r).max(0.0).sqrt()) / beta_norm;
                (2.0 * CUTOFF * sinc * win) as f32
            })
            .collect();
        let ratio = f64::from(in_rate) / f64::from(out_rate.max(1));
        Self {
            channels,
            ratio: ratio.clamp(1.0 / MAX_RATIO, MAX_RATIO),
            table,
            buf: Vec::new(),
            pos: 0.0,
        }
    }

    /// Current ratio (input frames consumed per output frame).
    #[must_use]
    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    /// Change the ratio on the fly (used for clock-drift compensation).
    pub fn set_ratio(&mut self, ratio: f64) {
        self.ratio = ratio.clamp(1.0 / MAX_RATIO, MAX_RATIO);
    }

    /// Forget all history (start of a new talk-spurt).
    pub fn reset(&mut self) {
        self.buf.clear();
        self.pos = 0.0;
    }

    /// Input frames buffered but not yet fully consumed.
    #[must_use]
    pub fn pending_input_frames(&self) -> f64 {
        (self.buf.len() / self.channels) as f64 - self.pos
    }

    /// Append `input` (interleaved) and push every output frame that can now be
    /// computed onto `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.buf.extend_from_slice(input);
        let ch = self.channels;
        let frames = self.buf.len() / ch;
        let scale = (1.0 / self.ratio).min(1.0);
        // Support (in input frames) of the scaled prototype.
        let support = (HALF as f64 / scale).ceil() as usize;
        let mut acc = vec![0.0_f32; ch];
        loop {
            let base = self.pos.floor() as isize;
            if base + support as isize >= frames as isize {
                break;
            }
            let t = self.pos;
            let lo = (base + 1 - support as isize).max(0);
            let hi = base + support as isize;
            acc.fill(0.0);
            let mut wsum = 0.0_f32;
            for k in lo..=hi {
                let v = (t - k as f64).abs() * scale;
                let w = self.weight(v) * scale as f32;
                wsum += w;
                let o = k as usize * ch;
                for (a, s) in acc.iter_mut().zip(&self.buf[o..o + ch]) {
                    *a += w * s;
                }
            }
            // Normalising by the tap sum makes DC gain exactly 1 at every phase.
            let norm = if wsum.abs() > 1e-6 { 1.0 / wsum } else { 1.0 };
            out.extend(acc.iter().map(|a| a * norm));
            self.pos += self.ratio;
        }
        // Drop history we no longer need.
        let keep_from = (self.pos.floor() as isize - support as isize - 1).max(0) as usize;
        if keep_from > 0 {
            let _ = self.buf.drain(..keep_from * ch);
            self.pos -= keep_from as f64;
        }
    }

    /// Prototype filter value at distance `v` (in prototype samples).
    #[inline]
    fn weight(&self, v: f64) -> f32 {
        let x = v * PHASES as f64;
        let i = x as usize;
        if i >= HALF * PHASES {
            return 0.0;
        }
        let f = (x - i as f64) as f32;
        self.table[i] * (1.0 - f) + self.table[i + 1] * f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: f64, hz: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (0.5 * (2.0 * std::f64::consts::PI * hz * i as f64 / rate).sin()) as f32)
            .collect()
    }

    /// RMS error of `out` against an ideal tone, ignoring edge transients.
    fn residual_db(out: &[f32], rate: f64, hz: f64) -> f64 {
        let skip = 600.min(out.len() / 4);
        let body = &out[skip..out.len() - skip];
        let mut err = 0.0;
        let mut sig = 0.0;
        for (i, s) in body.iter().enumerate() {
            let idx = (i + skip) as f64;
            let ideal = 0.5 * (2.0 * std::f64::consts::PI * hz * idx / rate).sin();
            err += (f64::from(*s) - ideal).powi(2);
            sig += ideal * ideal;
        }
        10.0 * (err / sig).log10()
    }

    #[test]
    fn upsample_44k1_to_48k_is_clean() {
        let mut r = Resampler::new(1, 44_100, 48_000);
        let mut out = Vec::new();
        r.process(&tone(44_100.0, 1_000.0, 44_100), &mut out);
        let db = residual_db(&out, 48_000.0, 1_000.0);
        assert!(db < -80.0, "residual {db} dB");
    }

    #[test]
    fn downsample_96k_to_48k_is_clean() {
        let mut r = Resampler::new(1, 96_000, 48_000);
        let mut out = Vec::new();
        r.process(&tone(96_000.0, 3_000.0, 96_000), &mut out);
        let db = residual_db(&out, 48_000.0, 3_000.0);
        assert!(db < -80.0, "residual {db} dB");
    }

    #[test]
    fn chunked_equals_whole() {
        let src = tone(44_100.0, 700.0, 20_000);
        let mut a = Resampler::new(1, 44_100, 48_000);
        let mut whole = Vec::new();
        a.process(&src, &mut whole);
        let mut b = Resampler::new(1, 44_100, 48_000);
        let mut parts = Vec::new();
        for c in src.chunks(333) {
            b.process(c, &mut parts);
        }
        let n = whole.len().min(parts.len());
        assert!(n > 15_000);
        for i in 0..n {
            assert!((whole[i] - parts[i]).abs() < 1e-5, "mismatch at {i}");
        }
    }

    #[test]
    fn ratio_ramp_has_no_discontinuity() {
        let mut r = Resampler::new(1, 48_000, 48_000);
        let src = tone(48_000.0, 1_000.0, 4_800);
        let mut out = Vec::new();
        for (i, c) in src.chunks(480).enumerate() {
            r.set_ratio(1.0 + 0.002 * (i as f64 / 10.0));
            r.process(c, &mut out);
        }
        let worst = out
            .windows(2)
            .skip(200)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0_f32, f32::max);
        // A 1 kHz, 0.5 amplitude tone changes at most ~0.066 per sample.
        assert!(worst < 0.08, "step {worst}");
    }

    #[test]
    fn stereo_channels_stay_separate() {
        let mut r = Resampler::new(2, 44_100, 48_000);
        let n = 20_000;
        let l = tone(44_100.0, 1_000.0, n);
        let mut inter = Vec::with_capacity(2 * n);
        for s in &l {
            inter.push(*s);
            inter.push(0.0);
        }
        let mut out = Vec::new();
        r.process(&inter, &mut out);
        let right_peak = out.iter().skip(1).step_by(2).fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(right_peak < 1e-6, "crosstalk {right_peak}");
    }
}
