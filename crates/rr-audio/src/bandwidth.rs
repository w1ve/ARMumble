//! Fit the requested audio quality into the server's bandwidth limit without
//! silently degrading it.
//!
//! Mumble servers silently drop voice packets once a user exceeds the server's
//! per-user limit, which sounds like heavy distortion. The planner therefore
//! never asks for more than the server allows, prefers to give up *latency*
//! (longer frames, less per-packet overhead) before it gives up *bit rate*, and
//! reports when it had to clamp so the UI can say so.

/// Bytes of IP + UDP + Mumble voice framing added to every packet.
pub const PACKET_OVERHEAD_BYTES: u32 = 38;
/// Fraction of the server limit we allow ourselves to use.
const HEADROOM: f64 = 0.95;
/// Frame durations the planner may choose from, shortest first.
const FRAMES_MS: [u32; 3] = [20, 40, 60];

/// The outcome of planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandwidthPlan {
    /// Opus payload bit rate to use.
    pub bitrate_bps: u32,
    /// Frame duration to use.
    pub frame_ms: u32,
    /// True when `bitrate_bps` is lower than what was requested.
    pub clamped: bool,
    /// Resulting rate on the wire including packet overhead.
    pub wire_bps: u32,
}

fn overhead_bps(frame_ms: u32) -> u32 {
    PACKET_OVERHEAD_BYTES * 8 * 1000 / frame_ms
}

/// Choose bit rate and frame duration.
///
/// * `requested_bps` – payload bit rate the user wants.
/// * `stereo` – whether two channels are encoded (sets the quality floor).
/// * `max_frame_ms` – longest frame the user accepts (latency cap, at least 20).
/// * `server_limit_bps` – the server's per-user limit, if known.
#[must_use]
pub fn plan(
    requested_bps: u32,
    stereo: bool,
    max_frame_ms: u32,
    server_limit_bps: Option<u32>,
) -> BandwidthPlan {
    let frames: Vec<u32> = FRAMES_MS.iter().copied().filter(|f| *f <= max_frame_ms.max(20)).collect();
    let Some(limit) = server_limit_bps else {
        let frame_ms = frames[0];
        return BandwidthPlan {
            bitrate_bps: requested_bps,
            frame_ms,
            clamped: false,
            wire_bps: requested_bps + overhead_bps(frame_ms),
        };
    };
    let usable = (f64::from(limit) * HEADROOM) as u32;
    let avail = |f: u32| usable.saturating_sub(overhead_bps(f));
    // 1. Smallest frame that carries the full request.
    if let Some(f) = frames.iter().copied().find(|f| avail(*f) >= requested_bps) {
        return BandwidthPlan {
            bitrate_bps: requested_bps,
            frame_ms: f,
            clamped: false,
            wire_bps: requested_bps + overhead_bps(f),
        };
    }
    // 2. Otherwise the smallest frame that still meets the quality floor.
    let floor = if stereo { 96_000 } else { 48_000 };
    let f = frames
        .iter()
        .copied()
        .find(|f| avail(*f) >= floor)
        .unwrap_or_else(|| frames[frames.len() - 1]);
    let bitrate = avail(f).max(6_000).min(requested_bps);
    BandwidthPlan {
        bitrate_bps: bitrate,
        frame_ms: f,
        clamped: true,
        wire_bps: bitrate + overhead_bps(f),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_server_gets_request() {
        let p = plan(128_000, true, 40, None);
        assert_eq!((p.bitrate_bps, p.frame_ms, p.clamped), (128_000, 20, false));
    }

    #[test]
    fn roomy_server_keeps_full_rate() {
        let p = plan(128_000, true, 40, Some(192_000));
        assert_eq!((p.bitrate_bps, p.frame_ms, p.clamped), (128_000, 20, false));
        assert!(p.wire_bps <= 192_000);
    }

    #[test]
    fn tight_server_is_clamped_and_reported() {
        let p = plan(128_000, true, 40, Some(72_000));
        assert!(p.clamped);
        assert_eq!(p.frame_ms, 40);
        assert!(p.wire_bps <= 72_000);
    }

    #[test]
    fn medium_server_trades_latency_for_rate() {
        let p = plan(100_000, true, 60, Some(120_000));
        assert!(!p.clamped, "{p:?}");
        assert!(p.frame_ms > 20);
        assert!(p.wire_bps <= 120_000);
    }
}
