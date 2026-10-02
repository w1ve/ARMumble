//! Real-hardware validation for the capture → encode → decode pipeline.
//!
//! The synthetic source proves the pipeline's logic on headless CI, but it
//! cannot prove that **real screen capture** works: that needs a live display,
//! a real compositor and actual monitor geometry. This example is the bridge —
//! run it on the machine that will act as the Station.
//!
//! For each capture backend compiled in, it reports:
//!   1. every monitor the backend can enumerate (id, name, geometry, primary),
//!   2. a real capture from the primary monitor, with timing,
//!   3. that the frame is self-consistent and not uniformly blank,
//!   4. an H.264 encode of that frame (IDR, then a delta),
//!   5. a decode back to RGBA at the original geometry,
//!   6. a sustained capture→encode loop reporting per-stage latency.
//!
//! With both backends enabled it finishes with a side-by-side comparison, which
//! is the evidence for choosing one on Windows.
//!
//! Run with:
//! ```text
//! cargo run --release -p rr-codec --example capture_probe --features real-capture,dxgi-capture
//! ```
//!
//! Exits non-zero on failure so it can gate a release check.
//!
//! # Reading the numbers
//!
//! `xcap` re-reads the whole framebuffer on every call, so each call does the
//! same work and its timing is simply "cost per frame".
//!
//! DXGI Desktop Duplication only hands over a frame when the desktop actually
//! changed, so a stopwatch around `grab` would mostly measure *how still the
//! screen was*, not what a frame costs us. To keep the two comparable, this
//! probe subtracts [`ScreenCapturer::last_wait`] — the time the backend spent
//! blocked waiting for a change — from each timing. What is left is the work
//! that scales with resolution and caps the frame rate, which is the number
//! worth comparing. `xcap` reports no wait, so for it the two are the same.
//!
//! The wait itself is reported separately, and it is a feature rather than a
//! loss: a still screen costs the Station nothing.

// A probe binary legitimately reports to stdout and bails out on error.
#![allow(
    clippy::print_stdout,
    reason = "this is a diagnostic probe whose entire purpose is console output"
)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a probe should fail loudly with the underlying error"
)]
#![allow(
    unused_crate_dependencies,
    reason = "example uses a subset of the crate's dependencies"
)]

/// Whether the portable `xcap` backend is available.
const HAS_XCAP: bool = cfg!(feature = "real-capture");

/// Whether the Windows DXGI backend is available.
const HAS_DXGI: bool = cfg!(all(windows, feature = "dxgi-capture"));

#[cfg(any(feature = "real-capture", all(windows, feature = "dxgi-capture")))]
mod probe {
    use std::error::Error;
    use std::time::{Duration, Instant};

    use rr_capture::{CaptureError, Frame, MonitorInfo, ScreenCapturer};
    use rr_codec::{Decoder, EncoderSettings, VideoEncoder};

    /// Frames to collect in the sustained-throughput stage.
    const LOOP_FRAMES: usize = 60;

    /// Wall-clock cap on the sustained stage, so a completely still desktop
    /// cannot make a change-driven backend spin forever.
    const LOOP_DEADLINE: Duration = Duration::from_secs(10);

    /// How long a one-shot stage waits for the desktop to move.
    ///
    /// Generous on purpose: an idle machine with a static wallpaper genuinely
    /// produces no frames, and that is the backend working correctly, not a
    /// failure to diagnose.
    const ONESHOT_DEADLINE: Duration = Duration::from_secs(20);

    /// What one backend achieved, for the final comparison.
    pub(crate) struct Report {
        /// Backend label.
        pub(crate) label: &'static str,
        /// Mean per-frame capture work, excluding time spent waiting for the
        /// screen to change.
        pub(crate) capture_mean: Duration,
        /// Worst such measurement.
        pub(crate) capture_worst: Duration,
        /// Mean time spent blocked waiting for a change. Zero for `xcap`.
        pub(crate) wait_mean: Duration,
        /// Mean encode time.
        pub(crate) encode_mean: Duration,
        /// Frames actually measured.
        pub(crate) frames: usize,
        /// Calls that found no change (DXGI only; always 0 for `xcap`).
        pub(crate) no_change: usize,
        /// Encoded bits per second over the measured frames.
        pub(crate) mbps: f64,
    }

    impl Report {
        /// Frames per second this backend could sustain, derived from its own
        /// per-frame cost rather than from how often the test screen happened to
        /// change.
        pub(crate) fn sustainable_fps(&self) -> f64 {
            let per_frame = (self.capture_mean + self.encode_mean).as_secs_f64();
            if per_frame > 0.0 {
                1.0 / per_frame
            } else {
                f64::INFINITY
            }
        }
    }

    /// Grab, treating "nothing changed" as something to wait through rather
    /// than an error.
    ///
    /// Used for the one-shot stages, where the probe needs *a* frame and does
    /// not care how long the desktop took to move.
    fn grab_blocking(capturer: &mut dyn ScreenCapturer) -> Result<Frame, Box<dyn Error>> {
        let start = Instant::now();
        loop {
            match capturer.grab() {
                Ok(frame) => return Ok(frame),
                Err(CaptureError::NoNewFrame) => {
                    if start.elapsed() > ONESHOT_DEADLINE {
                        return Err("no frame arrived: the screen never changed".into());
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Stage 1: enumerate monitors and select one.
    ///
    /// `pick` is an optional **1-based index into the backend's own enumeration**,
    /// not a monitor id: ids are backend-specific (`xcap` reports Windows monitor
    /// handles, `DxgiCapturer` reports display numbers), so an index is the only
    /// selector that means the same thing to both. `None` selects the primary.
    fn stage_monitors(
        capturer: &mut dyn ScreenCapturer,
        pick: Option<usize>,
    ) -> Result<MonitorInfo, Box<dyn Error>> {
        println!("[1] Enumerating monitors");
        let monitors = capturer.monitors()?;
        if monitors.is_empty() {
            return Err("no monitors reported".into());
        }
        for (i, m) in monitors.iter().enumerate() {
            println!(
                "    [{}] id={:<10} {:<28} {:>5}x{:<5} primary={}",
                i + 1,
                m.id,
                m.name,
                m.width,
                m.height,
                m.primary
            );
        }
        let target = match pick {
            Some(n) => monitors
                .get(n.saturating_sub(1))
                .ok_or_else(|| format!("no monitor at index {n}; {} available", monitors.len()))?
                .clone(),
            None => monitors
                .iter()
                .find(|m| m.primary)
                .unwrap_or(&monitors[0])
                .clone(),
        };
        println!(
            "    -> using id={} ({}) {}x{}\n",
            target.id, target.name, target.width, target.height
        );
        capturer.select_monitor(target.id)?;
        Ok(target)
    }

    /// Stages 2 and 3: one capture, then prove it is a real picture.
    fn stage_capture(capturer: &mut dyn ScreenCapturer) -> Result<Frame, Box<dyn Error>> {
        println!("[2] Capturing one frame");
        let t0 = Instant::now();
        let frame = grab_blocking(capturer)?;
        println!(
            "    {}x{} stride={} format={:?} bytes={} seq={} in {:?}",
            frame.width,
            frame.height,
            frame.stride,
            frame.format,
            frame.data.len(),
            frame.sequence,
            t0.elapsed()
        );

        println!("\n[3] Validating the frame");
        frame.validate()?;
        println!("    geometry/buffer consistent");
        let packed = frame.width as usize * frame.format.bytes_per_pixel();
        if frame.stride != packed {
            println!(
                "    note: rows are padded ({} vs {packed} packed) - stride handling exercised",
                frame.stride
            );
        }
        // A capture that is a single flat colour usually means a permissions
        // problem or a blank/protected surface rather than a working grab.
        if is_uniform(&frame) {
            return Err("captured frame is a single uniform colour - capture likely blocked".into());
        }
        println!("    frame contains varied pixel data (not blank)");
        Ok(frame)
    }

    /// Encode until the encoder actually yields a frame.
    ///
    /// A hardware encoder is a pipeline: it can accept a frame and return nothing
    /// while it fills, with the output arriving a call or two later. The software
    /// encoder always answers immediately. This hides the difference from the
    /// one-shot stages, which just need *a* frame.
    fn encode_blocking(
        encoder: &mut VideoEncoder,
        capturer: &mut dyn ScreenCapturer,
        frame: &Frame,
        enc_w: u32,
        enc_h: u32,
    ) -> Result<rr_codec::EncodedFrame, Box<dyn Error>> {
        if let Some(out) = encoder.encode(&crop_to(frame, enc_w, enc_h))? {
            return Ok(out);
        }
        // Keep feeding fresh frames until something comes out.
        for _ in 0..16 {
            let next = grab_blocking(capturer)?;
            if let Some(out) = encoder.encode(&crop_to(&next, enc_w, enc_h))? {
                return Ok(out);
            }
        }
        Err("encoder accepted frames but produced no output".into())
    }

    /// Stages 4 and 5: encode an IDR and a delta, then decode the IDR back.
    fn stage_codec(
        capturer: &mut dyn ScreenCapturer,
        frame: &Frame,
    ) -> Result<(VideoEncoder, u32, u32), Box<dyn Error>> {
        // H.264 4:2:0 needs even dimensions; real monitors can be odd.
        let (enc_w, enc_h) = (frame.width & !1, frame.height & !1);
        println!("\n[4] Encoding H.264 at {enc_w}x{enc_h} from {:?}", frame.format);

        // `RR_FORCE_SOFTWARE=1` pins the software path, so the two can be compared
        // on the same machine without a rebuild.
        let mut encoder = if std::env::var("RR_FORCE_SOFTWARE").is_ok() {
            VideoEncoder::new_software(enc_w, enc_h, EncoderSettings::default())?
        } else {
            VideoEncoder::new(enc_w, enc_h, EncoderSettings::default())?
        };
        println!(
            "    encoder: {} (hardware: {})",
            encoder.name(),
            encoder.is_hardware()
        );
        // A viewer joining mid-stream needs an IDR, so ask for one explicitly
        // rather than relying on the encoder opening with one.
        encoder.request_keyframe();

        let t1 = Instant::now();
        let idr = encode_blocking(&mut encoder, capturer, frame, enc_w, enc_h)?;
        println!(
            "    IDR:   {} bytes keyframe={} in {:?}",
            idr.data.len(),
            idr.is_keyframe,
            t1.elapsed()
        );
        if !idr.is_keyframe {
            return Err("first encoded frame was not a keyframe".into());
        }

        let next = grab_blocking(capturer)?;
        let t2 = Instant::now();
        let delta = encode_blocking(&mut encoder, capturer, &next, enc_w, enc_h)?;
        println!(
            "    delta: {} bytes keyframe={} in {:?}",
            delta.data.len(),
            delta.is_keyframe,
            t2.elapsed()
        );

        println!("\n[5] Decoding");
        let mut decoder = Decoder::new()?;
        let t3 = Instant::now();
        let picture = decoder
            .decode(&idr.data)?
            .ok_or("IDR produced no picture")?;
        println!(
            "    {}x{} rgba={} bytes in {:?}",
            picture.width,
            picture.height,
            picture.rgba.len(),
            t3.elapsed()
        );
        if (picture.width, picture.height) != (enc_w, enc_h) {
            return Err(format!(
                "decoded geometry {}x{} does not match encoded {enc_w}x{enc_h}",
                picture.width, picture.height
            )
            .into());
        }
        Ok((encoder, enc_w, enc_h))
    }

    /// Stage 6: sustained capture + encode, which produces the headline numbers.
    fn stage_sustained(
        label: &'static str,
        capturer: &mut dyn ScreenCapturer,
        encoder: &mut VideoEncoder,
        enc_w: u32,
        enc_h: u32,
    ) -> Result<Report, Box<dyn Error>> {
        println!("\n[6] Sustained capture+encode, up to {LOOP_FRAMES} frames");
        let mut capture_total = Duration::ZERO;
        let mut encode_total = Duration::ZERO;
        let mut bytes_total = 0usize;
        let mut capture_worst = Duration::ZERO;
        let mut encode_worst = Duration::ZERO;
        let mut frames = 0usize;
        let mut no_change = 0usize;

        let mut wait_total = Duration::ZERO;
        let loop_start = Instant::now();
        while frames < LOOP_FRAMES && loop_start.elapsed() < LOOP_DEADLINE {
            let a = Instant::now();
            let f = match capturer.grab() {
                Ok(f) => f,
                // Nothing moved on screen. Not a frame, not a failure, and not
                // something to charge to the backend's per-frame cost.
                Err(CaptureError::NoNewFrame) => {
                    no_change += 1;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            // Charge the backend only for work, not for the user sitting still.
            let wall = a.elapsed();
            let wait = capturer.last_wait().unwrap_or(Duration::ZERO).min(wall);
            let g = wall - wait;
            wait_total += wait;

            let b = Instant::now();
            let produced = encoder.encode(&crop_to(&f, enc_w, enc_h))?;
            let enc = b.elapsed();

            // A hardware pipeline can swallow a frame and answer later. Charge the
            // submission time regardless — it is real cost — but only count bytes
            // for frames that actually came out.
            capture_total += g;
            encode_total += enc;
            if let Some(e) = produced {
                bytes_total += e.data.len();
            }
            capture_worst = capture_worst.max(g);
            encode_worst = encode_worst.max(enc);
            frames += 1;
        }
        let wall = loop_start.elapsed();

        if frames == 0 {
            return Err("no frames captured in the sustained stage".into());
        }
        let n = u32::try_from(frames).unwrap_or(1);
        let report = Report {
            label,
            capture_mean: capture_total / n,
            capture_worst,
            wait_mean: wait_total / n,
            encode_mean: encode_total / n,
            frames,
            no_change,
            mbps: (bytes_total as f64 * 8.0) / wall.as_secs_f64() / 1_000_000.0,
        };

        println!("    frames measured {frames}, no-change calls {no_change}, wall {wall:?}");
        println!(
            "    capture work avg {:?}  worst {capture_worst:?}",
            report.capture_mean
        );
        println!(
            "    waiting for change avg {:?} (not charged to capture)",
            report.wait_mean
        );
        println!(
            "    encode  avg {:?}  worst {encode_worst:?}",
            report.encode_mean
        );
        println!("    encoded {:.2} Mbit/s", report.mbps);
        println!(
            "    capture+encode avg {:?} -> sustainable {:.1} fps (33.3ms = 30 fps)",
            report.capture_mean + report.encode_mean,
            report.sustainable_fps()
        );
        Ok(report)
    }

    /// Run the full probe against one backend.
    ///
    /// # Errors
    /// Returns an error describing the first stage that failed.
    pub(crate) fn run_backend(
        label: &'static str,
        capturer: &mut dyn ScreenCapturer,
        pick: Option<usize>,
    ) -> Result<Report, Box<dyn Error>> {
        println!("================================================");
        println!("Backend: {label}");
        println!("================================================\n");

        let target = stage_monitors(capturer, pick)?;
        let _ = target;
        let frame = stage_capture(capturer)?;
        let (mut encoder, enc_w, enc_h) = stage_codec(capturer, &frame)?;
        let report = stage_sustained(label, capturer, &mut encoder, enc_w, enc_h)?;

        println!("\nRESULT: PASS - real capture, encode and decode all work with {label}.\n");
        Ok(report)
    }

    /// Print the side-by-side table and the headline speedup.
    pub(crate) fn print_comparison(reports: &[Report]) {
        let (Some(new), Some(old)) = (reports.first(), reports.last()) else {
            return;
        };
        println!("================================================");
        println!("Comparison");
        println!("================================================");
        println!(
            "{:<26} {:>7} {:>13} {:>12} {:>11} {:>8}",
            "backend", "frames", "capture work", "capture max", "encode avg", "fps"
        );
        for r in reports {
            println!(
                "{:<26} {:>7} {:>13} {:>12} {:>11} {:>8.1}",
                r.label,
                r.frames,
                format!("{:.2?}", r.capture_mean),
                format!("{:.2?}", r.capture_worst),
                format!("{:.2?}", r.encode_mean),
                r.sustainable_fps()
            );
        }

        // Callers push DXGI first, so `new` is the candidate and `old` is the
        // baseline it has to beat.
        let a = new.capture_mean.as_secs_f64();
        let b = old.capture_mean.as_secs_f64();
        if a > 0.0 {
            println!(
                "\n{} captures {:.1}x faster than {} ({:.2?} vs {:.2?} per frame).",
                new.label,
                b / a,
                old.label,
                new.capture_mean,
                old.capture_mean
            );
        }
        println!(
            "{} skipped {} calls where the screen had not changed; {} always pays full price.",
            new.label, new.no_change, old.label
        );
    }

    /// True when every pixel is identical, which indicates a blocked or blank
    /// grab rather than a successful capture.
    fn is_uniform(frame: &Frame) -> bool {
        let bpp = frame.format.bytes_per_pixel();
        let Some(first_row) = frame.row(0) else {
            return true;
        };
        let Some(first) = first_row.get(..bpp) else {
            return true;
        };
        // Sample a grid rather than every pixel: enough to catch a flat surface
        // without walking a 4K buffer.
        for y in (0..frame.height).step_by(16) {
            let Some(row) = frame.row(y) else { continue };
            for x in (0..frame.width).step_by(16) {
                let off = x as usize * bpp;
                // A pixel we cannot read is not evidence of variation, so only
                // a readable pixel that differs settles it.
                if row.get(off..off + bpp).is_some_and(|px| px != first) {
                    return false;
                }
            }
        }
        true
    }

    /// Crop a frame to `w` x `h`, preserving stride semantics.
    ///
    /// Real monitors can report odd dimensions, which H.264 4:2:0 cannot encode.
    ///
    /// Borrows when no crop is needed, which is the common case. Returning an owned
    /// `Frame` unconditionally meant cloning 33 MiB per frame at 4K *inside the
    /// timed region*, which inflated the measured encode cost by several
    /// milliseconds and would have been pure waste in a real pipeline too.
    fn crop_to(frame: &Frame, w: u32, h: u32) -> std::borrow::Cow<'_, Frame> {
        if frame.width == w && frame.height == h && frame.stride == w as usize * 4 {
            return std::borrow::Cow::Borrowed(frame);
        }
        let bpp = frame.format.bytes_per_pixel();
        let packed = w as usize * bpp;
        let mut data = Vec::with_capacity(packed * h as usize);
        for y in 0..h {
            if let Some(row) = frame.row(y) {
                data.extend_from_slice(row.get(..packed).unwrap_or(row));
            }
        }
        std::borrow::Cow::Owned(Frame {
            data,
            width: w,
            height: h,
            stride: packed,
            format: frame.format,
            captured_at: frame.captured_at,
            sequence: frame.sequence,
        })
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("RemoteRadio capture probe");
    println!("=========================\n");
    if !(HAS_XCAP || HAS_DXGI) {
        println!("This probe needs a real capture backend. Re-run with:");
        println!("  cargo run --release -p rr-codec --example capture_probe");
        println!("    --features real-capture,dxgi-capture");
        return Err("no capture backend enabled".into());
    }
    run()
}

/// Unreachable without a backend; `main` rejects that case first.
#[cfg(not(any(feature = "real-capture", all(windows, feature = "dxgi-capture"))))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    Err("no capture backend enabled".into())
}

/// Probe the DXGI backend.
///
/// It runs with a real acquire timeout so a still desktop makes the capture
/// thread *sleep* rather than spin — spinning would burn a core and distort the
/// encode timings measured in the same loop. The wait is then subtracted via
/// `last_wait`, so blocking costs the measurement nothing.
#[cfg(all(windows, feature = "dxgi-capture"))]
fn probe_dxgi(
    reports: &mut Vec<probe::Report>,
    pick: Option<usize>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut dxgi = rr_capture::DxgiCapturer::new()?;
    reports.push(probe::run_backend(
        "DXGI Desktop Duplication",
        &mut dxgi,
        pick,
    )?);
    Ok(())
}

/// No DXGI backend compiled in, so nothing to probe.
#[cfg(all(
    not(all(windows, feature = "dxgi-capture")),
    any(feature = "real-capture", all(windows, feature = "dxgi-capture"))
))]
fn probe_dxgi(
    _reports: &mut Vec<probe::Report>,
    _pick: Option<usize>,
) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

/// Probe the portable `xcap` backend.
#[cfg(feature = "real-capture")]
fn probe_xcap(
    reports: &mut Vec<probe::Report>,
    pick: Option<usize>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut xcap = rr_capture::XcapCapturer::new();
    reports.push(probe::run_backend("xcap (GDI baseline)", &mut xcap, pick)?);
    Ok(())
}

/// No `xcap` backend compiled in, so nothing to probe.
#[cfg(all(
    not(feature = "real-capture"),
    any(feature = "real-capture", all(windows, feature = "dxgi-capture"))
))]
fn probe_xcap(
    _reports: &mut Vec<probe::Report>,
    _pick: Option<usize>,
) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

#[cfg(any(feature = "real-capture", all(windows, feature = "dxgi-capture")))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Optional 1-based monitor index, so a second display (often a different
    // resolution, which is what actually moves these numbers) can be measured
    // without recompiling.
    let pick = std::env::args()
        .nth(1)
        .and_then(|a| a.parse::<usize>().ok());
    if let Some(n) = pick {
        println!("Selecting monitor #{n} from each backend's enumeration.\n");
    }

    let mut reports = Vec::new();
    // DXGI first: it is the backend the Station will actually use on Windows,
    // and `print_comparison` reads the first entry as the candidate.
    probe_dxgi(&mut reports, pick)?;
    probe_xcap(&mut reports, pick)?;
    if reports.len() > 1 {
        probe::print_comparison(&reports);
    }
    Ok(())
}
