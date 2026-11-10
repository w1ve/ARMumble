//! Network jitter estimation used to size the playout buffer.

/// Number of measurement windows kept.
const WINDOWS: usize = 8;
/// Length of one window in seconds.
const WINDOW_SECS: f64 = 2.0;
/// Lowest and highest buffering target in milliseconds.
const MIN_TARGET_MS: f64 = 40.0;
const MAX_TARGET_MS: f64 = 500.0;
/// Safety factor applied to the observed delay peak.
const SAFETY: f64 = 1.25;

/// Tracks packet delay variation and derives the buffering target.
///
/// For every packet the *transit* (arrival time minus sender time) is
/// computed. The smallest transit over the recent past is the "no queueing"
/// baseline; how far transits rise above it is the jitter. The largest recent
/// excursion, with a safety factor, is the buffering needed to ride it out.
#[derive(Debug, Clone)]
pub struct JitterEstimator {
    min_transit: [f64; WINDOWS],
    peak_delay: [f64; WINDOWS],
    cur: usize,
    window_start: f64,
    started: bool,
    target_ms: f64,
    last_frame_ms: f64,
}

impl Default for JitterEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl JitterEstimator {
    /// A fresh estimator with a conservative 60 ms starting target.
    #[must_use]
    pub fn new() -> Self {
        Self {
            min_transit: [f64::INFINITY; WINDOWS],
            peak_delay: [0.0; WINDOWS],
            cur: 0,
            window_start: 0.0,
            started: false,
            target_ms: 60.0,
            last_frame_ms: 20.0,
        }
    }

    /// Forget the delay baseline (the sender's clock jumps between talk-spurts)
    /// but keep the learned jitter peaks.
    pub fn reset_baseline(&mut self) {
        self.min_transit = [f64::INFINITY; WINDOWS];
        self.started = false;
    }

    /// Record a packet. `arrival_s` is the local arrival time and `sent_s` the
    /// sender's media time (sequence number × 10 ms); `frame_ms` is the packet
    /// duration.
    pub fn on_packet(&mut self, arrival_s: f64, sent_s: f64, frame_ms: f64) {
        self.last_frame_ms = frame_ms;
        if !self.started {
            self.started = true;
            self.window_start = arrival_s;
        }
        self.rotate(arrival_s);
        let transit = arrival_s - sent_s;
        if transit < self.min_transit[self.cur] {
            self.min_transit[self.cur] = transit;
        }
        let base = self.min_transit.iter().copied().fold(f64::INFINITY, f64::min);
        let delay = (transit - base).max(0.0);
        if delay > self.peak_delay[self.cur] {
            self.peak_delay[self.cur] = delay;
        }
        // Attack immediately.
        let needed = self.needed_ms();
        if needed > self.target_ms {
            self.target_ms = needed;
        }
    }

    fn rotate(&mut self, now: f64) {
        let mut steps = 0;
        while now - self.window_start >= WINDOW_SECS && steps < WINDOWS {
            self.cur = (self.cur + 1) % WINDOWS;
            self.min_transit[self.cur] = f64::INFINITY;
            self.peak_delay[self.cur] = 0.0;
            self.window_start += WINDOW_SECS;
            steps += 1;
            // Release slowly toward what is currently needed.
            let needed = self.needed_ms();
            if needed < self.target_ms {
                self.target_ms += (needed - self.target_ms) * 0.25;
            }
        }
        if steps == WINDOWS {
            self.window_start = now;
        }
    }

    fn needed_ms(&self) -> f64 {
        let peak = self.peak_delay.iter().copied().fold(0.0, f64::max) * 1000.0;
        (peak * SAFETY + self.last_frame_ms).clamp(MIN_TARGET_MS, MAX_TARGET_MS)
    }

    /// Current buffering target in milliseconds.
    #[must_use]
    pub fn target_ms(&self) -> f64 {
        self.target_ms
    }

    /// Largest recent delay excursion in milliseconds.
    #[must_use]
    pub fn jitter_ms(&self) -> f64 {
        self.peak_delay.iter().copied().fold(0.0, f64::max) * 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_stream_keeps_small_target() {
        let mut j = JitterEstimator::new();
        for i in 0..1000 {
            let t = f64::from(i) * 0.02;
            j.on_packet(t + 0.05, t, 20.0);
        }
        assert!(j.target_ms() < 60.0, "{}", j.target_ms());
        assert!(j.jitter_ms() < 1.0);
    }

    #[test]
    fn burst_raises_target_and_it_decays() {
        let mut j = JitterEstimator::new();
        for i in 0..100 {
            let t = f64::from(i) * 0.02;
            j.on_packet(t, t, 20.0);
        }
        j.on_packet(2.0 + 0.2, 2.0, 20.0);
        assert!(j.target_ms() >= 200.0, "{}", j.target_ms());
        for i in 0..3000 {
            let t = 2.02 + f64::from(i) * 0.02;
            j.on_packet(t, t, 20.0);
        }
        assert!(j.target_ms() < 80.0, "{}", j.target_ms());
    }
}
