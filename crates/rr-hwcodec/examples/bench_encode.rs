//! Where does hardware encode time actually go, at a real resolution?
//!
//! Captures a chosen monitor with DXGI and encodes it with the hardware encoder,
//! reporting the three phases separately:
//!
//! - **build** — colour conversion if needed, plus the copy into a Media Foundation
//!   buffer. At 3840x2160 that copy alone is 33 MiB.
//! - **submit** — the `ProcessInput` call.
//! - **drain** — pumping events and copying finished frames out.
//!
//! Attribution matters here for the same reason it did for capture: "encode is
//! slow" does not say whether to reach for a zero-copy GPU path, a different
//! encoder configuration, or neither.
//!
//! Run with:
//! ```text
//! cargo run --release -p rr-hwcodec --example bench_encode
//! cargo run --release -p rr-hwcodec --example bench_encode -- 2   # second monitor
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run()
}

#[cfg(not(windows))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("Windows only.");
    Err("hardware encoding is Windows-only".into())
}

#[cfg(windows)]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, Instant};

    use rr_capture::{CaptureError, DxgiCapturer, ScreenCapturer};
    use rr_hwcodec::{HwEncoder, HwEncoderSettings};

    /// Frames to measure once the pipeline is warm.
    const FRAMES: usize = 90;

    let pick = std::env::args()
        .nth(1)
        .and_then(|a| a.parse::<usize>().ok());
    let prefer_bgra = std::env::var("RR_PREFER_NV12").is_err();

    println!("Hardware encode phase breakdown");
    println!("===============================\n");

    let mut capturer = DxgiCapturer::new()?;
    let (w, h) = select_and_size(&mut capturer, pick)?;
    let mpx = f64::from(w) * f64::from(h) / 1e6;

    let mut encoder = HwEncoder::new(
        w,
        h,
        HwEncoderSettings {
            prefer_bgra,
            ..HwEncoderSettings::default()
        },
    )?;
    println!("\n  encoder  {}", encoder.info().name);
    println!("  input    {:?}", encoder.input_layout());
    println!("  async    {}", encoder.is_async());
    println!("  size     {w}x{h} ({mpx:.1} Mpx)\n");

    // Warm up: the first frames pay for pipeline fill and the one format
    // renegotiation, which would otherwise dominate a 90-frame mean.
    for _ in 0..10 {
        if let Ok(f) = capturer.grab() {
            let _ = encoder.encode(&f)?;
        }
    }

    let mut build = Duration::ZERO;
    let mut submit = Duration::ZERO;
    let mut drain = Duration::ZERO;
    let mut worst = Duration::ZERO;
    let mut capture = Duration::ZERO;
    let mut produced = 0usize;
    let mut bytes = 0usize;
    let mut measured = 0u32;

    let wall_start = Instant::now();
    while measured < FRAMES as u32 && wall_start.elapsed() < Duration::from_secs(30) {
        let t_cap = Instant::now();
        let frame = match capturer.grab() {
            Ok(f) => f,
            Err(CaptureError::NoNewFrame) => continue,
            Err(e) => return Err(e.into()),
        };
        let cap_wall = t_cap.elapsed();
        let wait = capturer.last_wait().unwrap_or(Duration::ZERO).min(cap_wall);
        capture += cap_wall - wait;

        let out = encoder.encode(&frame)?;
        let s = encoder.last_stats();
        build += s.build;
        submit += s.submit;
        drain += s.drain;
        worst = worst.max(s.total());
        if let Some(f) = out {
            produced += 1;
            bytes += f.data.len();
        }
        measured += 1;
    }
    let wall = wall_start.elapsed();
    let tail = encoder.flush()?;
    produced += tail.len();

    report(&Totals {
        measured,
        build,
        submit,
        drain,
        worst,
        capture,
        produced,
        bytes,
        wall,
    });
    Ok(())
}

/// List the monitors, select one, and return its encodable size.
///
/// Returns even dimensions, since H.264 4:2:0 cannot represent odd ones, and
/// takes the size from a real frame rather than the monitor's reported geometry so
/// scaling or rotation cannot surprise the encoder.
#[cfg(windows)]
fn select_and_size(
    capturer: &mut rr_capture::DxgiCapturer,
    pick: Option<usize>,
) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    use std::time::{Duration, Instant};

    use rr_capture::{CaptureError, ScreenCapturer};

    let monitors = capturer.monitors()?;
    for (i, m) in monitors.iter().enumerate() {
        println!(
            "  [{}] {:<22} {}x{} primary={}",
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

    let deadline = Instant::now();
    loop {
        match capturer.grab() {
            Ok(f) => return Ok((f.width & !1, f.height & !1)),
            Err(CaptureError::NoNewFrame) if deadline.elapsed() < Duration::from_secs(20) => {}
            Err(CaptureError::NoNewFrame) => return Err("screen never changed".into()),
            Err(e) => return Err(e.into()),
        }
    }
}

/// Accumulated measurements, so the reporting is separable from the measuring.
#[cfg(windows)]
struct Totals {
    /// Frames measured.
    measured: u32,
    /// Summed staging time.
    build: std::time::Duration,
    /// Summed `ProcessInput` time.
    submit: std::time::Duration,
    /// Summed drain time.
    drain: std::time::Duration,
    /// Worst single-frame encode total.
    worst: std::time::Duration,
    /// Summed capture work, excluding waits.
    capture: std::time::Duration,
    /// Encoded frames that came out.
    produced: usize,
    /// Total encoded bytes.
    bytes: usize,
    /// Wall-clock time for the measured run.
    wall: std::time::Duration,
}

#[cfg(windows)]
fn report(t: &Totals) {
    if t.measured == 0 {
        // Dividing by zero here produced a "sustainable 4.49e308 fps" line and a
        // confident "30 fps is met" verdict off no data at all. Say nothing instead.
        println!("No frames were delivered in {:.2?}.", t.wall);
        println!("The desktop never changed, so there was nothing to encode. That is");
        println!("the capture backend working as intended, not a failure — re-run with");
        println!("something moving on the selected display.");
        return;
    }
    let n = t.measured.max(1);
    let attributed = t.build + t.submit + t.drain;
    let total = attributed / n;
    let cap_mean = t.capture / n;
    let measured = t.measured;
    let worst = t.worst;
    let produced = t.produced;
    let wall = t.wall;

    println!("Phases, mean of {measured} frames");
    println!("--------------------------------");
    println!("  build  (convert + full-frame copy) {:>9.2?}", t.build / n);
    println!("  submit (ProcessInput)              {:>9.2?}", t.submit / n);
    println!("  drain  (events + output copy)      {:>9.2?}", t.drain / n);
    println!("  encode total                       {total:>9.2?}  worst {worst:.2?}");
    println!("  capture work (for scale)           {cap_mean:>9.2?}");

    println!("\nThroughput");
    println!("----------");
    let pipeline = cap_mean + total;
    let serialised = 1.0 / pipeline.as_secs_f64().max(f64::MIN_POSITIVE);
    // Capture and encode are independent stages, so running them on separate
    // threads bounds the rate by the slower one rather than their sum.
    let pipelined_ms = cap_mean.max(total).as_secs_f64();
    let pipelined = 1.0 / pipelined_ms.max(f64::MIN_POSITIVE);
    println!("  capture + encode per frame         {pipeline:.2?}");
    println!("  sustainable, serialised            {serialised:.1} fps");
    println!("  sustainable, pipelined             {pipelined:.1} fps  (max of the two stages)");
    println!(
        "  observed wall rate                 {:.1} fps over {wall:.2?}",
        f64::from(measured) / wall.as_secs_f64()
    );
    println!("  frames out                         {produced}");
    println!(
        "  bitrate                            {:.1} Mbit/s at 30 fps",
        (t.bytes as f64 * 8.0 * 30.0) / f64::from(n) / 1e6
    );

    println!("\nReading");
    println!("-------");
    println!("  Budget at 30 fps is 33.3 ms for everything.");
    let build_share = t.build.as_secs_f64() / attributed.as_secs_f64().max(f64::MIN_POSITIVE);
    println!("  Staging is {:.0}% of encode time.", build_share * 100.0);
    if build_share > 0.25 {
        println!("  -> Large enough to be worth removing: feeding the capture texture");
        println!("     straight to the encoder would eliminate both this copy and the");
        println!("     capture readback above it.");
    } else {
        println!("  -> Staging is not the problem; the encoder itself dominates.");
    }
    if serialised > 30.0 {
        println!("  30 fps is met at this resolution even serialised.");
    } else if pipelined > 30.0 {
        println!("  30 fps is met only if capture and encode run on separate threads.");
    } else {
        println!("  30 fps is NOT met even pipelined; the slower stage needs work.");
    }
}
