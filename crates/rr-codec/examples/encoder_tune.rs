//! Does the software encoder have cheap headroom left, and how does it scale?
//!
//! After the DXGI backend landed, capture costs ~3.9 ms at 1920x1200 and
//! **encode is the bottleneck**. Before reaching for hardware encoding — which
//! is a per-vendor project — it is worth knowing whether `OpenH264` is simply
//! configured badly. Two knobs are candidates and `rr-codec` sets neither:
//!
//! - `complexity` (`Low`/`Medium`/`High`): the direct speed/quality trade.
//! - `num_threads` (0 = auto, 1 = single, >1 = fixed): `OpenH264`'s internal
//!   threading, which parallelises across slices.
//!
//! This sweeps both against an identical frame sequence and reports encode time
//! *and* output size, because a speed-up that costs bitrate is not a win for a
//! bandwidth-limited remote desktop.
//!
//! # Two traps this harness exists to avoid
//!
//! 1. **`.complexity(Complexity::default())` is not the same as leaving
//!    `complexity` unset.** The Rust wrapper's `Default` is its own choice, not
//!    `OpenH264`'s default for screen content, and on a low-entropy frame it
//!    measured ~2.7x slower. `None` here means "never call the setter", which is
//!    what `rr-codec` does.
//! 2. **Workload dominates the result.** An early version changed a small,
//!    highly compressible block and produced ~300 byte frames; encode measured
//!    ~13 ms. The current high-entropy workload produces ~44 KB frames and
//!    measures ~8 ms — *faster*, because motion estimation gives up quickly when
//!    there is nothing to match. Never quote an encoder time without saying what
//!    it encoded.
//!
//! Run with:
//! ```text
//! cargo run --release -p rr-codec --example encoder_tune --features dxgi-capture
//! # optional 1-based monitor index, to see how cost scales with resolution:
//! cargo run --release -p rr-codec --example encoder_tune --features dxgi-capture -- 2
//! ```

#![allow(
    clippy::print_stdout,
    reason = "diagnostic benchmark whose purpose is console output"
)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a benchmark should fail loudly"
)]
#![allow(
    unused_crate_dependencies,
    reason = "example uses a subset of the crate's dependencies"
)]

#[cfg(all(windows, feature = "dxgi-capture"))]
mod tune {
    use std::error::Error;
    use std::time::{Duration, Instant};

    use openh264::encoder::{
        BitRate, Complexity, Encoder as OpenH264Encoder, EncoderConfig, FrameRate,
        IntraFramePeriod, RateControlMode, UsageType,
    };
    use openh264::formats::{BgraSliceU8, YUVBuffer};
    use openh264::OpenH264API;
    use rr_capture::{CaptureError, DxgiCapturer, Frame, ScreenCapturer};

    /// Frames per configuration. Enough for the encoder to settle into a steady
    /// state after its opening IDR.
    pub(crate) const FRAMES: usize = 24;

    /// How long to wait for the desktop to change before giving up.
    const GRAB_DEADLINE: Duration = Duration::from_secs(20);

    /// One measurement: mean encode time and mean encoded size.
    pub(crate) struct Measured {
        /// Mean time to convert and encode one frame.
        pub(crate) mean: Duration,
        /// Mean encoded bytes per frame.
        pub(crate) bytes: usize,
    }

    /// Grab one real frame, for realistic static content to build on.
    pub(crate) fn capture_base(pick: Option<usize>) -> Result<Frame, Box<dyn Error>> {
        let mut capturer = DxgiCapturer::new()?;
        let monitors = capturer.monitors()?;
        for (i, m) in monitors.iter().enumerate() {
            println!(
                "  [{}] {:<24} {}x{} primary={}",
                i + 1,
                m.name,
                m.width,
                m.height,
                m.primary
            );
        }
        let target = match pick {
            Some(n) => monitors
                .get(n.saturating_sub(1))
                .ok_or_else(|| format!("no monitor at index {n}"))?
                .clone(),
            None => monitors
                .iter()
                .find(|m| m.primary)
                .or_else(|| monitors.first())
                .ok_or("no monitors")?
                .clone(),
        };
        capturer.select_monitor(target.id)?;

        let start = Instant::now();
        loop {
            match capturer.grab() {
                Ok(f) => return Ok(f),
                Err(CaptureError::NoNewFrame) if start.elapsed() < GRAB_DEADLINE => {}
                Err(CaptureError::NoNewFrame) => return Err("screen never changed".into()),
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Write one BGRA pixel, ignoring an out-of-range offset.
    fn put_px(buf: &mut [u8], off: usize, b: u8, g: u8, r: u8) {
        if let Some(px) = buf.get_mut(off..off + 4) {
            px[0] = b;
            px[1] = g;
            px[2] = r;
            px[3] = 0xFF;
        }
    }

    /// Fill one frame's changing region with high-entropy content.
    fn paint_region(buf: &mut [u8], base: &Frame, seed: u32) {
        let bpp = 4usize;
        let block_w = (base.width as usize * 2 / 3).max(2);
        let block_h = (base.height as usize / 2).max(2);
        let x0 = (base.width as usize - block_w) / 2;
        let y0 = (base.height as usize - block_h) / 2;

        // Cheap deterministic xorshift32, seeded per frame so successive frames
        // genuinely differ.
        let mut state = 0x2545_F491_u32 ^ seed.wrapping_mul(0x9E37_79B9);
        for y in y0..y0 + block_h {
            let row = y * base.stride;
            for x in (x0..x0 + block_w).step_by(2) {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                // Two pixels per draw: enough entropy to defeat trivial
                // compression without the PRNG dominating the runtime.
                let [a, b, c, d] = state.to_le_bytes();
                put_px(buf, row + x * bpp, a, b, c);
                put_px(buf, row + (x + 1) * bpp, b, c, d);
            }
        }
    }

    /// Build the frame sequence: the real capture, then copies with a large
    /// high-entropy region, so every configuration sees byte-identical input.
    pub(crate) fn build_sequence(base: &Frame, count: usize) -> Vec<Vec<u8>> {
        (0..count)
            .map(|n| {
                let mut buf = base.data.clone();
                paint_region(&mut buf, base, n as u32);
                buf
            })
            .collect()
    }

    /// Repack a frame's rows tightly, as the encoder path does.
    fn pack_rows(data: &[u8], width: u32, height: u32, stride: usize) -> Vec<u8> {
        let packed_w = width as usize * 4;
        if stride == packed_w {
            return data.to_vec();
        }
        let mut out = Vec::with_capacity(packed_w * height as usize);
        for y in 0..height as usize {
            let s = y * stride;
            out.extend_from_slice(data.get(s..s + packed_w).unwrap_or(&[]));
        }
        out
    }

    /// Encode the whole sequence with one configuration.
    ///
    /// `complexity`/`threads` of `None` mean "do not call that setter at all",
    /// which is what `rr-codec` does and is **not** the same as passing the
    /// wrapper's `Default`.
    pub(crate) fn measure(
        label: &str,
        width: u32,
        height: u32,
        stride: usize,
        frames: &[Vec<u8>],
        complexity: Option<Complexity>,
        threads: Option<u16>,
    ) -> Result<Measured, Box<dyn Error>> {
        let mut config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .bitrate(BitRate::from_bps(8_000_000))
            .max_frame_rate(FrameRate::from_hz(30.0))
            .intra_frame_period(IntraFramePeriod::from_num_frames(0))
            .adaptive_quantization(false)
            .background_detection(false);
        if let Some(c) = complexity {
            config = config.complexity(c);
        }
        if let Some(t) = threads {
            config = config.num_threads(t);
        }

        let mut encoder = OpenH264Encoder::with_api_config(OpenH264API::from_source(), config)?;
        let mut yuv = YUVBuffer::new(width as usize, height as usize);
        let dims = (width as usize, height as usize);

        let mut total = Duration::ZERO;
        let mut bytes = 0usize;
        let mut counted = 0u32;

        for (i, data) in frames.iter().enumerate() {
            // Outside the timer: this is harness cost, not encoder cost.
            let packed = pack_rows(data, width, height, stride);

            let t = Instant::now();
            yuv.read_bgra8(BgraSliceU8::new(&packed, dims));
            let out = encoder.encode(&yuv)?;
            let elapsed = t.elapsed();
            let len = out.to_vec().len();

            // Skip the opening IDR: far more expensive than the steady state,
            // and it would dominate a 24-frame mean.
            if i > 0 {
                total += elapsed;
                bytes += len;
                counted += 1;
            }
        }

        let n = counted.max(1);
        let m = Measured {
            mean: total / n,
            bytes: bytes / n as usize,
        };
        if !label.is_empty() {
            let mbit = (m.bytes as f64 * 8.0 * 30.0) / 1_000_000.0;
            println!(
                "  {label:<30} encode mean {:>9.2?}   {:>8} B/frame  ({mbit:>6.1} Mbit/s @30fps)",
                m.mean, m.bytes
            );
        }
        Ok(m)
    }

    /// [`measure`] without the per-run print, for the interleaved sweep.
    fn measure_quiet(
        width: u32,
        height: u32,
        stride: usize,
        frames: &[Vec<u8>],
        complexity: Option<Complexity>,
        threads: Option<u16>,
    ) -> Result<Measured, Box<dyn Error>> {
        measure("", width, height, stride, frames, complexity, threads)
    }

    /// Rounds to run. Each round measures every configuration once, and the
    /// **minimum** across rounds is reported.
    ///
    /// This is not belt-and-braces: measuring each configuration once, in order,
    /// produced identical configurations differing by up to **4x** on a 4-core
    /// laptop, purely from whatever else the machine was doing during that
    /// second. Position in the run correlated far better than the setting did.
    /// Interleaving rounds and taking the minimum reports the machine's
    /// best-case for each setting, which is the only figure that is comparable
    /// between settings when a competing process can steal a core at any moment.
    const ROUNDS: usize = 3;

    /// One configuration under test.
    struct Config {
        /// Label for the output.
        label: &'static str,
        /// `None` means "never call `.complexity()`", as `rr-codec` does.
        complexity: Option<Complexity>,
        /// `None` means "never call `.num_threads()`", as `rr-codec` does.
        threads: Option<u16>,
    }

    /// Everything worth testing, in one table.
    fn configs() -> Vec<Config> {
        vec![
            Config {
                label: "shipped (both unset)",
                complexity: None,
                threads: None,
            },
            Config {
                label: "threads=1",
                complexity: None,
                threads: Some(1),
            },
            Config {
                label: "threads=auto(0)",
                complexity: None,
                threads: Some(0),
            },
            Config {
                label: "threads=2",
                complexity: None,
                threads: Some(2),
            },
            Config {
                label: "threads=4",
                complexity: None,
                threads: Some(4),
            },
            Config {
                label: "threads=8",
                complexity: None,
                threads: Some(8),
            },
            Config {
                label: "complexity=Low",
                complexity: Some(Complexity::Low),
                threads: None,
            },
            Config {
                label: "complexity=Medium",
                complexity: Some(Complexity::Medium),
                threads: None,
            },
            Config {
                label: "complexity=High",
                complexity: Some(Complexity::High),
                threads: None,
            },
            Config {
                label: "complexity=Default::default()",
                complexity: Some(Complexity::default()),
                threads: None,
            },
        ]
    }

    /// Run every configuration `ROUNDS` times, interleaved, keeping the best.
    ///
    /// Returns the results in the same order as [`configs`], so index 0 is the
    /// shipped baseline.
    pub(crate) fn sweep(
        w: u32,
        h: u32,
        stride: usize,
        frames: &[Vec<u8>],
    ) -> Result<Vec<(&'static str, Measured)>, Box<dyn Error>> {
        let cfgs = configs();
        let mut best: Vec<Option<Measured>> = (0..cfgs.len()).map(|_| None).collect();

        for round in 0..ROUNDS {
            println!("  round {}/{ROUNDS}", round + 1);
            for (slot, c) in best.iter_mut().zip(cfgs.iter()) {
                let m = measure_quiet(w, h, stride, frames, c.complexity, c.threads)?;
                // Keep the fastest observation: it is the one least polluted by
                // another process stealing a core mid-measurement.
                if slot.as_ref().is_none_or(|b| m.mean < b.mean) {
                    *slot = Some(m);
                }
            }
        }

        let mut out = Vec::with_capacity(cfgs.len());
        for (c, b) in cfgs.iter().zip(best) {
            let m = b.ok_or("a configuration produced no measurement")?;
            let mbit = (m.bytes as f64 * 8.0 * 30.0) / 1_000_000.0;
            println!(
                "  {:<30} best {:>9.2?}   {:>8} B/frame  ({mbit:>6.1} Mbit/s @30fps)",
                c.label, m.mean, m.bytes
            );
            out.push((c.label, m));
        }
        Ok(out)
    }

    /// Print the verdict.
    pub(crate) fn analyse(results: &[(&str, Measured)], px: u64) {
        println!("\nAnalysis");
        println!("--------");
        let Some((_, shipped)) = results.first() else {
            return;
        };
        let mpx = (px as f64 / 1e6).max(f64::MIN_POSITIVE);
        let fps = 1.0 / shipped.mean.as_secs_f64().max(f64::MIN_POSITIVE);
        println!(
            "  Shipped: {:.2?}/frame over {mpx:.1} Mpx -> {fps:.0} fps if encode were the only cost.",
            shipped.mean
        );
        println!(
            "  Per megapixel: {:.2} ms/Mpx",
            shipped.mean.as_secs_f64() * 1000.0 / mpx
        );

        // Is anything meaningfully better than what we ship?
        let base = shipped.mean.as_secs_f64();
        let mut winner: Option<(&str, f64, f64)> = None;
        for (label, m) in results.iter().skip(1) {
            let speedup = base / m.mean.as_secs_f64().max(f64::MIN_POSITIVE);
            let size = m.bytes as f64 / shipped.bytes.max(1) as f64;
            if speedup > 1.15 && size < 1.15 {
                let better = winner.is_none_or(|(_, s, _)| speedup > s);
                if better {
                    winner = Some((label, speedup, size));
                }
            }
        }
        match winner {
            Some((label, speedup, size)) => {
                println!(
                    "  Best alternative: {label} at {speedup:.2}x the speed for {size:.2}x bytes."
                );
                println!("  -> Worth setting. Re-run to confirm it is not noise.");
            }
            None => {
                println!("  Nothing beats the shipped config by >15% without costing bitrate.");
                println!("  -> Neither complexity nor threading is a lever here. The remaining");
                println!("     options are hardware encoding, or encoding fewer pixels.");
            }
        }
        println!("  NOTE: these are best-of-{ROUNDS} figures. If two rounds of the same");
        println!("  configuration still disagree widely, the machine is too busy to trust;");
        println!("  the capture_probe example measures real screen content end to end and is");
        println!("  the better instrument for absolute numbers.");
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run()
}

#[cfg(not(all(windows, feature = "dxgi-capture")))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("Needs a real capture backend. Re-run with:");
    println!("  cargo run --release -p rr-codec --example encoder_tune --features dxgi-capture");
    Err("dxgi-capture feature not enabled".into())
}

#[cfg(all(windows, feature = "dxgi-capture"))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("OpenH264 configuration sweep");
    println!("============================\n");

    let pick = std::env::args()
        .nth(1)
        .and_then(|a| a.parse::<usize>().ok());
    let base = tune::capture_base(pick)?;

    // H.264 4:2:0 needs even dimensions.
    let (w, h) = (base.width & !1, base.height & !1);
    let px = u64::from(w) * u64::from(h);
    println!(
        "\nSource: real capture {}x{} {:?}, encoding {w}x{h} ({:.1} Mpx)\n",
        base.width,
        base.height,
        base.format,
        px as f64 / 1e6
    );

    let frames = tune::build_sequence(&base, tune::FRAMES);

    println!("Sweeping configurations, interleaved, best-of-3:");
    let results = tune::sweep(w, h, base.stride, &frames)?;
    tune::analyse(&results, px);
    Ok(())
}
