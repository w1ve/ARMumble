//! Real-hardware validation for the capture → encode → decode pipeline.
//!
//! The synthetic source proves the pipeline's logic on headless CI, but it
//! cannot prove that **real screen capture** works: that needs a live display,
//! a real compositor and actual monitor geometry. This example is the bridge —
//! run it on the machine that will act as the Station.
//!
//! It reports, in order:
//!   1. every monitor `xcap` can enumerate (id, name, geometry, primary flag),
//!   2. a real capture from the primary monitor, with timing,
//!   3. that the frame is self-consistent and not uniformly blank,
//!   4. an H.264 encode of that frame (IDR, then a delta),
//!   5. a decode back to RGBA at the original geometry,
//!   6. a sustained capture→encode loop reporting per-stage latency, which is
//!      the baseline the Windows fast path (`DXGI`/`WGC`) must beat later.
//!
//! Run with:
//! ```text
//! cargo run -p rr-codec --example capture_probe --features rr-capture/screen-capture
//! ```
//!
//! Exits non-zero on failure so it can gate a release check.

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

// These are only reachable when a real capture backend is compiled in.
#[cfg(feature = "real-capture")]
use std::time::{Duration, Instant};

#[cfg(feature = "real-capture")]
use rr_capture::{Frame, ScreenCapturer};
#[cfg(feature = "real-capture")]
use rr_codec::{Decoder, Encoder, EncoderSettings};

/// Frames to run in the sustained-throughput stage.
#[cfg(feature = "real-capture")]
const LOOP_FRAMES: usize = 60;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("RemoteRadio capture probe");
    println!("=========================\n");
    run()
}

/// Without the `real-capture` feature there is no backend to probe, so explain
/// how to enable it rather than silently reporting success.
#[cfg(not(feature = "real-capture"))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("This probe needs real screen capture enabled. Re-run with:");
    println!("  cargo run -p rr-codec --example capture_probe --features real-capture");
    Err("real-capture feature not enabled".into())
}

#[cfg(feature = "real-capture")]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use rr_capture::XcapCapturer;

    // ---- 1. Monitor enumeration -------------------------------------
    println!("[1] Enumerating monitors");
    let mut capturer = XcapCapturer::new();
    let monitors = capturer.monitors()?;
    if monitors.is_empty() {
        return Err("no monitors reported".into());
    }
    for m in &monitors {
        println!(
            "    id={:<10} {:<28} {:>5}x{:<5} primary={}",
            m.id, m.name, m.width, m.height, m.primary
        );
    }
    let target = monitors
        .iter()
        .find(|m| m.primary)
        .unwrap_or(&monitors[0])
        .clone();
    println!("    -> using monitor id={} ({})\n", target.id, target.name);
    capturer.select_monitor(target.id)?;

    // ---- 2. One real capture ----------------------------------------
    println!("[2] Capturing one frame");
    let t0 = Instant::now();
    let frame = capturer.grab()?;
    let grab_time = t0.elapsed();
    println!(
        "    {}x{} stride={} format={:?} bytes={} in {:?}",
        frame.width,
        frame.height,
        frame.stride,
        frame.format,
        frame.data.len(),
        grab_time
    );

    // ---- 3. Sanity-check the pixels ---------------------------------
    println!("\n[3] Validating the frame");
    frame.validate()?;
    println!("    geometry/buffer consistent");
    if frame.stride != frame.width as usize * frame.format.bytes_per_pixel() {
        println!(
            "    note: rows are padded ({} vs {} packed) - stride handling exercised",
            frame.stride,
            frame.width as usize * frame.format.bytes_per_pixel()
        );
    }
    // A capture that is a single flat colour usually means a permissions
    // problem or a blank/protected surface rather than a working grab.
    if is_uniform(&frame) {
        return Err("captured frame is a single uniform colour - capture likely blocked".into());
    }
    println!("    frame contains varied pixel data (not blank)");

    // ---- 4. Encode --------------------------------------------------
    // H.264 4:2:0 needs even dimensions; real monitors can be odd.
    let (enc_w, enc_h) = (frame.width & !1, frame.height & !1);
    println!("\n[4] Encoding H.264 at {enc_w}x{enc_h}");
    let mut encoder = Encoder::new(enc_w, enc_h, EncoderSettings::default())?;
    let cropped = crop_to(&frame, enc_w, enc_h);

    let t1 = Instant::now();
    let idr = encoder.encode(&cropped)?;
    let encode_idr = t1.elapsed();
    println!(
        "    IDR:   {} bytes keyframe={} in {:?}",
        idr.data.len(),
        idr.is_keyframe,
        encode_idr
    );
    if !idr.is_keyframe {
        return Err("first encoded frame was not a keyframe".into());
    }

    let next = capturer.grab()?;
    let t2 = Instant::now();
    let delta = encoder.encode(&crop_to(&next, enc_w, enc_h))?;
    let encode_delta = t2.elapsed();
    println!(
        "    delta: {} bytes keyframe={} in {:?}",
        delta.data.len(),
        delta.is_keyframe,
        encode_delta
    );

    // ---- 5. Decode --------------------------------------------------
    println!("\n[5] Decoding");
    let mut decoder = Decoder::new()?;
    let t3 = Instant::now();
    let picture = decoder
        .decode(&idr.data)?
        .ok_or("IDR produced no picture")?;
    let decode_time = t3.elapsed();
    println!(
        "    {}x{} rgba={} bytes in {:?}",
        picture.width,
        picture.height,
        picture.rgba.len(),
        decode_time
    );
    if (picture.width, picture.height) != (enc_w, enc_h) {
        return Err(format!(
            "decoded geometry {}x{} does not match encoded {enc_w}x{enc_h}",
            picture.width, picture.height
        )
        .into());
    }

    // ---- 6. Sustained loop (the P7 baseline) ------------------------
    println!("\n[6] Sustained capture+encode, {LOOP_FRAMES} frames");
    let mut grab_total = Duration::ZERO;
    let mut encode_total = Duration::ZERO;
    let mut bytes_total = 0usize;
    let mut grab_worst = Duration::ZERO;
    let mut encode_worst = Duration::ZERO;

    let loop_start = Instant::now();
    for _ in 0..LOOP_FRAMES {
        let a = Instant::now();
        let f = capturer.grab()?;
        let g = a.elapsed();

        let b = Instant::now();
        let e = encoder.encode(&crop_to(&f, enc_w, enc_h))?;
        let enc = b.elapsed();

        grab_total += g;
        encode_total += enc;
        bytes_total += e.data.len();
        grab_worst = grab_worst.max(g);
        encode_worst = encode_worst.max(enc);
    }
    let wall = loop_start.elapsed();

    let n = u32::try_from(LOOP_FRAMES).unwrap_or(1);
    let fps = LOOP_FRAMES as f64 / wall.as_secs_f64();
    let mbps = (bytes_total as f64 * 8.0) / wall.as_secs_f64() / 1_000_000.0;
    println!("    capture avg {:?}  worst {:?}", grab_total / n, grab_worst);
    println!(
        "    encode  avg {:?}  worst {:?}",
        encode_total / n,
        encode_worst
    );
    println!("    throughput {fps:.1} fps, {mbps:.2} Mbit/s encoded");
    println!(
        "    per-frame budget at 30 fps is 33.3ms; capture+encode avg = {:?}",
        (grab_total + encode_total) / n
    );

    println!("\nRESULT: PASS - real capture, encode and decode all work here.");
    println!("Record the timings above as the pre-optimisation baseline (P7).");
    Ok(())
}

/// True when every pixel is identical, which indicates a blocked or blank grab
/// rather than a successful capture.
#[cfg(feature = "real-capture")]
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
            if let Some(px) = row.get(off..off + bpp) {
                if px != first {
                    return false;
                }
            }
        }
    }
    true
}

/// Crop a frame to `w` x `h`, preserving stride semantics.
///
/// Real monitors can report odd dimensions, which H.264 4:2:0 cannot encode.
#[cfg(feature = "real-capture")]
fn crop_to(frame: &Frame, w: u32, h: u32) -> Frame {
    if frame.width == w && frame.height == h {
        return frame.clone();
    }
    let bpp = frame.format.bytes_per_pixel();
    let packed = w as usize * bpp;
    let mut data = Vec::with_capacity(packed * h as usize);
    for y in 0..h {
        if let Some(row) = frame.row(y) {
            data.extend_from_slice(row.get(..packed).unwrap_or(row));
        }
    }
    Frame {
        data,
        width: w,
        height: h,
        stride: packed,
        format: frame.format,
        captured_at: frame.captured_at,
        sequence: frame.sequence,
    }
}
