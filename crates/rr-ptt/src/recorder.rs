//! Test doubles: a [`PttSink`] that records what it was told to do instead of
//! driving a radio, and a [`PttSource`] that replays a scripted sequence of
//! intents. Together they let the [`Watchdog`](crate::Watchdog) timers and
//! fail-closed behaviour be tested with no serial port and no transmitter.

use crate::{PttError, PttIntent, PttSink, PttSource, Result};

/// A recorded [`PttSink`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PttEvent {
    /// `set_keyed(down)` was called.
    SetKeyed(bool),
}

/// A [`PttSink`] that records every call rather than driving hardware.
///
/// Optionally fails the next `set_keyed` (via [`RecordingPttSink::fail_once`])
/// so the watchdog's fail-closed handling of a serial error can be exercised.
#[derive(Debug, Default)]
pub struct RecordingPttSink {
    events: Vec<PttEvent>,
    keyed: bool,
    fail_next: bool,
}

impl RecordingPttSink {
    /// A fresh recorder, powered up unkeyed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Make the next [`PttSink::set_keyed`] call return an error, to test the
    /// watchdog failing closed on a control-line error.
    pub fn fail_once(&mut self) {
        self.fail_next = true;
    }

    /// Every recorded call, in order.
    #[must_use]
    pub fn events(&self) -> &[PttEvent] {
        &self.events
    }

    /// The current key state as last successfully set.
    #[must_use]
    pub fn is_keyed(&self) -> bool {
        self.keyed
    }

    /// How many times the line was driven keyed (rising transitions included —
    /// every `set_keyed(true)` call, not deduplicated).
    #[must_use]
    pub fn key_calls(&self) -> usize {
        self.events
            .iter()
            .filter(|e| matches!(e, PttEvent::SetKeyed(true)))
            .count()
    }
}

impl PttSink for RecordingPttSink {
    fn set_keyed(&mut self, down: bool) -> Result<()> {
        if self.fail_next {
            self.fail_next = false;
            // Record the attempt so a test can see the watchdog tried, then fail.
            self.events.push(PttEvent::SetKeyed(down));
            return Err(PttError::Io("recording sink: forced failure".into()));
        }
        self.events.push(PttEvent::SetKeyed(down));
        self.keyed = down;
        Ok(())
    }
}

impl Drop for RecordingPttSink {
    fn drop(&mut self) {
        // Mirror the real sink's contract: release on drop. Best-effort; a test
        // can inspect `events` to confirm the final state.
        if self.keyed {
            let _ = self.set_keyed(false);
        }
    }
}

/// A [`PttSource`] that replays a scripted list of intents, one per `poll`.
///
/// Once the script is exhausted it reports the last intent (so a held key stays
/// held), which matches how a real footswitch would keep reading the same level.
/// A `None` entry models a transient failed read for that cycle.
#[derive(Debug, Default)]
pub struct RecordingPttSource {
    script: Vec<Option<PttIntent>>,
    pos: usize,
    last: Option<PttIntent>,
}

impl RecordingPttSource {
    /// Build a source that yields `script` in order, then repeats the final
    /// readable intent forever.
    #[must_use]
    pub fn new(script: Vec<Option<PttIntent>>) -> Self {
        Self { script, pos: 0, last: None }
    }
}

impl PttSource for RecordingPttSource {
    fn poll(&mut self) -> Option<PttIntent> {
        let item = if self.pos < self.script.len() {
            let v = self.script[self.pos];
            self.pos += 1;
            v
        } else {
            // Exhausted: keep reporting the last real reading (held level).
            self.last.map(Some).unwrap_or(None)
        };
        if let Some(i) = item {
            self.last = Some(i);
        }
        item
    }
}
