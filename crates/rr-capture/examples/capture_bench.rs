//! Capture-cost breakdown, to find where the per-frame time actually goes.
//!
//! The P2 baseline showed capture averaging ~33 ms against ~17 ms for encode,
//! making capture the bottleneck. This benchmark attributes that cost so the
//! optimisation effort is aimed at the real problem rather than a guess.
//!
//! For `xcap` it separates:
//!
//!   A. `Monitor::all()` — monitor enumeration (`EnumDisplayMonitors`,
//!      `GetMonitorInfoW`, `EnumDisplaySettingsW`, DC create/delete per monitor)
//!   B. `capture_image()` alone, with the monitor handle already resolved
//!   C. the two combined, i.e. what that backend does per frame
//!   D. `friendly_name()`, which calls `QueryDisplayConfig`
//!
//! It then measures the DXGI Desktop Duplication backend that replaced it. That
//! backend blocks until the desktop changes, so a stopwatch around `grab` would
//! be credited with however long the screen sat still — which says nothing about
//! what a frame costs. `ScreenCapturer::last_wait` reports that blocked time and
//! it is subtracted, leaving the work.
//!
//! Run with:
//! ```text
//! cargo run --release -p rr-capture --example capture_bench --features screen-capture,dxgi-capture
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

/// Helpers shared by the two measurement stages.
///
/// Both stages are feature-gated, so with no backend compiled in everything in
/// here would be genuine dead code. Gating the module keeps that honest in one
/// place, instead of an `allow(dead_code)` that would also hide a real mistake.
#[cfg(any(feature = "screen-capture", all(windows, feature = "dxgi-capture")))]
mod common {
    use std::time::Duration;

    /// Samples per stage. Enough to average out scheduler noise without taking
    /// minutes on the slow paths.
    pub(crate) const N: usize = 30;

    /// Report mean/min/max for a set of samples, returning the mean.
    pub(crate) fn report(label: &str, samples: &[Duration]) -> Duration {
        let total: Duration = samples.iter().sum();
        let n = u32::try_from(samples.len().max(1)).unwrap_or(1);
        let mean = total / n;
        let min = samples.iter().min().copied().unwrap_or_default();
        let max = samples.iter().max().copied().unwrap_or_default();
        println!("  {label:<42} mean {mean:>10.2?}   min {min:>10.2?}   max {max:>10.2?}");
        mean
    }

    /// Milliseconds a raw memcpy of `bytes` would take at 10 GB/s.
    ///
    /// Modern DDR4/DDR5 sustains well above that, so this is a generous lower
    /// bound on "just moving the pixels". Anything far above it is API overhead
    /// rather than memory bandwidth, which is the distinction that makes the
    /// `xcap` numbers interpretable.
    pub(crate) fn memcpy_floor_ms(bytes: u64) -> f64 {
        bytes as f64 / 10e9 * 1000.0
    }
}

#[cfg(any(feature = "screen-capture", all(windows, feature = "dxgi-capture")))]
use std::time::Duration;

#[cfg(any(feature = "screen-capture", all(windows, feature = "dxgi-capture")))]
use common::{memcpy_floor_ms, report, N};

/// Whether the portable `xcap` backend is compiled in.
const HAS_XCAP: bool = cfg!(feature = "screen-capture");

/// Whether the Windows DXGI backend is compiled in.
const HAS_DXGI: bool = cfg!(all(windows, feature = "dxgi-capture"));

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if !(HAS_XCAP || HAS_DXGI) {
        println!("No capture backend compiled in. Re-run with:");
        println!("  --features screen-capture,dxgi-capture");
        return Err("no capture backend enabled".into());
    }

    #[cfg(feature = "screen-capture")]
    xcap_stages()?;

    #[cfg(all(windows, feature = "dxgi-capture"))]
    dxgi_stages()?;

    Ok(())
}

/// Attribute the cost of the `xcap` GDI path.
#[cfg(feature = "screen-capture")]
fn xcap_stages() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Instant;

    use xcap::Monitor;

    println!("xcap (GDI) cost breakdown ({N} samples per stage)\n");

    // ---- A. Monitor enumeration -------------------------------------
    let mut enum_samples = Vec::with_capacity(N);
    for _ in 0..N {
        let t = Instant::now();
        let ms = Monitor::all()?;
        // Touch the vec so the call cannot be optimised away.
        let _ = std::hint::black_box(ms.len());
        enum_samples.push(t.elapsed());
    }
    let enum_mean = report("A. Monitor::all() [was per-frame]", &enum_samples);

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
    let _ = report("   id() [per monitor in resolve]", &id_samples);

    // ---- B. capture_image with the handle already resolved ----------
    let mut cap_samples = Vec::with_capacity(N);
    for _ in 0..N {
        let t = Instant::now();
        let img = primary.capture_image()?;
        let _ = std::hint::black_box(img.width());
        cap_samples.push(t.elapsed());
    }
    let cap_mean = report("B. capture_image() [cached handle]", &cap_samples);

    // ---- C. Full backend path: enumerate + capture ------------------
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
    let combined_mean = report("C. enumerate + capture [full path]", &combined);

    // ---- Verdict ----------------------------------------------------
    println!("\nAnalysis");
    println!("--------");
    let (w, h) = (primary.width().unwrap_or(0), primary.height().unwrap_or(0));
    let bytes = u64::from(w) * u64::from(h) * 4;
    let mib = bytes as f64 / (1024.0 * 1024.0);
    println!("  frame: {w}x{h} = {mib:.1} MiB per frame");

    let overhead = combined_mean.saturating_sub(cap_mean);
    let pct = if combined_mean.as_secs_f64() > 0.0 {
        overhead.as_secs_f64() / combined_mean.as_secs_f64() * 100.0
    } else {
        0.0
    };
    println!("  enumeration overhead per frame: {overhead:.2?} ({pct:.0}% of the path)");
    println!("  capture floor (cached handle):  {cap_mean:.2?}");
    println!(
        "  (a raw {mib:.1} MiB memcpy at 10 GB/s would be ~{:.2} ms)",
        memcpy_floor_ms(bytes)
    );

    println!("\nConclusions");
    println!("-----------");
    if overhead > Duration::from_millis(2) {
        println!("  * Caching the monitor handle is worth doing: it removes {overhead:.2?}/frame.");
    } else {
        println!("  * Enumeration is cheap here ({enum_mean:.2?}); caching it is tidy but not a win.");
    }
    if cap_mean > Duration::from_millis(8) {
        println!("  * capture_image() itself is the real cost at {cap_mean:.2?}. That is a full");
        println!("    GDI surface copy with no damage tracking, which is why the DXGI backend");
        println!("    exists.");
    } else {
        println!("  * capture_image() is already fast ({cap_mean:.2?}); the overhead was elsewhere.");
    }
    println!();
    Ok(())
}

/// Take one throwaway frame so the measured samples exclude session setup.
///
/// The duplication session is opened lazily, so the first grab also pays for a
/// Direct3D 11 device, the DXGI output lookup and the staging texture. That is
/// once-per-session cost, not per-frame cost, and leaving it in the samples
/// moves the mean by milliseconds.
#[cfg(all(windows, feature = "dxgi-capture"))]
fn warm_up(
    capturer: &mut dyn rr_capture::ScreenCapturer,
    deadline: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Instant;

    use rr_capture::CaptureError;

    let start = Instant::now();
    let mut warmed = false;
    while start.elapsed() < deadline {
        match capturer.grab() {
            Ok(_) => {
                warmed = true;
                break;
            }
            Err(CaptureError::NoNewFrame) => {}
            Err(e) => return Err(e.into()),
        }
    }
    println!(
        "  session setup (first grab, excluded below): {:.2?}{}",
        start.elapsed(),
        if warmed { "" } else { " [no frame arrived]" }
    );
    Ok(())
}

/// Measure the DXGI Desktop Duplication backend through the public trait.
#[cfg(all(windows, feature = "dxgi-capture"))]
fn dxgi_stages() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Instant;

    use rr_capture::{CaptureError, DxgiCapturer, ScreenCapturer};

    /// Cap on how long to hunt for `N` delivered frames, so a perfectly still
    /// desktop cannot hang the benchmark.
    const DEADLINE: Duration = Duration::from_secs(20);

    println!("DXGI Desktop Duplication ({N} delivered frames)\n");

    // A real timeout, so an idle desktop makes the capture thread sleep instead
    // of spinning a core. The blocked time is subtracted below.
    let mut capturer = DxgiCapturer::new()?;

    let t = Instant::now();
    let monitors = capturer.monitors()?;
    let enumerate_once = t.elapsed();
    for m in &monitors {
        println!(
            "  id={:<4} {:<28} {:>5}x{:<5} primary={}",
            m.id, m.name, m.width, m.height, m.primary
        );
    }
    let target = monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first())
        .ok_or("no monitors")?
        .clone();
    capturer.select_monitor(target.id)?;
    println!(
        "  -> monitor id={} ({}), enumeration took {enumerate_once:.2?}\n",
        target.id, target.name
    );

    warm_up(&mut capturer, DEADLINE)?;

    let mut samples = Vec::with_capacity(N);
    let mut waits = Vec::with_capacity(N);
    let mut no_change = 0usize;
    let mut first_geometry = None;
    let start = Instant::now();
    while samples.len() < N && start.elapsed() < DEADLINE {
        let t = Instant::now();
        match capturer.grab() {
            Ok(frame) => {
                let wall = t.elapsed();
                // Subtract the blocked wait; what remains is acquire teardown,
                // the GPU copy, the map and the memcpy out — the real cost.
                let wait = capturer.last_wait().unwrap_or(Duration::ZERO).min(wall);
                if first_geometry.is_none() {
                    first_geometry = Some((frame.width, frame.height, frame.stride, frame.format));
                }
                let _ = std::hint::black_box(frame.data.len());
                samples.push(wall - wait);
                waits.push(wait);
            }
            // Desktop unchanged. Deliberately not recorded: it is not a frame.
            Err(CaptureError::NoNewFrame) => no_change += 1,
            Err(e) => return Err(e.into()),
        }
    }

    if samples.is_empty() {
        println!("  No frames delivered in {DEADLINE:?}: the desktop never changed.");
        println!("  That is the backend working as intended, but it means there is nothing");
        println!("  to time. Re-run with something moving on screen.");
        return Ok(());
    }

    let mean = report("E. grab() work [copy + map, no wait]", &samples);
    let _ = report("F. blocked waiting for a change", &waits);
    println!(
        "  delivered {} frames, {no_change} calls found no change",
        samples.len()
    );

    println!("\nAnalysis");
    println!("--------");
    if let Some((w, h, stride, format)) = first_geometry {
        let bytes = stride as u64 * u64::from(h);
        let mib = bytes as f64 / (1024.0 * 1024.0);
        println!("  frame: {w}x{h} stride={stride} format={format:?} = {mib:.1} MiB");
        let floor = memcpy_floor_ms(bytes);
        println!("  (a raw {mib:.1} MiB memcpy at 10 GB/s would be ~{floor:.2} ms)");
        let ratio = mean.as_secs_f64() * 1000.0 / floor.max(f64::MIN_POSITIVE);
        println!("  grab() is {ratio:.1}x the memcpy floor");
    }
    let fps = if mean.as_secs_f64() > 0.0 {
        1.0 / mean.as_secs_f64()
    } else {
        f64::INFINITY
    };
    println!("  capture alone could sustain {fps:.0} fps (33.3ms = 30 fps budget)");
    if no_change > 0 {
        println!(
            "  {no_change} calls cost nothing because the screen was static - work the GDI"
        );
        println!("  path would have done in full regardless.");
    }
    println!();
    Ok(())
}
