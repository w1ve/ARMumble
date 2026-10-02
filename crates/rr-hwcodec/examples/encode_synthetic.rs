//! Does the hardware encoder actually work?
//!
//! Drives [`rr_hwcodec::HwEncoder`] with the deterministic synthetic capture
//! source, so it needs no display and no second monitor. The point is to prove the
//! Media Foundation pipeline functions — activation, async unlock, type
//! negotiation, submission, event pumping, output extraction — before measuring
//! anything or wiring it into the Station.
//!
//! What it checks:
//!   1. A hardware encoder exists and activates.
//!   2. Which input layout was negotiated, and whether the MFT is asynchronous.
//!   3. Frames go in and H.264 bytes come out.
//!   4. The first output is an IDR and carries SPS/PPS, which a decoder needs.
//!   5. Output starts with an Annex-B start code, i.e. it is an elementary stream
//!      rather than length-prefixed AVCC.
//!
//! Run with:
//! ```text
//! cargo run --release -p rr-hwcodec --example encode_synthetic
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

#[cfg(not(windows))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("Windows only.");
    Err("hardware encoding is Windows-only".into())
}

#[cfg(windows)]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Instant;

    use rr_capture::{ScreenCapturer, SyntheticCapturer};
    use rr_hwcodec::{available_h264_encoders, HwEncoder, HwEncoderSettings};

    // Watch the Media Foundation negotiation with RUST_LOG=debug. Off by default
    // so the probe's own output stays readable.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    // `RR_PREFER_NV12=1` skips the BGRA attempt. Some drivers accept a BGRA input
    // type and then fail to encode, so being able to compare the two paths without
    // a rebuild is worth the switch.
    let prefer_bgra = std::env::var("RR_PREFER_NV12").is_err();

    /// Frames to push through. Enough to clear the pipeline's start-up and see a
    /// steady stream of outputs.
    const FRAMES: usize = 60;
    /// A commonplace resolution, and small enough to be quick.
    const W: u32 = 1280;
    const H: u32 = 720;

    println!("Hardware H.264 encode test (synthetic source)");
    println!("=============================================\n");

    for e in available_h264_encoders()? {
        println!(
            "  {} {}",
            if e.hardware { "[HARDWARE]" } else { "[software]" },
            e.name
        );
    }

    println!("\nCreating encoder at {W}x{H} (prefer_bgra = {prefer_bgra})...");
    let mut encoder = HwEncoder::new(
        W,
        H,
        HwEncoderSettings {
            prefer_bgra,
            ..HwEncoderSettings::default()
        },
    )?;
    println!("  encoder   {}", encoder.info().name);
    println!("  hardware  {}", encoder.info().hardware);
    println!("  async     {}", encoder.is_async());
    println!(
        "  input     {:?} (CPU conversion needed: {})",
        encoder.input_layout(),
        encoder.input_layout().needs_cpu_conversion()
    );
    println!("  detail    {encoder:?}");

    let mut capturer = SyntheticCapturer::new(W, H);
    let mut outputs = Vec::new();
    let mut keyframes = 0usize;
    let mut total_bytes = 0usize;

    println!("\nEncoding {FRAMES} frames...");
    let start = Instant::now();
    for i in 0..FRAMES {
        let frame = capturer.grab()?;
        // Ask for a keyframe up front: a viewer joining mid-stream needs one, and
        // this proves the request path works rather than assuming it.
        if i == 0 {
            encoder.request_keyframe();
        }
        if let Some(out) = encoder.encode(&frame)? {
            if out.is_keyframe {
                keyframes += 1;
            }
            total_bytes += out.data.len();
            outputs.push(out);
        }
    }
    let encode_wall = start.elapsed();
    let tail = encoder.flush()?;
    for out in tail {
        if out.is_keyframe {
            keyframes += 1;
        }
        total_bytes += out.data.len();
        outputs.push(out);
    }

    println!("\nResult");
    println!("------");
    println!("  frames submitted  {FRAMES}");
    println!("  frames encoded    {}", outputs.len());
    println!("  keyframes         {keyframes}");
    println!("  total bytes       {total_bytes}");
    println!(
        "  wall              {encode_wall:.2?} ({:.2?}/frame submitted)",
        encode_wall / u32::try_from(FRAMES).unwrap_or(1)
    );

    verify(&outputs, keyframes, total_bytes)
}

/// Assert the stream is actually usable, not merely non-empty.
#[cfg(windows)]
fn verify(
    outputs: &[rr_hwcodec::EncodedFrame],
    keyframes: usize,
    total_bytes: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\nChecks");
    println!("------");
    let mut failures = Vec::new();

    if outputs.is_empty() {
        failures.push("no output at all: the encoder accepted frames and produced nothing");
    }
    if total_bytes == 0 {
        failures.push("every output was empty");
    }
    if keyframes == 0 {
        failures.push("no keyframe produced, so no viewer could ever start decoding");
    }

    if let Some(first) = outputs.first() {
        // Annex-B elementary streams begin with a 3- or 4-byte start code. AVCC
        // would begin with a big-endian length, so this distinguishes them, and
        // WebRTC needs Annex-B.
        let annexb = first.data.starts_with(&[0, 0, 0, 1]) || first.data.starts_with(&[0, 0, 1]);
        println!("  starts with Annex-B start code: {annexb}");
        if !annexb {
            failures.push("output is not an Annex-B elementary stream");
        }

        // A decodable stream needs SPS (NAL type 7) and PPS (type 8) before the
        // first IDR (type 5). Report what is actually present.
        let types = nal_types(&first.data);
        println!("  NAL types in first output: {types:?}");
        if !types.contains(&7) {
            failures.push("first output carries no SPS (NAL 7); a decoder cannot start");
        }
        if !types.contains(&8) {
            failures.push("first output carries no PPS (NAL 8); a decoder cannot start");
        }
        if !first.is_keyframe {
            failures.push("first output is not flagged as a keyframe");
        }
    }

    if failures.is_empty() {
        println!("\nRESULT: PASS - hardware H.264 encoding works.");
        Ok(())
    } else {
        println!("\nRESULT: FAIL");
        for f in &failures {
            println!("  - {f}");
        }
        Err("hardware encoder checks failed".into())
    }
}

/// The distinct NAL unit types present in an Annex-B stream.
///
/// Walks start codes rather than parsing properly: enough to confirm SPS/PPS/IDR
/// are present, which is all this probe needs to assert.
#[cfg(windows)]
fn nal_types(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 3 < data.len() {
        let four = data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 0 && data[i + 3] == 1;
        let three = data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1;
        if four || three {
            let header = i + if four { 4 } else { 3 };
            if let Some(byte) = data.get(header) {
                let t = byte & 0x1F;
                if !out.contains(&t) {
                    out.push(t);
                }
            }
            i = header;
        } else {
            i += 1;
        }
    }
    out
}
