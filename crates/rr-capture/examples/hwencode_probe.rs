//! Can this machine's GPU encode H.264 fast enough for 4K remote desktop?
//!
//! Software `OpenH264` measured 56–114 ms per 3840x2160 frame on a 4-core mobile
//! i7 — 7.5–13.8 fps once capture is added, against a 30 fps target. Sweeping the
//! encoder's own settings moved that by under 6%, so the remaining options are
//! hardware encoding or encoding fewer pixels. This probe answers the first
//! question **before** any of it is designed in: *is the fixed-function encoder
//! on this class of machine fast enough?*
//!
//! # What it measures, and what it does not
//!
//! It pushes real captured frames through Media Foundation's H.264 encoder, which
//! dispatches to whatever hardware the machine has — Intel Quick Sync, NVIDIA
//! NVENC, AMD VCN — behind one API, and falls back to a software MFT if there is
//! none. Sustained throughput is the answer: wall-clock time for N frames
//! including the drain on `finish`.
//!
//! This is a **capability probe, not a prototype**. It writes an MP4, because
//! `windows-capture`'s encoder only offers containers and a real low-latency
//! path needs an H.264 elementary stream straight from the MFT. Container
//! muxing and file I/O are counted here. Nothing here is a design proposal.
//!
//! # Important: this path does NOT reach the hardware
//!
//! Measured on an i7-1165G7 with Iris Xe: this probe reports ~51 fps at 4K, but
//! the GPU's media engine stays at **0%** while the CPU runs 37–73%. Media
//! Foundation silently selected a *software* H.264 MFT.
//!
//! The reason is the input: `send_frame_buffer` hands over a CPU buffer, and the
//! sink writer is not configured with a D3D device manager, so a hardware MFT is
//! never eligible. `windows-capture`'s encoder can only otherwise be fed its own
//! WGC `Frame`, which is no use to a DXGI-duplication capturer.
//!
//! So this probe's number is a **software** baseline reached through a different
//! code path, and is kept only as a record of that dead end. The hardware
//! capability of this machine was established separately with `ffmpeg
//! -c:v h264_qsv`: **70–85 fps at 3840x2160**, with Intel's media engine at 62.7%.
//! Reaching that from our own code needs the Media Foundation H.264 MFT driven
//! with a D3D device manager — which is implementation work, not a probe.
//!
//! Note for anyone verifying hardware use on an Intel iGPU: the media block
//! reports under `engtype_videodecode`, **not** `engtype_VideoEncode`. Watching
//! the latter shows 0% even when Quick Sync is fully busy.
//!
//! It lives in `rr-capture` rather than `rr-codec` purely because the
//! `windows-capture` dependency is already here; moving it would mean adding that
//! dependency to a second crate for a throwaway measurement.
//!
//! Run with:
//! ```text
//! cargo run --release -p rr-capture --example hwencode_probe --features dxgi-capture
//! # optional 1-based monitor index, e.g. a 4K second display:
//! cargo run --release -p rr-capture --example hwencode_probe --features dxgi-capture -- 2
//! ```

#![allow(
    clippy::print_stdout,
    reason = "diagnostic probe whose purpose is console output"
)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a probe should fail loudly"
)]
#![allow(
    unused_crate_dependencies,
    reason = "example uses a subset of the crate's dependencies"
)]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run()
}

#[cfg(not(all(windows, feature = "dxgi-capture")))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("Windows only. Re-run with:");
    println!("  cargo run --release -p rr-capture --example hwencode_probe --features dxgi-capture");
    Err("dxgi-capture feature not enabled".into())
}

#[cfg(all(windows, feature = "dxgi-capture"))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, Instant};

    use rr_capture::{CaptureError, DxgiCapturer, ScreenCapturer};
    use windows_capture::encoder::{
        AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder,
        VideoSettingsSubType,
    };

    /// Frames to push. Enough to amortise encoder start-up and see a steady rate.
    const FRAMES: usize = 180;

    /// Distinct source frames to cycle through. Pushing one identical frame over
    /// and over would let the encoder emit near-empty deltas and flatter the
    /// result; a handful of genuinely different frames does not, and at 4K eight
    /// of them is already 265 MB.
    const VARIANTS: usize = 8;

    /// Media Foundation timestamps are in 100 ns units.
    const TICKS_PER_FRAME_30FPS: i64 = 333_333;

    /// Target bitrate, matching `rr-codec`'s software encoder settings.
    const BITRATE: u32 = 12_000_000;

    println!("Hardware H.264 capability probe");
    println!("===============================\n");

    // ---- Capture a real frame ----------------------------------------
    let pick = std::env::args()
        .nth(1)
        .and_then(|a| a.parse::<usize>().ok());

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

    let deadline = Instant::now();
    let base = loop {
        match capturer.grab() {
            Ok(f) => break f,
            Err(CaptureError::NoNewFrame) if deadline.elapsed() < Duration::from_secs(20) => {}
            Err(CaptureError::NoNewFrame) => return Err("screen never changed".into()),
            Err(e) => return Err(e.into()),
        }
    };

    // H.264 4:2:0 needs even dimensions.
    let (w, h) = (base.width & !1, base.height & !1);
    let mpx = f64::from(w) * f64::from(h) / 1e6;
    println!(
        "\nSource: {}x{} {:?}, encoding {w}x{h} ({mpx:.1} Mpx), {FRAMES} frames\n",
        base.width, base.height, base.format
    );

    // ---- Build the frame variants ------------------------------------
    let variants = build_variants(&base, w, h, VARIANTS);
    println!(
        "Prepared {VARIANTS} packed BGRA frames ({:.0} MiB total)\n",
        (variants.len() * variants.first().map_or(0, Vec::len)) as f64 / (1024.0 * 1024.0)
    );

    // ---- Encode ------------------------------------------------------
    let out_path = std::env::temp_dir().join("rr_hwencode_probe.mp4");
    let _ = std::fs::remove_file(&out_path);

    let mut encoder = VideoEncoder::new(
        VideoSettingsBuilder::new(w, h)
            .sub_type(VideoSettingsSubType::H264)
            .bitrate(BITRATE)
            .frame_rate(30),
        AudioSettingsBuilder::default().disabled(true),
        ContainerSettingsBuilder::default(),
        &out_path,
    )?;

    println!("Encoding via Media Foundation H.264 (hardware if available)...");
    let start = Instant::now();
    let mut ts = 0i64;
    for i in 0..FRAMES {
        let frame = variants
            .get(i % VARIANTS)
            .ok_or("variant index out of range")?;
        // Returns immediately: the encoder pipeline is asynchronous, so this
        // loop measures how fast it will accept work, not per-frame latency.
        encoder.send_frame_buffer(frame, ts)?;
        ts += TICKS_PER_FRAME_30FPS;
    }
    let submitted = start.elapsed();

    // `finish` drains the pipeline and finalises the container, so the honest
    // throughput number includes it.
    encoder.finish()?;
    let wall = start.elapsed();

    // ---- Report ------------------------------------------------------
    let bytes = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
    report(w, h, mpx, FRAMES, submitted, wall, bytes)?;
    let _ = std::fs::remove_file(&out_path);
    Ok(())
}

/// Print the measurement and the verdict.
#[cfg(all(windows, feature = "dxgi-capture"))]
fn report(
    w: u32,
    h: u32,
    mpx: f64,
    frames: usize,
    submitted: std::time::Duration,
    wall: std::time::Duration,
    bytes: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;

    let fps = frames as f64 / wall.as_secs_f64();
    let per_frame = wall / u32::try_from(frames).unwrap_or(1);
    let mbit = (bytes as f64 * 8.0) / (frames as f64 / 30.0) / 1e6;

    println!("\nResult");
    println!("------");
    println!("  submit loop   {submitted:.2?} (asynchronous, not the encode cost)");
    println!("  total wall    {wall:.2?} including drain and mux");
    println!("  per frame     {per_frame:.2?}");
    println!("  sustained     {fps:.1} fps at {w}x{h}");
    println!("  output        {bytes} bytes -> {mbit:.1} Mbit/s at 30 fps");
    println!(
        "  per megapixel {:.2} ms/Mpx",
        per_frame.as_secs_f64() * 1000.0 / mpx.max(f64::MIN_POSITIVE)
    );

    println!("\nVerdict");
    println!("-------");
    if bytes < 10_000 {
        println!("  INVALID: output is {bytes} bytes, so nothing was really encoded.");
        return Err("encoder produced no meaningful output".into());
    }
    // The 33.3 ms budget covers the whole frame, so leave room for capture and
    // transport rather than spending all of it on encoding.
    let budget = Duration::from_millis(33);
    if per_frame < budget / 3 {
        println!("  {fps:.0} fps. Comfortably inside the 33.3 ms budget with room for");
        println!("  capture, packetisation and the network. 4K at 30 fps is achievable");
        println!("  on this machine with hardware encoding.");
    } else if per_frame < budget {
        println!("  {fps:.0} fps. Inside the 33.3 ms budget, but without much headroom");
        println!("  once capture and transport are added. Workable, worth re-checking");
        println!("  under real load.");
    } else {
        println!("  {fps:.0} fps. Still outside the 33.3 ms budget. Hardware encoding alone");
        println!("  does not fix this resolution on this machine; reducing encoded pixels");
        println!("  would also be needed.");
    }
    println!("\n  Compare: software OpenH264 on real desktop content measured");
    println!("  56-114 ms/frame at 3840x2160 and 17-21 ms at 1920x1200.");
    println!("  Note this probe writes an MP4; a real path would use the H.264");
    println!("  elementary stream from the MFT and avoid the muxing counted here.");
    Ok(())
}

/// Build `count` packed-BGRA frames that genuinely differ from each other.
///
/// `send_frame_buffer` wants tightly packed BGRA, while DXGI frames may carry row
/// padding, so this repacks. Windows also expects bottom-to-top rows on that
/// path; the output video would be inverted, which does not affect how long
/// encoding takes and this probe only measures time.
#[cfg(all(windows, feature = "dxgi-capture"))]
fn build_variants(base: &rr_capture::Frame, w: u32, h: u32, count: usize) -> Vec<Vec<u8>> {
    let packed_w = w as usize * 4;
    (0..count)
        .map(|n| {
            let mut buf = Vec::with_capacity(packed_w * h as usize);
            for y in 0..h as usize {
                let start = y * base.stride;
                let row = base
                    .data
                    .get(start..start + packed_w)
                    .unwrap_or(&[]);
                buf.extend_from_slice(row);
            }
            paint_region(&mut buf, w, h, packed_w, n as u32);
            buf
        })
        .collect()
}

/// Overwrite a large region with high-entropy content, different per `seed`.
#[cfg(all(windows, feature = "dxgi-capture"))]
fn paint_region(buf: &mut [u8], w: u32, h: u32, stride: usize, seed: u32) {
    let block_w = (w as usize * 2 / 3).max(2);
    let block_h = (h as usize / 2).max(2);
    let x0 = (w as usize - block_w) / 2;
    let y0 = (h as usize - block_h) / 2;

    let mut state = 0x2545_F491_u32 ^ seed.wrapping_mul(0x9E37_79B9);
    for y in y0..y0 + block_h {
        let row = y * stride;
        for x in (x0..x0 + block_w).step_by(2) {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let [a, b, c, d] = state.to_le_bytes();
            put_px(buf, row + x * 4, a, b, c);
            put_px(buf, row + (x + 1) * 4, b, c, d);
        }
    }
}

/// Write one BGRA pixel, ignoring an out-of-range offset.
#[cfg(all(windows, feature = "dxgi-capture"))]
fn put_px(buf: &mut [u8], off: usize, b: u8, g: u8, r: u8) {
    if let Some(px) = buf.get_mut(off..off + 4) {
        px[0] = b;
        px[1] = g;
        px[2] = r;
        px[3] = 0xFF;
    }
}
