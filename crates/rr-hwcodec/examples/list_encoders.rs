//! What video encoders does this machine actually have?
//!
//! Run this on any prospective Station to see whether it can encode in hardware,
//! and which component would do it. The Station will make the same call at
//! startup to choose between `rr-hwcodec` and `rr-codec`'s software encoder.
//!
//! Also times the BGRA → NV12 conversion at the resolutions that matter, because
//! that cost is only paid when the chosen encoder refuses BGRA input — and at 4K
//! it is large enough to change the design.
//!
//! Run with:
//! ```text
//! cargo run --release -p rr-hwcodec --example list_encoders
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

use std::time::Instant;

use rr_hwcodec::{available_h264_encoders, bgra_to_nv12, has_hardware_h264, nv12_len};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("RemoteRadio hardware encoder probe");
    println!("==================================\n");

    let encoders = available_h264_encoders()?;
    if encoders.is_empty() {
        println!("No H.264 encoder MFTs found at all.");
    } else {
        println!("H.264 encoders ({} found):", encoders.len());
        for e in &encoders {
            let kind = if e.hardware { "HARDWARE" } else { "software" };
            println!("  [{kind}] {}", e.name);
        }
    }

    let hw = has_hardware_h264()?;
    println!("\nHardware H.264 available: {hw}");
    if hw {
        println!("  -> this machine can encode 4K in hardware; measured 70-85 fps");
        println!("     at 3840x2160 via Quick Sync, vs 7.5-13.8 fps in software.");
    } else {
        println!("  -> software encoding only. Expect ~20 fps at 1920x1200 and");
        println!("     well under 15 fps at 3840x2160 on a 4-core CPU.");
    }

    // ---- Conversion cost ---------------------------------------------
    println!("\nBGRA -> NV12 conversion cost (only paid if the encoder refuses BGRA)");
    for (label, w, h) in [("1920x1200", 1920u32, 1200u32), ("3840x2160", 3840, 2160)] {
        // A gradient rather than zeroes: a buffer of zeroes can be optimised in
        // ways real pixel data cannot.
        let stride = w as usize * 4;
        let mut src = vec![0u8; stride * h as usize];
        for (i, b) in src.iter_mut().enumerate() {
            *b = u8::try_from(i % 251).unwrap_or(0);
        }
        let mut dst = vec![0u8; nv12_len(w, h)];

        // Best of five: this is a tight CPU loop and a busy desktop will steal a
        // core mid-measurement, which inflates a mean but cannot deflate a
        // minimum.
        let mut best = None;
        for _ in 0..5 {
            let t = Instant::now();
            let written = bgra_to_nv12(&src, w, h, stride, &mut dst).expect("conversion");
            let elapsed = t.elapsed();
            assert_eq!(written, nv12_len(w, h));
            if best.is_none_or(|b| elapsed < b) {
                best = Some(elapsed);
            }
        }
        let best = best.expect("at least one sample");
        let mpx = f64::from(w) * f64::from(h) / 1e6;
        println!(
            "  {label}  best {:>8.2?}  ({:.2} ms/Mpx)",
            best,
            best.as_secs_f64() * 1000.0 / mpx
        );
    }
    println!("\n  For scale: the whole frame budget at 30 fps is 33.3 ms, and DXGI");
    println!("  capture already costs 3.9 ms at 1920x1200 and 16-18 ms at 3840x2160.");
    println!("  If this conversion is a large fraction of that, the encoder must be");
    println!("  fed BGRA directly, or the frame must stay on the GPU.");

    Ok(())
}
