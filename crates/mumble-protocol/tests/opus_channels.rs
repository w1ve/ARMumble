//! Opus channel-count behaviour: the foundation of stereo support.
//!
//! `RemoteRadio` wants to carry **stereo** radio audio (a transceiver's main and
//! sub receiver on left and right) over Mumble. Mumble has no stereo support and
//! no channel-count field anywhere in its wire format — not in the 1.5 protobuf
//! `MumbleUDP.Audio` message, not in `Version`, `Authenticate`, `CodecVersion` or
//! `ServerSync`. Upstream issue #2829 has been open for years.
//!
//! The plan is therefore to send stereo **without** a protocol change, relying on
//! two properties of Opus itself:
//!
//! 1. **Opus packets are self-describing** — the channel count lives in the TOC
//!    byte — and Mumble relays `opus_data` as an opaque blob.
//! 2. **`libopus` converts on channel mismatch.** A mono decoder downmixes a
//!    stereo packet; a stereo decoder upmixes a mono one.
//!
//! Property 2 is what makes mono fallback *free* rather than something we build:
//! every other Mumble client creates a mono decoder and would simply hear a
//! downmix, and our own receive path can be unconditionally stereo and still
//! handle mono senders.
//!
//! **These tests exist to prove property 2 rather than assume it.** If any of
//! them fail, the no-negotiation design is wrong and stereo needs an explicit
//! capability gate (this fork already has one: `Version.fancy_version`, field 6).
//! See `context.md` §14b.
//!
//! # ⚠️ Methodology, and a trap already fallen into
//!
//! Opus is a **predictive** codec: both encoder and decoder carry state across
//! packets. An earlier version of this file measured a single frame obtained by
//! feeding the *same* packet to the decoder repeatedly. That is meaningless —
//! every decode after the first advances the decoder's state as though time had
//! passed, so it reconstructs from predictions that no longer match.
//!
//! The symptom was data that could not be true: 96 kbps measured dramatically
//! *better* channel separation than 128 kbps and 160 kbps, and the channel-coding
//! controls appeared to have exactly zero effect. **Physically implausible
//! results mean the instrument is wrong, not the codec.**
//!
//! So: encode a run of phase-continuous frames, decode that run **in order**, and
//! analyse only the tail, once both sides are past their warm-up.
//!
//! ```sh
//! cargo test --package mumble-protocol --test opus_channels --features opus-codec
//! ```

#![allow(
    unused_crate_dependencies,
    missing_docs,
    reason = "feature-gated test: when `opus-codec` is off the crate is empty"
)]
#![cfg(feature = "opus-codec")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "integration test: unwrap/expect are idiomatic here"
)]

use mumble_protocol::audio::decoder::{AudioDecoder, OpusDecoder};
use mumble_protocol::audio::encoder::{
    AudioEncoder, EncodedPacket, ForceChannels, OpusEncoder, OpusEncoderConfig,
};
use mumble_protocol::audio::sample::{AudioFormat, AudioFrame, SampleFormat};

/// 20 ms at 48 kHz — Mumble's default frame, and the project's default
/// `OpusEncoderConfig::frame_size`.
const FRAME: usize = 960;
const SAMPLE_RATE: u32 = 48_000;

/// Frames encoded and decoded before measuring.
///
/// Opus has an algorithmic lookahead and its rate control needs several frames to
/// settle; the decoder likewise. The existing `audio_quality.rs` suite trims a
/// warm-up for the same reason.
const WARMUP_FRAMES: usize = 10;

/// Frames analysed, concatenated into one window.
///
/// 10 frames is 200 ms, which puts the analysis bin width at 5 Hz. A single 20 ms
/// frame gives 50 Hz bins and a measurably noisier result.
const MEASURE_FRAMES: usize = 10;

const TOTAL_FRAMES: usize = WARMUP_FRAMES + MEASURE_FRAMES;

/// Frequencies chosen to land on **exact** analysis bins at both the 20 ms frame
/// size and the 200 ms measurement window, so there is no spectral leakage from
/// one tone into the other's bin. Leakage would masquerade as channel bleed and
/// make every separation figure below meaningless.
const TONE_LEFT: f32 = 450.0;
const TONE_RIGHT: f32 = 1000.0;

const STEREO_48KHZ_F32: AudioFormat = AudioFormat {
    sample_rate: SAMPLE_RATE,
    channels: 2,
    sample_format: SampleFormat::F32,
};

/// Bitrate floor for carrying two uncorrelated receivers.
///
/// Measured on `libopus` via `separation_table_across_bitrate_and_channel_forcing`
/// with two hard-panned uncorrelated tones. Worst-case channel separation:
///
/// | bitrate | left | right |
/// |---|---|---|
/// | 48 kbps | 5.4 dB | 5.6 dB |
/// | **72 kbps** (project default) | **13.0 dB** | **13.3 dB** |
/// | **96 kbps** | **52.6 dB** | **61.6 dB** |
/// | 128 kbps | 54.0 dB | 66.0 dB |
/// | 160 kbps | 57.9 dB | 66.6 dB |
///
/// There is a **cliff between 72 and 96 kbps** — a ~40 dB jump — which is Opus
/// giving up coupled/intensity stereo for genuine dual-channel coding once it has
/// the bits. Below it, a sub-receiver audibly bleeds into the main one. Above it,
/// separation is effectively perfect and more bits buy almost nothing.
///
/// ⚠️ These are **pure tones**, the easiest case. Real receiver audio is
/// broadband and noise-like, which couples harder, so the shipping default should
/// sit above this floor with headroom and be validated against a real signal.
const RECOMMENDED_STEREO_BITRATE: i32 = 96_000;

// ---------------------------------------------------------------------------
//  Signal helpers
// ---------------------------------------------------------------------------

/// A continuous sine wave of `n` samples, so frames sliced from it are
/// phase-continuous. A phase discontinuity at each frame boundary would show up
/// as broadband splatter and pollute the separation measurement.
fn sine(freq_hz: f32, amplitude: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            amplitude * (std::f32::consts::TAU * freq_hz * t).sin()
        })
        .collect()
}

/// Build an interleaved stereo frame (L R L R ...), the convention `AudioFrame`
/// documents.
fn stereo_frame(left: &[f32], right: &[f32], sequence: u64) -> AudioFrame {
    assert_eq!(left.len(), right.len());
    let mut data = Vec::with_capacity(left.len() * 2 * 4);
    for (l, r) in left.iter().zip(right) {
        data.extend_from_slice(&l.to_ne_bytes());
        data.extend_from_slice(&r.to_ne_bytes());
    }
    AudioFrame {
        data,
        format: STEREO_48KHZ_F32,
        sequence,
        is_silent: false,
    }
}

fn mono_frame(samples: &[f32], sequence: u64) -> AudioFrame {
    let mut data = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        data.extend_from_slice(&s.to_ne_bytes());
    }
    AudioFrame {
        data,
        format: AudioFormat::MONO_48KHZ_F32,
        sequence,
        is_silent: false,
    }
}

/// Split interleaved stereo into (left, right).
fn deinterleave(samples: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let left = samples.iter().step_by(2).copied().collect();
    let right = samples.iter().skip(1).step_by(2).copied().collect();
    (left, right)
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

/// Magnitude at a single frequency via the Goertzel algorithm.
///
/// Goertzel rather than an FFT because only two known bins are ever of interest,
/// and it is a dozen lines with no dependency. The result is scaled to be
/// comparable to the amplitude of a pure tone at that frequency.
fn magnitude_at(samples: &[f32], freq_hz: f32) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let w = std::f32::consts::TAU * freq_hz / SAMPLE_RATE as f32;
    let coeff = 2.0 * w.cos();
    let mut s_prev = 0.0f32;
    let mut s_prev2 = 0.0f32;
    for &x in samples {
        let s = x + coeff * s_prev - s_prev2;
        s_prev2 = s_prev;
        s_prev = s;
    }
    let power = s_prev2.mul_add(s_prev2, s_prev * s_prev) - coeff * s_prev * s_prev2;
    power.max(0.0).sqrt() * 2.0 / samples.len() as f32
}

/// How far `wanted` stands above `unwanted`, in dB — channel separation as one
/// comparable number.
fn separation_db(wanted: f32, unwanted: f32) -> f32 {
    // Floor the denominator so near-perfect separation yields a large finite
    // number rather than infinity.
    20.0 * (wanted.max(1e-9) / unwanted.max(1e-9)).log10()
}

// ---------------------------------------------------------------------------
//  Encode / decode drivers
// ---------------------------------------------------------------------------

fn encoder(format: AudioFormat, bitrate: i32) -> OpusEncoder {
    let config = OpusEncoderConfig {
        bitrate,
        ..OpusEncoderConfig::default()
    };
    OpusEncoder::new(config, format).expect("construct Opus encoder")
}

/// A stereo encoder with the channel-coding controls set explicitly, so a test
/// can isolate what each one contributes.
fn stereo_encoder(bitrate: i32, force: ForceChannels, no_phase_inversion: bool) -> OpusEncoder {
    let config = OpusEncoderConfig {
        bitrate,
        force_channels: force,
        phase_inversion_disabled: no_phase_inversion,
        ..OpusEncoderConfig::default()
    };
    OpusEncoder::new(config, STEREO_48KHZ_F32).expect("construct stereo Opus encoder")
}

/// Encode [`TOTAL_FRAMES`] phase-continuous frames, returning **every** packet in
/// order.
///
/// Returning the whole run rather than just the last packet is what makes a
/// correct decode possible — see the methodology note at the top of this file.
fn encode_run(enc: &mut OpusEncoder, build: impl Fn(usize) -> AudioFrame) -> Vec<Vec<u8>> {
    let packets: Vec<Vec<u8>> = (0..TOTAL_FRAMES)
        .map(|i| enc.encode(&build(i)).expect("encode frame").data)
        .collect();
    assert!(
        packets.iter().all(|p| !p.is_empty()),
        "encoder produced an empty packet mid-run"
    );
    packets
}

/// Decode a run **in order** and return the concatenated PCM of the final
/// [`MEASURE_FRAMES`] frames, by which point both codecs are past warm-up.
fn decode_run_tail(dec: &mut OpusDecoder, packets: &[Vec<u8>]) -> Vec<f32> {
    let mut tail = Vec::new();
    for (i, data) in packets.iter().enumerate() {
        let encoded = EncodedPacket {
            data: data.clone(),
            sequence: i as u64,
            frame_samples: FRAME as u32,
        };
        let frame = dec.decode(&encoded).expect("decode packet");
        if i >= WARMUP_FRAMES {
            tail.extend_from_slice(frame.as_f32_samples());
        }
    }
    assert!(!tail.is_empty(), "decoded nothing after the warm-up");
    tail
}

/// Decode a run in order and return the final frame, for the tests that care
/// about one frame's shape rather than its spectrum.
fn decode_run_last_frame(dec: &mut OpusDecoder, packets: &[Vec<u8>]) -> AudioFrame {
    let mut last = None;
    for (i, data) in packets.iter().enumerate() {
        let encoded = EncodedPacket {
            data: data.clone(),
            sequence: i as u64,
            frame_samples: FRAME as u32,
        };
        last = Some(dec.decode(&encoded).expect("decode packet"));
    }
    last.expect("at least one decoded frame")
}

/// Stereo source: 450 Hz hard left, 1000 Hz hard right. Deliberately
/// **uncorrelated**, which is the real-world case for a transceiver's main and
/// sub receiver — and the hardest case for Opus, whose stereo coding couples
/// channels on the assumption that they are related.
fn stereo_two_tone_builder() -> impl Fn(usize) -> AudioFrame {
    let total = FRAME * TOTAL_FRAMES;
    let left = sine(TONE_LEFT, 0.5, total);
    let right = sine(TONE_RIGHT, 0.5, total);
    move |i| {
        let lo = i * FRAME;
        stereo_frame(&left[lo..lo + FRAME], &right[lo..lo + FRAME], i as u64)
    }
}

fn mono_tone_builder() -> impl Fn(usize) -> AudioFrame {
    let total = FRAME * TOTAL_FRAMES;
    let signal = sine(TONE_LEFT, 0.5, total);
    move |i| {
        let lo = i * FRAME;
        mono_frame(&signal[lo..lo + FRAME], i as u64)
    }
}

/// Encode the two-tone stereo signal and report (left, right) separation in dB.
fn measure_separation(bitrate: i32, force: ForceChannels, no_phase_inversion: bool) -> (f32, f32) {
    let mut enc = stereo_encoder(bitrate, force, no_phase_inversion);
    let packets = encode_run(&mut enc, stereo_two_tone_builder());
    let mut dec = OpusDecoder::new(STEREO_48KHZ_F32).expect("stereo decoder");
    let (left, right) = deinterleave(&decode_run_tail(&mut dec, &packets));
    (
        separation_db(
            magnitude_at(&left, TONE_LEFT),
            magnitude_at(&left, TONE_RIGHT),
        ),
        separation_db(
            magnitude_at(&right, TONE_RIGHT),
            magnitude_at(&right, TONE_LEFT),
        ),
    )
}

// ---------------------------------------------------------------------------
//  The two properties the whole stereo plan depends on
// ---------------------------------------------------------------------------

/// **The mono-fallback guarantee.** Every other Mumble client builds a mono
/// decoder. If a stereo packet made one of them error out, fall silent, or drop a
/// receiver entirely, we could not transmit stereo into a shared channel without
/// breaking everyone else — and the no-negotiation design would collapse.
///
/// Asserts the decode succeeds, produces a correctly sized mono frame, is
/// audible, and contains **both** source tones — i.e. `libopus` genuinely
/// downmixed rather than discarding a channel.
#[test]
fn stereo_packet_decoded_by_a_mono_decoder_is_a_real_downmix() {
    let mut enc = encoder(STEREO_48KHZ_F32, RECOMMENDED_STEREO_BITRATE);
    let packets = encode_run(&mut enc, stereo_two_tone_builder());

    let mut dec = OpusDecoder::new(AudioFormat::MONO_48KHZ_F32).expect("mono decoder");
    let frame = decode_run_last_frame(&mut dec, &packets);

    assert_eq!(frame.format.channels, 1, "decoder output must stay mono");
    assert_eq!(
        frame.sample_count(),
        FRAME,
        "a 20 ms packet must yield 960 samples per channel regardless of the \
         encoder's channel count"
    );
    assert_eq!(frame.data.len(), FRAME * 4, "mono f32: 4 bytes per sample");

    // Re-run for the spectrum over a longer window.
    let mut dec = OpusDecoder::new(AudioFormat::MONO_48KHZ_F32).expect("mono decoder");
    let downmix = decode_run_tail(&mut dec, &packets);
    assert!(
        rms(&downmix) > 0.05,
        "downmix is effectively silent (rms {:.4}); a mono decoder appears not to \
         handle a stereo packet",
        rms(&downmix)
    );

    // Both tones must survive. If only one did, libopus would be dropping a
    // channel rather than summing, and a sub-receiver would be inaudible to every
    // stock Mumble client.
    let left_tone = magnitude_at(&downmix, TONE_LEFT);
    let right_tone = magnitude_at(&downmix, TONE_RIGHT);
    let noise = magnitude_at(&downmix, 7_000.0);
    assert!(
        left_tone > noise * 10.0,
        "450 Hz (left channel) missing from the downmix: {left_tone:.5} vs noise {noise:.5}"
    );
    assert!(
        right_tone > noise * 10.0,
        "1000 Hz (right channel) missing from the downmix: {right_tone:.5} vs noise {noise:.5}"
    );
}

/// **Lets our receive path be unconditionally stereo.** If a stereo decoder
/// handles mono packets, the inbound side needs no per-speaker channel
/// negotiation and no format switching mid-stream — which matters because
/// `AudioMixer`'s per-speaker buffers carry no format metadata at all.
///
/// Also asserts the upmix is a true duplication, because that is what makes a
/// mono speaker sound centred rather than lateralised.
#[test]
fn mono_packet_decoded_by_a_stereo_decoder_duplicates_the_channel() {
    let mut enc = encoder(AudioFormat::MONO_48KHZ_F32, 72_000);
    let packets = encode_run(&mut enc, mono_tone_builder());

    let mut dec = OpusDecoder::new(STEREO_48KHZ_F32).expect("stereo decoder");
    let frame = decode_run_last_frame(&mut dec, &packets);

    assert_eq!(frame.format.channels, 2);
    assert_eq!(
        frame.sample_count(),
        FRAME,
        "sample_count() is per channel, so it must not change with channel count"
    );
    assert_eq!(
        frame.data.len(),
        FRAME * 2 * 4,
        "stereo f32: 960 frames x 2 channels x 4 bytes"
    );

    let (left, right) = deinterleave(frame.as_f32_samples());
    assert_eq!(left.len(), FRAME);
    assert!(
        rms(&left) > 0.05,
        "upmix is silent (rms {:.4})",
        rms(&left)
    );

    let worst = left
        .iter()
        .zip(&right)
        .map(|(l, r)| (l - r).abs())
        .fold(0.0f32, f32::max);
    assert!(
        worst < 1e-5,
        "a mono source must upmix to identical channels; worst L/R difference was {worst}"
    );
}

/// **The feature itself.** Two uncorrelated tones must come back on their own
/// channels. This is what makes a dual-receiver stream useful rather than a
/// stereo-shaped mono stream.
#[test]
fn stereo_round_trip_keeps_the_channels_apart() {
    let (left_sep, right_sep) = measure_separation(
        RECOMMENDED_STEREO_BITRATE,
        ForceChannels::Stereo,
        false,
    );

    // 12 dB is a deliberately forgiving floor: far below what a listener would
    // call "separate", but well above the ~0 dB a collapsed or swapped stereo
    // image gives, so this fails on real breakage rather than on codec tuning.
    assert!(
        left_sep > 12.0,
        "left channel separation only {left_sep:.1} dB (1000 Hz bleeding into the left)"
    );
    assert!(
        right_sep > 12.0,
        "right channel separation only {right_sep:.1} dB (450 Hz bleeding into the right)"
    );
}

// ---------------------------------------------------------------------------
//  Measurements that inform the implementation
// ---------------------------------------------------------------------------

/// **Measurement, not just pass/fail.** Prints channel separation across bitrates
/// and channel-coding settings, so the stereo recommendation in `context.md` §14b
/// rests on numbers from this `libopus` rather than on theory.
///
/// Read the table with:
/// ```sh
/// cargo test -p mumble-protocol --features opus-codec --test opus_channels \
///     separation_table -- --nocapture
/// ```
///
/// ⚠️ **Sanity-check the shape of the table, not just the assertion.** Separation
/// should be non-decreasing with bitrate. If it is not, suspect the harness
/// before suspecting Opus — that is precisely how the predictive-decode bug
/// described at the top of this file was caught.
#[test]
fn separation_table_across_bitrate_and_channel_forcing() {
    println!(
        "\n  stereo channel separation, two uncorrelated tones \
         ({TONE_LEFT:.0} Hz L / {TONE_RIGHT:.0} Hz R)\n"
    );
    println!("  bitrate  force    phase-inv  left dB  right dB");
    println!("  -------  -------  ---------  -------  --------");

    let mut best = f32::MIN;
    let mut by_bitrate: Vec<(i32, f32)> = Vec::new();
    for bitrate in [48_000, 72_000, 96_000, 128_000, 160_000] {
        let mut best_for_bitrate = f32::MIN;
        for force in [ForceChannels::Auto, ForceChannels::Stereo] {
            for no_phase_inv in [false, true] {
                let (l, r) = measure_separation(bitrate, force, no_phase_inv);
                println!(
                    "  {:>7}  {:<7}  {:<9}  {:>7.1}  {:>8.1}",
                    bitrate,
                    if matches!(force, ForceChannels::Stereo) {
                        "stereo"
                    } else {
                        "auto"
                    },
                    if no_phase_inv { "disabled" } else { "on" },
                    l,
                    r
                );
                let worst_channel = l.min(r);
                best = best.max(worst_channel);
                best_for_bitrate = best_for_bitrate.max(worst_channel);
            }
        }
        by_bitrate.push((bitrate, best_for_bitrate));
    }
    println!();

    assert!(
        best > 15.0,
        "no tested configuration reached usable channel separation (best worst-case \
         channel was {best:.1} dB). Two independent receivers cannot be carried."
    );

    // Guard the harness, not the codec: more bits must not mean worse separation.
    // A generous 3 dB tolerance absorbs ordinary codec-decision noise while still
    // catching a methodology error, which shows up as a large inversion.
    for pair in by_bitrate.windows(2) {
        let (lo_rate, lo_sep) = pair[0];
        let (hi_rate, hi_sep) = pair[1];
        assert!(
            hi_sep > lo_sep - 3.0,
            "separation fell from {lo_sep:.1} dB at {lo_rate} bps to {hi_sep:.1} dB at \
             {hi_rate} bps. More bits cannot reduce channel separation — distrust the \
             harness before the codec (see the methodology note in this file)."
        );
    }
}

/// **The honest characterisation of the current default.** 72 kbps was chosen for
/// mono speech, and Opus couples stereo channels on the assumption they are
/// related — so two independent receivers are its worst case.
///
/// Asserts the *recommended* stereo configuration is materially better than the
/// mono default, which is the claim the implementation plan rests on. Deliberately
/// a **relative** assertion so it survives `libopus` retuning; absolute figures
/// live in the table test.
#[test]
fn the_mono_default_bitrate_is_not_enough_for_two_receivers() {
    let (default_l, default_r) = measure_separation(72_000, ForceChannels::Auto, false);
    let (good_l, good_r) =
        measure_separation(RECOMMENDED_STEREO_BITRATE, ForceChannels::Stereo, false);

    let default_worst = default_l.min(default_r);
    let good_worst = good_l.min(good_r);

    assert!(
        good_worst > default_worst,
        "the recommended stereo config ({RECOMMENDED_STEREO_BITRATE} bps, forced \
         stereo) should beat the mono default, but measured {good_worst:.1} dB vs \
         {default_worst:.1} dB"
    );
    assert!(
        good_worst > 12.0,
        "recommended stereo config only reached {good_worst:.1} dB separation"
    );
}

/// **Forcing stereo coding must not damage a mono *source*.** The Station will pin
/// `ForceChannels::Stereo` so a squelched sub-receiver cannot make the encoder
/// collapse the stream. This checks the cost when both channels legitimately carry
/// the same thing — the common case of one receiver fed to both ears.
///
/// ⚠️ **What is still untested.** `ForceChannels` made no measurable difference at
/// any bitrate in the table above, and that is expected rather than reassuring:
/// hard-panned uncorrelated tones never tempt Opus to collapse to mono, so the
/// signal does not exercise the control at all. The scenario it exists for is
/// *transitional* — a squelched sub-receiver making both channels identical for a
/// while, then opening. Proving it needs a signal that starts correlated and
/// diverges, then measures how promptly the divergence appears. Until that test
/// exists, treat `ForceChannels::Stereo` as a reasoned precaution, **not** as a
/// measured benefit.
#[test]
fn forcing_stereo_is_safe_when_both_channels_are_identical() {
    let total = FRAME * TOTAL_FRAMES;
    let signal = sine(TONE_LEFT, 0.5, total);
    let build = move |i: usize| {
        let lo = i * FRAME;
        stereo_frame(&signal[lo..lo + FRAME], &signal[lo..lo + FRAME], i as u64)
    };

    let mut enc = stereo_encoder(RECOMMENDED_STEREO_BITRATE, ForceChannels::Stereo, false);
    let packets = encode_run(&mut enc, build);
    let mut dec = OpusDecoder::new(STEREO_48KHZ_F32).expect("stereo decoder");
    let (left, right) = deinterleave(&decode_run_tail(&mut dec, &packets));

    assert!(
        rms(&left) > 0.05 && rms(&right) > 0.05,
        "forced-stereo coding of a duplicated mono source went quiet: \
         L rms {:.4}, R rms {:.4}",
        rms(&left),
        rms(&right)
    );

    let tone_l = magnitude_at(&left, TONE_LEFT);
    let tone_r = magnitude_at(&right, TONE_LEFT);
    let imbalance = separation_db(tone_l, tone_r).abs();
    assert!(
        imbalance < 3.0,
        "identical input channels came back {imbalance:.1} dB apart"
    );
}

/// **Protecting the downmix every other client hears.** Opus uses phase inversion
/// when coupling stereo channels, which can partially cancel on downmix to mono.
/// Since Mumble has no stereo support, *every stock client hears a mono downmix*
/// of whatever we send — so cancellation there is not a corner case, it is the
/// normal listening experience for everyone else in the channel.
///
/// Reports the downmix level with and without phase inversion. Asserts only that
/// neither collapses, because whether disabling it helps depends on how correlated
/// the content is, and hard-panned tones are the least coupled case there is.
///
/// ⚠️ Measured result: **identical** downmix level either way (rms 0.2501 for
/// both) with this signal — i.e. this test does not currently demonstrate any
/// benefit, because there is almost no coupling to invert. Like `ForceChannels`,
/// the setting is retained on documented Opus behaviour rather than on evidence
/// from here; correlated content is needed to show its effect.
#[test]
fn phase_inversion_setting_keeps_the_mono_downmix_intact() {
    let downmix_rms = |no_phase_inv: bool| {
        let mut enc =
            stereo_encoder(RECOMMENDED_STEREO_BITRATE, ForceChannels::Stereo, no_phase_inv);
        let packets = encode_run(&mut enc, stereo_two_tone_builder());
        let mut dec = OpusDecoder::new(AudioFormat::MONO_48KHZ_F32).expect("mono decoder");
        rms(&decode_run_tail(&mut dec, &packets))
    };

    let with_inversion = downmix_rms(false);
    let without_inversion = downmix_rms(true);
    println!(
        "\n  mono downmix rms: phase inversion on {with_inversion:.4}, \
         disabled {without_inversion:.4}\n"
    );

    assert!(
        with_inversion > 0.01 && without_inversion > 0.01,
        "a mono downmix must never be near-silent: on {with_inversion:.4}, \
         disabled {without_inversion:.4}"
    );
}

/// **Guards a whole class of bug.** `sample_count()` is per channel, so a 20 ms
/// frame is 960 samples whether mono or stereo — but its *byte* length doubles.
///
/// The inbound mixer is full of constants that conflate the two
/// (`FADE_LEN = 240`, `CROSSFADE_LEN = 24`, `MAX_SPEAKER_BUFFER_SAMPLES = 19_200`,
/// all documented in mono samples). Fed interleaved stereo they would cover half
/// the intended wall-clock time and apply unequal gain to left and right. Pinning
/// the distinction here states the invariant those constants must be converted to
/// respect.
#[test]
fn frame_duration_is_per_channel_but_byte_length_is_not() {
    let mono = mono_frame(&sine(TONE_LEFT, 0.5, FRAME), 0);
    let left = sine(TONE_LEFT, 0.5, FRAME);
    let right = sine(TONE_RIGHT, 0.5, FRAME);
    let stereo = stereo_frame(&left, &right, 0);

    assert_eq!(mono.sample_count(), FRAME);
    assert_eq!(stereo.sample_count(), FRAME);

    let tolerance = 1e-9;
    assert!((mono.duration_secs() - 0.020).abs() < tolerance);
    assert!(
        (stereo.duration_secs() - 0.020).abs() < tolerance,
        "a stereo frame of the same duration must not report twice the time; got {}",
        stereo.duration_secs()
    );

    assert_eq!(stereo.data.len(), mono.data.len() * 2);
}
