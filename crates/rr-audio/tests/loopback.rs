//! End-to-end audio quality harness.
//!
//! Two pure tones (left 1 kHz, right 1.5 kHz) go through the real capture
//! engine, a simulated network (delay, jitter bursts, loss, reordering) and the
//! real playout engine, in virtual time, with independent sender and receiver
//! clock errors. Quality is measured as THD+N per channel, compared with an
//! ideal-network baseline, plus crosstalk and discontinuity (pop) detection.
//! Run with `--nocapture` to see the numbers.

use std::f64::consts::PI;
use std::time::Duration;

use rr_audio::capture::{CaptureConfig, CaptureEngine, EncodedPacket, TxMode};
use rr_audio::playout::playout;
use rustfft::{num_complex::Complex, FftPlanner};

// Dependencies of the library that the harness does not use directly.
use opus as _;
use thiserror as _;
use tracing as _;

const FFT_N: usize = 16_384;
const L_HZ: f64 = 1_000.0;
const R_HZ: f64 = 1_500.0;
const L_AMP: f64 = 0.5;
const R_AMP: f64 = 0.3;

/// Small deterministic generator so runs are reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as f64) / f64::from(1_u32 << 31)
    }
}

#[derive(Clone, Copy)]
struct Net {
    base_s: f64,
    jitter_s: f64,
    burst_every_s: f64,
    burst_s: f64,
    loss: f64,
}

impl Net {
    const IDEAL: Self = Self { base_s: 0.03, jitter_s: 0.0, burst_every_s: 0.0, burst_s: 0.0, loss: 0.0 };
}

#[derive(Clone, Copy)]
struct Scenario {
    name: &'static str,
    cap_rate: u32,
    out_rate: u32,
    sender_ppm: f64,
    receiver_ppm: f64,
    net: Net,
    seconds: f64,
}

struct Report {
    name: &'static str,
    thd_l_worst: f64,
    thd_r_worst: f64,
    thd_l_median: f64,
    crosstalk_db: f64,
    max_step: f32,
    underruns: u64,
    overflows: u64,
    lost: u64,
    late: u64,
    depth_ms: f32,
    target_ms: f32,
    ratio_ppm: f32,
}

fn run(sc: Scenario) -> Report {
    let cfg = CaptureConfig {
        device_rate: sc.cap_rate,
        device_channels: 2,
        tx_mode: TxMode::Continuous,
        ..CaptureConfig::default()
    };
    let (mut cap, _ctl) = CaptureEngine::new(cfg).expect("capture");
    let (mut play, handle) = playout(sc.out_rate);
    let mut rng = Rng(0x1234_5678_9abc_def0);

    let cap_block = (sc.cap_rate / 100) as usize; // 10 ms
    let out_block = 256_usize;
    let cap_dt = cap_block as f64 / (f64::from(sc.cap_rate) * (1.0 + sc.sender_ppm * 1e-6));
    let out_dt = out_block as f64 / (f64::from(sc.out_rate) * (1.0 + sc.receiver_ppm * 1e-6));

    let mut inflight: Vec<(f64, EncodedPacket)> = Vec::new();
    let (mut left, mut right) = (Vec::new(), Vec::new());
    let (mut cap_n, mut cap_k, mut out_k) = (0_u64, 0_u64, 0_u64);
    // The first burst lands inside the warm-up so the estimator can learn from it.
    let mut next_burst = if sc.net.burst_every_s > 0.0 { 2.0 } else { 0.0 };
    let mut buf = vec![0.0_f32; out_block * 2];
    let mut packets = Vec::new();

    loop {
        let t_cap = cap_k as f64 * cap_dt;
        let t_out = out_k as f64 * out_dt;
        if t_out >= sc.seconds {
            break;
        }
        if t_cap <= t_out {
            let mut block = Vec::with_capacity(cap_block * 2);
            for _ in 0..cap_block {
                let n = cap_n as f64 / f64::from(sc.cap_rate);
                block.push((L_AMP * (2.0 * PI * L_HZ * n).sin()) as f32);
                block.push((R_AMP * (2.0 * PI * R_HZ * n).sin()) as f32);
                cap_n += 1;
            }
            packets.clear();
            cap.process(&block, &mut packets);
            for p in packets.drain(..) {
                if rng.next() < sc.net.loss && !p.terminator {
                    continue;
                }
                let mut d = sc.net.base_s + rng.next() * sc.net.jitter_s;
                if sc.net.burst_every_s > 0.0 && t_cap >= next_burst {
                    d += sc.net.burst_s;
                    next_burst += sc.net.burst_every_s;
                }
                inflight.push((t_cap + d, p));
            }
            cap_k += 1;
        } else {
            inflight.sort_by(|a, b| a.0.total_cmp(&b.0));
            let due = inflight.partition_point(|(at, _)| *at <= t_out);
            for (at, p) in inflight.drain(..due) {
                handle.push_at(1, p.seq, p.data, p.terminator, Duration::from_secs_f64(at));
            }
            play.render(&mut buf);
            for f in buf.chunks_exact(2) {
                left.push(f[0]);
                right.push(f[1]);
            }
            out_k += 1;
        }
    }
    analyse(&sc, &left, &right, &handle)
}

fn spectrum(x: &[f32]) -> Vec<f64> {
    let n = x.len();
    // 4-term Blackman-Harris window: very low side lobes.
    let mut buf: Vec<Complex<f64>> = x
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let a = 2.0 * PI * i as f64 / n as f64;
            let w = 0.35875 - 0.48829 * a.cos() + 0.14128 * (2.0 * a).cos() - 0.01168 * (3.0 * a).cos();
            Complex::new(f64::from(*s) * w, 0.0)
        })
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    buf.iter().take(n / 2).map(|c| c.norm_sqr()).collect()
}

fn band(p: &[f64], centre: usize, half: usize) -> f64 {
    p[centre.saturating_sub(half)..=(centre + half).min(p.len() - 1)].iter().sum()
}

/// Returns (THD+N in dB relative to the tone, power of `other_hz` relative to the tone in dB).
fn thd_n(x: &[f32], rate: u32, tone_hz: f64, other_hz: f64) -> (f64, f64) {
    let p = spectrum(x);
    let bin = |hz: f64| (hz * x.len() as f64 / f64::from(rate)).round() as usize;
    let (lo, hi) = (bin(tone_hz * 0.97), bin(tone_hz * 1.03));
    let k0 = (lo..=hi).max_by(|a, b| p[*a].total_cmp(&p[*b])).unwrap_or(lo);
    let sig = band(&p, k0, 5);
    let total: f64 = p[bin(20.0)..].iter().sum();
    let noise = (total - sig).max(1e-30);
    let other = band(&p, bin(other_hz), 5);
    (10.0 * (noise / sig).log10(), 10.0 * (other / sig).max(1e-30).log10())
}

fn analyse(sc: &Scenario, left: &[f32], right: &[f32], h: &rr_audio::playout::PlayoutHandle) -> Report {
    let skip = (sc.out_rate as usize) * 3;
    let (mut tl, mut tr, mut xt) = (Vec::new(), Vec::new(), Vec::new());
    let mut start = skip;
    while start + FFT_N <= left.len() {
        let (a, x) = thd_n(&left[start..start + FFT_N], sc.out_rate, L_HZ, R_HZ);
        let (b, _) = thd_n(&right[start..start + FFT_N], sc.out_rate, R_HZ, L_HZ);
        tl.push(a);
        tr.push(b);
        xt.push(x);
        start += FFT_N;
    }
    assert!(!tl.is_empty(), "scenario {} too short", sc.name);
    let mut sorted = tl.clone();
    sorted.sort_by(f64::total_cmp);
    let step = |c: &[f32]| c[skip..].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0_f32, f32::max);
    let s = h.stats().into_iter().next().unwrap_or_default();
    Report {
        name: sc.name,
        thd_l_worst: tl.iter().copied().fold(f64::MIN, f64::max),
        thd_r_worst: tr.iter().copied().fold(f64::MIN, f64::max),
        thd_l_median: sorted[sorted.len() / 2],
        crosstalk_db: xt.iter().copied().fold(f64::MIN, f64::max),
        max_step: step(left).max(step(right)),
        underruns: s.underruns,
        overflows: s.overflows,
        lost: s.lost,
        late: s.late,
        depth_ms: s.depth_ms,
        target_ms: s.target_ms,
        ratio_ppm: s.ratio_ppm,
    }
}

fn show(r: &Report) {
    println!(
        "{:<28} THD+N L worst {:6.1} dB (median {:6.1}) R worst {:6.1} dB | xtalk {:6.1} dB | max step {:.3} | \
         underrun {} overflow {} plc {} late {} | depth {:.0}/{:.0} ms, {:+.0} ppm",
        r.name, r.thd_l_worst, r.thd_l_median, r.thd_r_worst, r.crosstalk_db, r.max_step,
        r.underruns, r.overflows, r.lost, r.late, r.depth_ms, r.target_ms, r.ratio_ppm
    );
}

fn base(name: &'static str) -> Scenario {
    Scenario {
        name,
        cap_rate: 48_000,
        out_rate: 48_000,
        sender_ppm: 0.0,
        receiver_ppm: 0.0,
        net: Net::IDEAL,
        seconds: 30.0,
    }
}

#[test]
fn quality_under_real_world_conditions() {
    let ideal = run(base("ideal 48k/48k"));
    show(&ideal);
    // Tones through Opus: the codec itself must be clean enough to be transparent.
    assert!(ideal.thd_l_worst < -40.0, "codec baseline too noisy");
    assert!(ideal.max_step < 0.12, "pops in baseline: {}", ideal.max_step);
    assert_eq!((ideal.underruns, ideal.overflows, ideal.lost), (0, 0, 0));

    let jittery = Net { jitter_s: 0.06, burst_every_s: 7.0, burst_s: 0.15, ..Net::IDEAL };
    let cases = [
        Scenario { name: "44.1k capture -> 96k out", cap_rate: 44_100, out_rate: 96_000, ..base("") },
        Scenario { name: "96k capture -> 44.1k out", cap_rate: 96_000, out_rate: 44_100, ..base("") },
        Scenario { name: "jitter + bursts", net: jittery, ..base("") },
        Scenario { name: "clock +150/-150 ppm", sender_ppm: 150.0, receiver_ppm: -150.0, seconds: 60.0, ..base("") },
        Scenario { name: "clock -200/+200 ppm", sender_ppm: -200.0, receiver_ppm: 200.0, seconds: 60.0, ..base("") },
        Scenario {
            name: "everything at once",
            cap_rate: 44_100,
            out_rate: 48_000,
            sender_ppm: 120.0,
            receiver_ppm: -90.0,
            net: jittery,
            seconds: 60.0,
        },
    ];
    for mut c in cases {
        if c.name.is_empty() {
            c.name = "unnamed";
        }
        let r = run(c);
        show(&r);
        assert!(r.thd_l_worst < ideal.thd_l_worst + 3.0, "{}: left distorted", r.name);
        assert!(r.thd_r_worst < ideal.thd_r_worst + 3.0, "{}: right distorted", r.name);
        assert!(r.max_step < 0.12, "{}: pop {}", r.name, r.max_step);
        assert_eq!((r.underruns, r.overflows), (0, 0), "{}: buffer faults", r.name);
        let allowed = if c.net.burst_every_s > 0.0 { 6 } else { 0 };
        assert!(r.lost <= allowed, "{}: {} concealed frames", r.name, r.lost);
    }
}

#[test]
fn packet_loss_is_concealed_not_garbled() {
    let lossy = Net { loss: 0.02, jitter_s: 0.02, ..Net::IDEAL };
    let r = run(Scenario { name: "2% random loss", net: lossy, ..base("") });
    show(&r);
    assert!(r.lost > 0);
    assert!(r.thd_l_median < -35.0, "median {}", r.thd_l_median);
}
