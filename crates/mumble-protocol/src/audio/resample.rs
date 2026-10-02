//! Streaming sample-rate conversion for the capture path.
//!
//! The rest of the pipeline is built around a single rate (48 kHz, Opus's
//! native rate), but a capture device delivers whatever rate its hardware runs
//! at. Many rig USB codecs are 48 kHz, but `SignaLink` interfaces and a lot of
//! cheap USB dongles are 44.1 kHz. Feeding 44.1 kHz samples into a pipeline that
//! believes they are 48 kHz plays the audio back about 8.8% fast — a
//! semitone-and-a-half sharp — which on a radio means a received signal that is
//! simply wrong, not merely lower quality.
//!
//! [`LinearResampler`] converts a continuous mono stream from an arbitrary input
//! rate to a fixed output rate. It is deliberately:
//!
//! - **Streaming.** Capture delivers audio in a long sequence of small chunks,
//!   so the resampler carries its fractional read position *and the last input
//!   sample* across calls. Resetting per chunk would drop or duplicate a sample
//!   at every boundary — ~100 clicks a second — which is exactly the class of
//!   boundary artefact the rest of this crate works to avoid.
//! - **Linear.** One multiply-add per output sample, no allocation beyond the
//!   output buffer, trivially real-time. Linear interpolation has audible
//!   high-frequency roll-off, but a 44.1→48 ratio is gentle (and radio receive
//!   audio is band-limited well below Nyquist), so a polyphase/sinc filter would
//!   be cost without benefit here. If a future need appears, this type is the
//!   single place to upgrade.
//! - **Mono.** Capture downmixes to mono before this stage, so operating on a
//!   mono stream keeps the arithmetic obvious. Stereo capture (a later step)
//!   will resample each channel with its own instance.
//!
//! A unit ratio is special-cased to a straight copy, so the common 48 kHz device
//! pays nothing.

/// Converts a continuous mono `f32` stream from `input_rate` to `output_rate`,
/// preserving interpolation state across successive [`process`](Self::process)
/// calls so chunk boundaries introduce no discontinuity.
#[derive(Debug, Clone)]
pub struct LinearResampler {
    input_rate: u32,
    output_rate: u32,
    /// How far the output read-head advances through the input per output
    /// sample: `input_rate / output_rate`. For 44.1→48 this is ~0.919.
    step: f64,
    /// Fractional read position within the input stream, measured from
    /// `prev_sample` (position 0.0) toward the next input sample (1.0). Carried
    /// across calls so the phase is continuous.
    pos: f64,
    /// The final input sample of the previous chunk. The first output sample of
    /// the next chunk may need to interpolate between it and the new chunk's
    /// first sample; without it, every boundary would restart from zero.
    prev_sample: f32,
    /// False until the first sample is seen, so the very first output does not
    /// interpolate against a non-existent predecessor.
    primed: bool,
}

impl LinearResampler {
    /// Build a resampler from `input_rate` to `output_rate` (both in Hz).
    ///
    /// A zero input rate is treated as a unit (pass-through) ratio rather than
    /// dividing by zero; callers should never pass one, but a capture device
    /// reporting a nonsense rate must not panic the audio thread.
    #[must_use]
    pub fn new(input_rate: u32, output_rate: u32) -> Self {
        let step = if input_rate == 0 || output_rate == 0 {
            1.0
        } else {
            f64::from(input_rate) / f64::from(output_rate)
        };
        Self {
            input_rate,
            output_rate,
            step,
            pos: 0.0,
            prev_sample: 0.0,
            primed: false,
        }
    }

    /// Whether input and output rates are equal, so [`process`](Self::process)
    /// is a straight copy. Exposed so a caller can skip even the function call.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.input_rate == self.output_rate
    }

    /// The configured input rate in Hz.
    #[must_use]
    pub const fn input_rate(&self) -> u32 {
        self.input_rate
    }

    /// The configured output rate in Hz.
    #[must_use]
    pub const fn output_rate(&self) -> u32 {
        self.output_rate
    }

    /// Resample `input`, appending the output samples to `out`.
    ///
    /// Appends rather than returns so a caller can reuse one buffer across the
    /// stream without reallocating. State is retained, so calling this once with
    /// a whole stream and calling it many times with the stream's chunks produce
    /// the same result (to within f32 rounding).
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.is_identity() {
            out.extend_from_slice(input);
            return;
        }
        if input.is_empty() {
            return;
        }

        // Prime on the very first sample ever seen: adopt it as `prev_sample`
        // and start the read head at it, so output begins exactly at the stream
        // start with no fabricated leading sample.
        if !self.primed {
            self.prev_sample = input[0];
            self.primed = true;
            self.pos = 0.0;
        }

        // Virtual indices for this call: `prev_sample` sits at -1, `input[0]`
        // at 0, ..., `input[n-1]` at n-1. `pos` is a read head in that space.
        // An output at `pos` interpolates between the samples at `floor(pos)`
        // (left) and `floor(pos)+1` (right). The right endpoint is only
        // available up to `n-1`, so emit while `pos <= n-1`; once the head would
        // need `input[n]` — a sample not in this chunk — stop and defer it to
        // the next call. That deferral is what makes chunked == one-shot: no
        // boundary sample is ever interpolated against a fabricated neighbour.
        let n = input.len();
        let last = (n - 1) as f64;
        while self.pos <= last {
            let base = self.pos.floor();
            let frac = (self.pos - base) as f32;
            let idx = base as isize;

            // idx == -1 means "the carried last sample of the previous chunk".
            let left = if idx < 0 {
                self.prev_sample
            } else {
                input[idx as usize]
            };
            // right == idx + 1. The only way it reaches `n` here is `pos`
            // landing exactly on the last sample (idx == n-1, frac == 0), where
            // the right endpoint carries zero weight; use `left` so there is no
            // out-of-bounds read and the result is unchanged.
            let right_idx = idx + 1;
            let right = if right_idx < 0 {
                self.prev_sample
            } else if (right_idx as usize) < n {
                input[right_idx as usize]
            } else {
                left
            };

            out.push(left + (right - left) * frac);
            self.pos += self.step;
        }

        // Carry: the last real sample of this chunk becomes the next chunk's
        // `prev_sample` (its virtual index -1), so shift `pos` left by `n`.
        self.prev_sample = input[n - 1];
        self.pos -= n as f64;
    }

    /// Resample a whole slice in one call, returning a fresh buffer. Convenience
    /// for tests and one-shot conversions.
    #[must_use]
    pub fn resample_all(&mut self, input: &[f32]) -> Vec<f32> {
        // Rough output-size estimate to avoid repeated growth.
        let estimate = ((input.len() as f64) / self.step).ceil() as usize + 1;
        let mut out = Vec::with_capacity(estimate);
        self.process(input, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Root-mean-square of a slice, for comparing signal levels.
    fn rms(xs: &[f32]) -> f32 {
        if xs.is_empty() {
            return 0.0;
        }
        let sum: f64 = xs.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
        (sum / xs.len() as f64).sqrt() as f32
    }

    #[test]
    fn identity_rate_is_a_straight_copy() {
        let mut r = LinearResampler::new(48_000, 48_000);
        assert!(r.is_identity());
        let input: Vec<f32> = (0..100).map(|i| i as f32).collect();
        assert_eq!(r.resample_all(&input), input);
    }

    #[test]
    fn downrate_shrinks_and_uprate_grows_sample_count() {
        // 48k -> 24k halves the count; 24k -> 48k doubles it (within one).
        let input: Vec<f32> = vec![0.5; 1000];
        let down = LinearResampler::new(48_000, 24_000).resample_all(&input);
        assert!(
            (down.len() as isize - 500).abs() <= 1,
            "48->24k of 1000 should be ~500, got {}",
            down.len()
        );
        let up = LinearResampler::new(24_000, 48_000).resample_all(&input);
        assert!(
            (up.len() as isize - 2000).abs() <= 2,
            "24->48k of 1000 should be ~2000, got {}",
            up.len()
        );
    }

    #[test]
    fn output_count_matches_the_rate_ratio_for_44100_to_48000() {
        // The real case: one second of 44.1k becomes ~48k samples.
        let input = vec![0.0f32; 44_100];
        let out = LinearResampler::new(44_100, 48_000).resample_all(&input);
        // Within a handful of samples of 48000 (depends on edge handling).
        assert!(
            (out.len() as isize - 48_000).abs() <= 2,
            "expected ~48000, got {}",
            out.len()
        );
    }

    #[test]
    fn a_constant_signal_stays_constant() {
        // Interpolation of a DC level must not introduce ripple.
        let input = vec![0.75f32; 500];
        let out = LinearResampler::new(44_100, 48_000).resample_all(&input);
        for (i, &s) in out.iter().enumerate() {
            assert!(
                (s - 0.75).abs() < 1e-5,
                "sample {i} drifted from DC: {s}"
            );
        }
    }

    #[test]
    fn streaming_in_chunks_equals_one_shot() {
        // The property the capture thread relies on: feeding the stream in
        // arbitrary chunks yields the same output as one big call.
        let input: Vec<f32> = (0..2000)
            .map(|i| (i as f32 * 0.05).sin())
            .collect();

        let one_shot = LinearResampler::new(44_100, 48_000).resample_all(&input);

        let mut chunked = LinearResampler::new(44_100, 48_000);
        let mut out = Vec::new();
        // Deliberately uneven chunk sizes, including a tiny one.
        for chunk in [&input[0..480], &input[480..481], &input[481..1500], &input[1500..2000]] {
            chunked.process(chunk, &mut out);
        }

        assert_eq!(
            one_shot.len(),
            out.len(),
            "chunked and one-shot lengths differ"
        );
        for (i, (a, b)) in one_shot.iter().zip(out.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-6,
                "sample {i} differs: one-shot={a}, chunked={b}"
            );
        }
    }

    #[test]
    fn a_sine_keeps_its_level_and_frequency_band() {
        // A 1 kHz tone resampled 44.1->48k should keep roughly its amplitude;
        // linear interpolation attenuates highs but 1 kHz is far below Nyquist.
        let fs_in = 44_100.0f32;
        let input: Vec<f32> = (0..44_100)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / fs_in).sin())
            .collect();
        let out = LinearResampler::new(44_100, 48_000).resample_all(&input);
        let sine_rms = 1.0 / std::f32::consts::SQRT_2; // ~0.707
        assert!(
            (rms(&out) - sine_rms).abs() < 0.02,
            "resampled 1 kHz tone RMS {} strayed from {}",
            rms(&out),
            sine_rms
        );
    }

    #[test]
    fn empty_input_produces_no_output_and_keeps_state() {
        let mut r = LinearResampler::new(44_100, 48_000);
        let mut out = Vec::new();
        r.process(&[], &mut out);
        assert!(out.is_empty());
        // A subsequent real chunk still works.
        r.process(&[0.1, 0.2, 0.3], &mut out);
        assert!(!out.is_empty());
    }

    #[test]
    fn zero_rate_does_not_panic() {
        // A device reporting a nonsense rate must not divide by zero.
        let mut r = LinearResampler::new(0, 48_000);
        let out = r.resample_all(&[1.0, 2.0, 3.0]);
        assert_eq!(out, vec![1.0, 2.0, 3.0], "zero rate falls back to copy");
    }
}
