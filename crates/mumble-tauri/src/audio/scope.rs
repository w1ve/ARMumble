//! Oscilloscope sample tap hub.
//!
//! The capture (input) and playback/mixer (output) chains push small blocks
//! of PCM here. When the frontend has a scope mounted it subscribes via the
//! `scope_subscribe` command (ref-counted); the hub then decimates and emits
//! `scope-data` events at ~30 fps. When no scope is mounted the push is a
//! single relaxed atomic load, so the audio threads pay almost nothing.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Which pipeline a scope frame came from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScopeSource {
    Input,
    Output,
}

impl ScopeSource {
    fn as_str(self) -> &'static str {
        match self {
            ScopeSource::Input => "input",
            ScopeSource::Output => "output",
        }
    }

    /// Parse the frontend's string identifier.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "input" => Some(ScopeSource::Input),
            "output" => Some(ScopeSource::Output),
            _ => None,
        }
    }
}

/// Target number of points per channel to draw. The waveform is decimated to
/// this many points regardless of the native frame size.
const POINTS_PER_CHANNEL: usize = 256;

/// Minimum interval between emitted frames per source (~30 fps).
const MIN_EMIT_INTERVAL_MS: u64 = 33;

/// Payload emitted on the `scope-data` event. Matches the `ScopeFrame`
/// interface in the frontend `Oscilloscope.tsx`.
#[derive(Serialize, Clone)]
struct ScopeFrame {
    source: &'static str,
    channels: usize,
    samples: Vec<f32>,
}

struct ScopeHub {
    app: OnceLock<tauri::AppHandle>,
    input_subs: AtomicUsize,
    output_subs: AtomicUsize,
    input_last_ms: AtomicU64,
    output_last_ms: AtomicU64,
}

static HUB: ScopeHub = ScopeHub {
    app: OnceLock::new(),
    input_subs: AtomicUsize::new(0),
    output_subs: AtomicUsize::new(0),
    input_last_ms: AtomicU64::new(0),
    output_last_ms: AtomicU64::new(0),
};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Record the Tauri app handle so the hub can emit events. Call once at
/// startup.
pub fn set_app_handle(app: tauri::AppHandle) {
    let _ = HUB.app.set(app);
}

/// Increment the subscriber count for a source (scope mounted).
pub fn subscribe(source: ScopeSource) {
    let counter = match source {
        ScopeSource::Input => &HUB.input_subs,
        ScopeSource::Output => &HUB.output_subs,
    };
    let _prev = counter.fetch_add(1, Ordering::Relaxed);
}

/// Decrement the subscriber count for a source (scope unmounted).
pub fn unsubscribe(source: ScopeSource) {
    let counter = match source {
        ScopeSource::Input => &HUB.input_subs,
        ScopeSource::Output => &HUB.output_subs,
    };
    // Saturating decrement via a relaxed CAS loop.
    let mut cur = counter.load(Ordering::Relaxed);
    while cur > 0 {
        match counter.compare_exchange_weak(
            cur,
            cur - 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => cur = actual,
        }
    }
}

fn subs(source: ScopeSource) -> usize {
    match source {
        ScopeSource::Input => HUB.input_subs.load(Ordering::Relaxed),
        ScopeSource::Output => HUB.output_subs.load(Ordering::Relaxed),
    }
}

fn last_ms(source: ScopeSource) -> &'static AtomicU64 {
    match source {
        ScopeSource::Input => &HUB.input_last_ms,
        ScopeSource::Output => &HUB.output_last_ms,
    }
}

/// Push a block of interleaved `f32` PCM samples for a source.
///
/// `channels` is 1 (mono) or 2 (stereo); `samples` is interleaved. This is
/// cheap and non-blocking: it returns immediately if nothing is subscribed or
/// the per-source rate limit has not elapsed. Safe to call from the audio
/// thread.
pub fn push(source: ScopeSource, samples: &[f32], channels: usize) {
    // Fast path: nobody is watching.
    if subs(source) == 0 || samples.is_empty() || channels == 0 {
        return;
    }

    // Rate-limit per source.
    let now = now_ms();
    let last = last_ms(source);
    let prev = last.load(Ordering::Relaxed);
    if now.saturating_sub(prev) < MIN_EMIT_INTERVAL_MS {
        return;
    }
    // Claim this emit slot; if another thread beat us, bail.
    if last
        .compare_exchange(prev, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    let Some(app) = HUB.app.get() else {
        return;
    };

    let decimated = decimate(samples, channels, POINTS_PER_CHANNEL);
    let frame = ScopeFrame {
        source: source.as_str(),
        channels,
        samples: decimated,
    };

    use tauri::Emitter;
    let _ = app.emit("scope-data", frame);
}

/// Decimate interleaved PCM to at most `points_per_channel` points per
/// channel, preserving interleaving and channel count. Uses a stride that
/// picks representative samples (peak-ish) rather than simple nearest, so the
/// waveform shape is not aliased away.
fn decimate(samples: &[f32], channels: usize, points_per_channel: usize) -> Vec<f32> {
    let frames = samples.len() / channels;
    if frames == 0 {
        return Vec::new();
    }
    if frames <= points_per_channel {
        // Already small enough; return as-is (trimmed to whole frames).
        return samples[..frames * channels].to_vec();
    }

    let mut out = Vec::with_capacity(points_per_channel * channels);
    let stride = frames as f32 / points_per_channel as f32;
    for p in 0..points_per_channel {
        // Window [start, end) of source frames this output point represents.
        let start = (p as f32 * stride) as usize;
        let end = (((p + 1) as f32) * stride) as usize;
        let end = end.min(frames).max(start + 1);
        for ch in 0..channels {
            // Pick the sample with the largest magnitude in the window so
            // transients survive the downsample.
            let mut best = 0.0f32;
            let mut best_abs = -1.0f32;
            for f in start..end {
                let s = samples[f * channels + ch];
                let a = s.abs();
                if a > best_abs {
                    best_abs = a;
                    best = s;
                }
            }
            out.push(best);
        }
    }
    out
}
