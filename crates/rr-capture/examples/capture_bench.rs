//! Capture-cost breakdown, to find where the per-frame time actually goes.
//!
//! The P2 baseline showed capture averaging ~33 ms against ~17 ms for encode,
//! making capture the bottleneck. This benchmark attributes that cost so the
//! optimisation effort is aimed at the real problem rather than a guess. It
//! separates:
//!
//!   A. `Monitor::all()` — monitor enumeration (`EnumDisplayMonitors`,
//!      `GetMonitorInfoW`, `EnumDisplaySettingsW`, DC create/delete per monitor)
//!   B. `capture_image()` alone, with the monitor handle already resolved
//!   C. the two combined, i.e. what the current backend does per frame
//!   D. `friendly_name()`, which calls `QueryDisplayConfig`
//!
//! Run with:
//! ```text
//! cargo run --release -p rr-capture --example capture_bench --features screen-capture
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

#[cfg(feature = "screen-capture")]
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run()
}

#[cfg(not(feature = "screen-capture"))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("Re-run with: --features screen-capture");
    Err("screen-capture feature not enabled".into())
}

#[cfg(feature = "screen-capture")]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use xcap::Monitor;

    /// Samples per stage. Enough to average out scheduler noise without
    /// taking minutes on the slow paths.
    const N: usize = 30;

    /// Report mean/min/max for a set of samples.
    fn report(label: &str, samples: &[Duration]) -> Duration {
        let total: Duration = samples.iter().sum();
        let n = u32::try_from(samples.len().max(1)).unwrap_or(1);
        let mean = total / n;
        let min = samples.iter().min().copied().unwrap_or_default();
        let max = samples.iter().max().copied().unwrap_or_default();
        println!("  {label:<42} mean {mean:>10.2?}   min {min:>10.2?}   max {max:>10.2?}");
        mean
    }

    println!("Capture cost breakdown ({N} samples per stage)\n");

    // ---- A. Monitor enumeration -------------------------------------
    let mut enum_samples = Vec::with_capacity(N);
    for _ in 0..N {
        let t = Instant::now();
        let ms = Monitor::all()?;
        // Touch the vec so the call cannot be optimised away.
        let _ = std::hint::black_box(ms.len());
        enum_samples.push(t.elapsed());
    }
    let enum_mean = report("A. Monitor::all() [per-frame today]", &enum_samples);

    // ---- D. friendly_name (QueryDisplayConfig) ----------------------
    let monitors = Monitor::all()?;
    let primary = monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())
        .ok_or("no monitors")?
        .clone();

    let mut name_samples = Vec::with_capacity(N);
    for _ in 0..N {
        let t = Instant::now();
        let n = primary.friendly_name().unwrap_or_default();
        let _ = std::hint::black_box(n.len());
        name_samples.push(t.elapsed());
    }
    let _ = report("D. friendly_name() [QueryDisplayConfig]", &name_samples);

    // ---- Accessors used while resolving a selection -----------------
    let mut id_samples = Vec::with_capacity(N);
    for _ in 0..N {
        let t = Instant::now();
        let _ = std::hint::black_box(primary.id().unwrap_or(0));
        id_samples.push(t.elapsed());
    }
    let _ = report("   id() [called per monitor in resolve]", &id_samples);

    // ---- B. capture_image with the handle already resolved ----------
    let mut cap_samples = Vec::with_capacity(N);
    for _ in 0..N {
        let t = Instant::now();
        let img = primary.capture_image()?;
        let _ = std::hint::black_box(img.width());
        cap_samples.push(t.elapsed());
    }
    let cap_mean = report("B. capture_image() [cached handle]", &cap_samples);

    // ---- C. Current backend path: enumerate + capture ---------------
    let mut combined = Vec::with_capacity(N);
    for _ in 0..N {
        let t = Instant::now();
        let ms = Monitor::all()?;
        let m = ms
            .into_iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .ok_or("no primary")?;
        let img = m.capture_image()?;
        let _ = std::hint::black_box(img.width());
        combined.push(t.elapsed());
    }
    let combined_mean = report("C. enumerate + capture [current path]", &combined);

    // ---- Verdict ----------------------------------------------------
    println!("\nAnalysis");
    println!("--------");
    let (w, h) = (
        primary.width().unwrap_or(0),
        primary.height().unwrap_or(0),
    );
    let bytes = u64::from(w) * u64::from(h) * 4;
    println!("  frame: {w}x{h} = {:.1} MiB per frame", bytes as f64 / (1024.0 * 1024.0));

    let overhead = combined_mean.saturating_sub(cap_mean);
    let pct = if combined_mean.as_secs_f64() > 0.0 {
        overhead.as_secs_f64() / combined_mean.as_secs_f64() * 100.0
    } else {
        0.0
    };
    println!("  enumeration overhead per frame: {overhead:.2?} ({pct:.0}% of the current path)");
    println!("  capture floor (cached handle):  {cap_mean:.2?}");

    // What a pure memcpy of that many bytes would cost, as a sanity bound:
    // modern DDR4/DDR5 sustains >10 GB/s, so a 9 MiB copy is ~1 ms. Anything
    // far above that is API overhead, not memory bandwidth.
    let memcpy_ms = bytes as f64 / 10e9 * 1000.0;
    println!("  (a raw {:.1} MiB memcpy at 10 GB/s would be ~{memcpy_ms:.2} ms)", bytes as f64 / (1024.0 * 1024.0));

    println!("\nConclusions");
    println!("-----------");
    if overhead > Duration::from_millis(2) {
        println!("  * Caching the monitor handle is worth doing: it removes {overhead:.2?}/frame.");
    } else {
        println!("  * Enumeration is cheap here ({enum_mean:.2?}); caching it is tidy but not a win.");
    }
    if cap_mean > Duration::from_millis(8) {
        println!("  * capture_image() itself is the real cost at {cap_mean:.2?}. That is a full");
        println!("    GDI surface copy with no damage tracking, so a DXGI Desktop Duplication");
        println!("    or WGC backend (dirty rectangles, GPU-side) is the structural fix.");
    } else {
        println!("  * capture_image() is already fast ({cap_mean:.2?}); the overhead was elsewhere.");
    }
    Ok(())
}
