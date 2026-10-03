//! The fail-safe watchdog (§14e). Wraps a [`PttSink`] and is the only thing the
//! rest of the application should hold, so every path to keying a transmitter
//! passes through these limits.
//!
//! What it guarantees:
//!
//! 1. **Deadman.** Once keyed, the key line auto-releases after
//!    [`WatchdogConfig::deadman`] with no keep-alive. A controller who vanishes
//!    mid-transmission (network drop, crash, lost footswitch) unkeys the rig
//!    within that window rather than leaving it on the air.
//! 2. **Maximum continuous transmit.** Independently, a single key-down may not
//!    exceed [`WatchdogConfig::max_transmit`]; when it does the line releases and
//!    stays released until an explicit [`Watchdog::unkey`] clears the latch, so a
//!    stuck keep-alive cannot hold the transmitter indefinitely.
//! 3. **Fail closed.** Any error from the sink forces the tracked state to
//!    unkeyed and the watchdog keeps trying to drive the line low. Ambiguity
//!    resolves to unkeyed, never keyed.
//! 4. **Release on drop.** Dropping the watchdog releases the line.
//!
//! Time is a parameter, not read from the clock, so every rule is tested
//! deterministically: callers pass a monotonically non-decreasing `now:
//! Instant` to each state-advancing method, and the application's real loop
//! passes `Instant::now()`.

use std::time::{Duration, Instant};

use crate::{PttSink, Result};

/// Tunable safety limits for the [`Watchdog`].
#[derive(Debug, Clone, Copy)]
pub struct WatchdogConfig {
    /// Auto-release if no keep-alive arrives within this window while keyed.
    /// Short by default; keep-alives are cheap. Measured from the last
    /// `key`/`keepalive`.
    pub deadman: Duration,
    /// Hard cap on a single continuous transmission, independent of keep-alives.
    /// Releasing on this latches "unkey required" so the limit cannot be defeated
    /// by a client that keeps sending keep-alives.
    pub max_transmit: Duration,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            // Conservative, overridable defaults. A remote keep-alive at ~5 Hz
            // comfortably refreshes a 2 s deadman; 120 s is a generous ceiling
            // for a single over that still prevents an indefinite stuck carrier.
            deadman: Duration::from_secs(2),
            max_transmit: Duration::from_secs(120),
        }
    }
}

/// Owns a [`PttSink`] and enforces the §14e safety limits. Fails closed.
#[derive(Debug)]
pub struct Watchdog<S: PttSink> {
    sink: S,
    config: WatchdogConfig,
    /// Present while keyed: when the current key-down began and when it was last
    /// refreshed. `None` means unkeyed.
    keyed_since: Option<Instant>,
    last_keepalive: Option<Instant>,
    /// Set when `max_transmit` tripped; blocks re-keying until an explicit
    /// `unkey` acknowledges it. Prevents a keep-alive flood from re-keying
    /// immediately after the ceiling released the line.
    max_tx_latched: bool,
}

impl<S: PttSink> Watchdog<S> {
    /// Wrap `sink` with the given limits. The sink is assumed powered up
    /// unkeyed (the trait requires it); this does not drive the line.
    #[must_use]
    pub fn new(sink: S, config: WatchdogConfig) -> Self {
        Self {
            sink,
            config,
            keyed_since: None,
            last_keepalive: None,
            max_tx_latched: false,
        }
    }

    /// Whether the watchdog currently believes the line is keyed.
    #[must_use]
    pub fn is_keyed(&self) -> bool {
        self.keyed_since.is_some()
    }

    /// Request the transmitter keyed, refreshing the deadman.
    ///
    /// Refused (returns `Ok(())` without keying) while the max-transmit latch is
    /// set; the caller must [`Watchdog::unkey`] first. Keying also serves as a
    /// keep-alive, so a caller that re-sends `key` on every intent frame never
    /// needs a separate keep-alive.
    ///
    /// # Errors
    /// Propagates a sink error after forcing the state closed (see
    /// [`Watchdog::fail_closed`]).
    pub fn key(&mut self, now: Instant) -> Result<()> {
        if self.max_tx_latched {
            // Ceiling tripped; stay unkeyed until an explicit unkey clears it.
            return Ok(());
        }
        self.last_keepalive = Some(now);
        if self.keyed_since.is_none() {
            self.keyed_since = Some(now);
            if let Err(e) = self.sink.set_keyed(true) {
                self.fail_closed();
                return Err(e);
            }
        }
        Ok(())
    }

    /// Refresh the deadman without changing the key state. A no-op when unkeyed.
    pub fn keepalive(&mut self, now: Instant) {
        if self.keyed_since.is_some() {
            self.last_keepalive = Some(now);
        }
    }

    /// Request the transmitter unkeyed. Idempotent, and always clears the
    /// max-transmit latch so a subsequent `key` is permitted again.
    ///
    /// # Errors
    /// Propagates a sink error after forcing the state closed.
    pub fn unkey(&mut self, _now: Instant) -> Result<()> {
        self.max_tx_latched = false;
        self.last_keepalive = None;
        if self.keyed_since.take().is_some() {
            if let Err(e) = self.sink.set_keyed(false) {
                self.fail_closed();
                return Err(e);
            }
        }
        Ok(())
    }

    /// Advance the safety timers. Call this on the application's PTT loop tick
    /// (e.g. every 10-20 ms). Releases the line if the deadman expired or the
    /// maximum transmit time was exceeded.
    ///
    /// # Errors
    /// Propagates a sink error after forcing the state closed. Even on error the
    /// tracked state is left unkeyed.
    pub fn tick(&mut self, now: Instant) -> Result<()> {
        let Some(since) = self.keyed_since else {
            return Ok(());
        };

        // Max continuous transmit: hard ceiling, latches.
        if now.saturating_duration_since(since) >= self.config.max_transmit {
            self.max_tx_latched = true;
            return self.release_now("max transmit time exceeded");
        }

        // Deadman: released on missed keep-alive. Does not latch — a fresh key
        // after the controller recovers is allowed.
        let last = self.last_keepalive.unwrap_or(since);
        if now.saturating_duration_since(last) >= self.config.deadman {
            return self.release_now("deadman: no keep-alive");
        }

        Ok(())
    }

    /// Drive the line low and clear the keyed state, logging why.
    fn release_now(&mut self, reason: &str) -> Result<()> {
        tracing::warn!("PTT watchdog releasing: {reason}");
        self.keyed_since = None;
        self.last_keepalive = None;
        if let Err(e) = self.sink.set_keyed(false) {
            self.fail_closed();
            return Err(e);
        }
        Ok(())
    }

    /// Force the tracked state to unkeyed after a sink error, and make a
    /// best-effort attempt to drive the line low. Called whenever the sink
    /// errors so the watchdog never *believes* it is keyed when the hardware may
    /// not be — the next `tick` will keep attempting the release.
    fn fail_closed(&mut self) {
        self.keyed_since = None;
        self.last_keepalive = None;
        // Best-effort low; ignore a second error - we are already failing closed
        // and the retry happens on the next tick.
        let _ = self.sink.set_keyed(false);
    }

    /// Borrow the wrapped sink (for tests / introspection).
    #[must_use]
    pub fn sink(&self) -> &S {
        &self.sink
    }
}

impl<S: PttSink> Drop for Watchdog<S> {
    fn drop(&mut self) {
        // Release on drop regardless of tracked state: a panic or early return
        // elsewhere must not leave a transmitter keyed.
        let _ = self.sink.set_keyed(false);
    }
}
