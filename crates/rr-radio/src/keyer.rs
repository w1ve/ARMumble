//! Station keyer: decides when the rig's key line is asserted.
//!
//! The rig is keyed when *either* a remote operator is holding PTT *or* VOX
//! hears inbound audio. Every key passes through the [`Watchdog`], so a stuck
//! request cannot hold the transmitter. Time is a parameter so every rule is
//! tested deterministically.

use std::time::{Duration, Instant};

use rr_ptt::{PttSink, Watchdog, WatchdogConfig};

use crate::settings::StationPttSettings;

/// Drives a rig's key line from remote PTT requests and inbound-audio VOX.
#[derive(Debug)]
pub struct StationKeyer<S: PttSink> {
    wd: Watchdog<S>,
    vox_enabled: bool,
    vox_hang: Duration,
    hang: Duration,
    deadman: Duration,
    remote_down: bool,
    remote_at: Option<Instant>,
    audio: bool,
    vox_until: Option<Instant>,
    release_at: Option<Instant>,
}

impl<S: PttSink> StationKeyer<S> {
    /// Wrap `sink` using the timing in `settings` (which are sanitised first).
    #[must_use]
    pub fn new(sink: S, settings: &StationPttSettings) -> Self {
        let s = settings.clone().sanitised();
        let wd = Watchdog::new(
            sink,
            WatchdogConfig {
                deadman: Duration::from_millis(u64::from(s.deadman_ms)),
                max_transmit: Duration::from_secs(u64::from(s.max_tx_secs)),
            },
        );
        Self {
            wd,
            vox_enabled: s.vox,
            vox_hang: Duration::from_millis(u64::from(s.vox_hang_ms)),
            hang: Duration::from_millis(u64::from(s.hang_ms)),
            deadman: Duration::from_millis(u64::from(s.deadman_ms)),
            remote_down: false,
            remote_at: None,
            audio: false,
            vox_until: None,
            release_at: None,
        }
    }

    /// A remote PTT message arrived. Repeats of `down = true` refresh the
    /// request; it lapses by itself if they stop.
    pub fn set_remote(&mut self, down: bool, now: Instant) {
        self.remote_down = down;
        self.remote_at = Some(now);
    }

    /// Inbound audio started (`true`) or stopped (`false`). Ignored unless VOX
    /// is enabled.
    pub fn set_audio_active(&mut self, active: bool, now: Instant) {
        if !self.vox_enabled {
            return;
        }
        if self.audio && !active {
            self.vox_until = Some(now + self.vox_hang);
        }
        if active {
            self.vox_until = None;
        }
        self.audio = active;
    }

    /// Whether the rig is currently keyed.
    #[must_use]
    pub fn is_keyed(&self) -> bool {
        self.wd.is_keyed()
    }

    /// Whether anything currently asks for the rig to be keyed.
    #[must_use]
    pub fn wants_key(&self, now: Instant) -> bool {
        let remote = self.remote_down
            && self.remote_at.is_some_and(|t| now.saturating_duration_since(t) < self.deadman);
        let vox = self.vox_enabled && (self.audio || self.vox_until.is_some_and(|t| now < t));
        remote || vox
    }

    /// Advance timers and drive the line. Call every 10 to 20 ms.
    pub fn tick(&mut self, now: Instant) {
        if self.wants_key(now) {
            self.release_at = None;
            match self.wd.key(now) {
                Ok(()) => self.wd.keepalive(now),
                Err(e) => tracing::warn!("PTT key failed (failed closed): {e}"),
            }
        } else if self.wd.is_keyed() {
            let at = *self.release_at.get_or_insert(now + self.hang);
            if now >= at {
                self.release_at = None;
                if let Err(e) = self.wd.unkey(now) {
                    tracing::warn!("PTT unkey failed (failed closed): {e}");
                }
            } else {
                // Holding through the hang time: keep the deadman fed.
                self.wd.keepalive(now);
            }
        } else {
            // Idle. Clearing is idempotent and also resets the max-transmit
            // latch, so a tripped limit does not block the next over.
            let _ = self.wd.unkey(now);
        }
        if let Err(e) = self.wd.tick(now) {
            tracing::warn!("PTT watchdog tick failed (failed closed): {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rr_ptt::RecordingPttSink;

    fn settings() -> StationPttSettings {
        StationPttSettings {
            enabled: true,
            vox: true,
            vox_hang_ms: 500,
            hang_ms: 100,
            deadman_ms: 2_000,
            max_tx_secs: 5,
            ..StationPttSettings::default()
        }
    }

    fn run(k: &mut StationKeyer<RecordingPttSink>, t0: Instant, from_ms: u64, to_ms: u64) {
        for ms in (from_ms..to_ms).step_by(15) {
            k.tick(t0 + Duration::from_millis(ms));
        }
    }

    #[test]
    fn remote_key_and_release_with_hang() {
        let t0 = Instant::now();
        let mut k = StationKeyer::new(RecordingPttSink::new(), &settings());
        k.set_remote(true, t0);
        run(&mut k, t0, 0, 300);
        assert!(k.is_keyed());
        k.set_remote(false, t0 + Duration::from_millis(300));
        run(&mut k, t0, 300, 360);
        assert!(k.is_keyed(), "hang time must hold the key");
        run(&mut k, t0, 360, 600);
        assert!(!k.is_keyed());
    }

    #[test]
    fn lost_release_expires_without_refresh() {
        let t0 = Instant::now();
        let mut k = StationKeyer::new(RecordingPttSink::new(), &settings());
        k.set_remote(true, t0);
        run(&mut k, t0, 0, 1_500);
        assert!(k.is_keyed());
        run(&mut k, t0, 1_500, 2_400);
        assert!(!k.is_keyed(), "request must lapse after the deadman");
    }

    #[test]
    fn refreshes_keep_a_long_over_alive() {
        let t0 = Instant::now();
        let mut k = StationKeyer::new(RecordingPttSink::new(), &settings());
        for ms in (0..4_000).step_by(15) {
            let now = t0 + Duration::from_millis(ms);
            if ms % 500 == 0 {
                k.set_remote(true, now);
            }
            k.tick(now);
        }
        assert!(k.is_keyed());
    }

    #[test]
    fn vox_keys_on_audio_and_holds_for_vox_hang() {
        let t0 = Instant::now();
        let mut k = StationKeyer::new(RecordingPttSink::new(), &settings());
        k.set_audio_active(true, t0);
        run(&mut k, t0, 0, 200);
        assert!(k.is_keyed());
        k.set_audio_active(false, t0 + Duration::from_millis(200));
        run(&mut k, t0, 200, 600);
        assert!(k.is_keyed(), "vox hang (500 ms) still running");
        run(&mut k, t0, 600, 1_000);
        assert!(!k.is_keyed());
    }

    #[test]
    fn vox_disabled_ignores_audio() {
        let t0 = Instant::now();
        let s = StationPttSettings { vox: false, ..settings() };
        let mut k = StationKeyer::new(RecordingPttSink::new(), &s);
        k.set_audio_active(true, t0);
        run(&mut k, t0, 0, 300);
        assert!(!k.is_keyed());
    }

    #[test]
    fn vox_and_remote_combine() {
        let t0 = Instant::now();
        let mut k = StationKeyer::new(RecordingPttSink::new(), &settings());
        k.set_audio_active(true, t0);
        k.set_remote(true, t0);
        run(&mut k, t0, 0, 100);
        k.set_audio_active(false, t0 + Duration::from_millis(100));
        run(&mut k, t0, 100, 1_000);
        // Remote request expired at 2 s; until then it alone keeps the key.
        assert!(k.is_keyed());
    }

    #[test]
    fn max_transmit_releases_and_next_over_works() {
        let t0 = Instant::now();
        let mut k = StationKeyer::new(RecordingPttSink::new(), &settings());
        for ms in (0..7_000).step_by(15) {
            let now = t0 + Duration::from_millis(ms);
            if ms % 500 == 0 {
                k.set_remote(true, now);
            }
            k.tick(now);
        }
        assert!(!k.is_keyed(), "5 s ceiling must release the rig");
        let t1 = t0 + Duration::from_secs(8);
        k.set_remote(false, t1);
        run(&mut k, t1, 0, 300);
        k.set_remote(true, t1 + Duration::from_millis(300));
        run(&mut k, t1, 300, 600);
        assert!(k.is_keyed(), "the limit must not block later transmissions");
    }
}
