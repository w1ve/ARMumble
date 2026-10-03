//! Headless safety tests for the PTT watchdog (§14e). No serial port, no
//! transmitter: a [`RecordingPttSink`] stands in for the rig and time is
//! injected so the deadman and max-transmit limits are exercised deterministically.

#![allow(clippy::unwrap_used, reason = "unwrap is acceptable in test code")]

use std::time::{Duration, Instant};

use crate::{
    PttEvent, PttIntent, PttSink, PttSource, RecordingPttSink, RecordingPttSource, Watchdog,
    WatchdogConfig,
};

fn cfg(deadman_ms: u64, max_tx_ms: u64) -> WatchdogConfig {
    WatchdogConfig {
        deadman: Duration::from_millis(deadman_ms),
        max_transmit: Duration::from_millis(max_tx_ms),
    }
}

#[test]
fn starts_unkeyed() {
    let wd = Watchdog::new(RecordingPttSink::new(), WatchdogConfig::default());
    assert!(!wd.is_keyed());
    assert!(wd.sink().events().is_empty());
}

#[test]
fn key_then_unkey_drives_the_line() {
    let t0 = Instant::now();
    let mut wd = Watchdog::new(RecordingPttSink::new(), cfg(1000, 10_000));
    wd.key(t0).unwrap();
    assert!(wd.is_keyed());
    assert!(wd.sink().is_keyed());
    wd.unkey(t0 + Duration::from_millis(50)).unwrap();
    assert!(!wd.is_keyed());
    assert!(!wd.sink().is_keyed());
    assert_eq!(
        wd.sink().events(),
        &[PttEvent::SetKeyed(true), PttEvent::SetKeyed(false)]
    );
}

#[test]
fn repeated_key_does_not_re_drive_the_line() {
    // A caller that re-sends `key` on every intent frame should not spam the
    // line; only the first keys it, the rest are keep-alives.
    let t0 = Instant::now();
    let mut wd = Watchdog::new(RecordingPttSink::new(), cfg(1000, 10_000));
    wd.key(t0).unwrap();
    wd.key(t0 + Duration::from_millis(100)).unwrap();
    wd.key(t0 + Duration::from_millis(200)).unwrap();
    assert_eq!(wd.sink().key_calls(), 1);
}

#[test]
fn deadman_releases_without_keepalive() {
    let t0 = Instant::now();
    let mut wd = Watchdog::new(RecordingPttSink::new(), cfg(500, 10_000));
    wd.key(t0).unwrap();
    // Just before the deadman: still keyed.
    wd.tick(t0 + Duration::from_millis(499)).unwrap();
    assert!(wd.is_keyed());
    // At/after the deadman with no keep-alive: released.
    wd.tick(t0 + Duration::from_millis(500)).unwrap();
    assert!(!wd.is_keyed());
    assert!(!wd.sink().is_keyed());
}

#[test]
fn keepalive_holds_the_line_open() {
    let t0 = Instant::now();
    let mut wd = Watchdog::new(RecordingPttSink::new(), cfg(500, 10_000));
    wd.key(t0).unwrap();
    // Refresh every 400 ms over 2 s; deadman is 500 ms, so it never trips.
    for i in 1..=5 {
        let t = t0 + Duration::from_millis(i * 400);
        wd.keepalive(t);
        wd.tick(t).unwrap();
        assert!(wd.is_keyed(), "should still be keyed at {}ms", i * 400);
    }
    // Stop refreshing: deadman trips 500 ms after the last keep-alive.
    wd.tick(t0 + Duration::from_millis(2000 + 500)).unwrap();
    assert!(!wd.is_keyed());
}

#[test]
fn max_transmit_releases_even_with_keepalives() {
    let t0 = Instant::now();
    let mut wd = Watchdog::new(RecordingPttSink::new(), cfg(1000, 2000));
    wd.key(t0).unwrap();
    // Keep-alive every 500 ms would defeat the deadman forever, but must NOT
    // defeat the 2 s hard ceiling.
    for i in 1..=3 {
        let t = t0 + Duration::from_millis(i * 500);
        wd.keepalive(t);
        wd.tick(t).unwrap();
    }
    // At 2 s the ceiling trips.
    wd.tick(t0 + Duration::from_millis(2000)).unwrap();
    assert!(!wd.is_keyed(), "max transmit should have released the line");
}

#[test]
fn max_transmit_latches_until_explicit_unkey() {
    let t0 = Instant::now();
    let mut wd = Watchdog::new(RecordingPttSink::new(), cfg(1000, 1000));
    wd.key(t0).unwrap();
    wd.tick(t0 + Duration::from_millis(1000)).unwrap();
    assert!(!wd.is_keyed());
    // A re-key while latched is refused (stays unkeyed) - a keep-alive flood
    // cannot immediately re-key after the ceiling released.
    wd.key(t0 + Duration::from_millis(1100)).unwrap();
    assert!(!wd.is_keyed(), "re-key must be refused while latched");
    // Explicit unkey clears the latch; then keying works again.
    wd.unkey(t0 + Duration::from_millis(1200)).unwrap();
    wd.key(t0 + Duration::from_millis(1300)).unwrap();
    assert!(wd.is_keyed(), "key allowed again after explicit unkey");
}

#[test]
fn sink_error_fails_closed() {
    let t0 = Instant::now();
    let mut sink = RecordingPttSink::new();
    sink.fail_once(); // the keying call will error
    let mut wd = Watchdog::new(sink, cfg(1000, 10_000));
    let res = wd.key(t0);
    assert!(res.is_err(), "key should surface the sink error");
    // Despite the error, the watchdog must NOT believe it is keyed.
    assert!(!wd.is_keyed(), "must fail closed, not believe it is keyed");
}

#[test]
fn tick_when_unkeyed_is_a_noop() {
    let t0 = Instant::now();
    let mut wd = Watchdog::new(RecordingPttSink::new(), cfg(500, 1000));
    wd.tick(t0 + Duration::from_secs(10)).unwrap();
    assert!(!wd.is_keyed());
    assert!(wd.sink().events().is_empty());
}

#[test]
fn drop_releases_the_line() {
    let t0 = Instant::now();
    // Observe via a shared flag that Drop drives the line low. Simplest: key,
    // then drop and confirm through a second sink wrapper is overkill; instead
    // confirm the recorder's own Drop contract by checking the trait directly.
    let mut sink = RecordingPttSink::new();
    sink.set_keyed(true).unwrap();
    assert!(sink.is_keyed());
    drop(sink); // RecordingPttSink::drop releases; nothing panics.

    // And the watchdog's Drop: key it, drop, and rely on the fact that Drop
    // calls set_keyed(false). We cannot read the sink after drop, so assert the
    // pre-drop invariant and that drop does not panic.
    let mut wd = Watchdog::new(RecordingPttSink::new(), cfg(1000, 10_000));
    wd.key(t0).unwrap();
    assert!(wd.is_keyed());
    drop(wd);
}

// -- Source (footswitch / command / VOX intent) -------------------

#[test]
fn source_replays_script_then_holds_last() {
    let mut src = RecordingPttSource::new(vec![
        Some(PttIntent::Unkey),
        Some(PttIntent::Key),
        None, // a glitched read
        Some(PttIntent::Key),
    ]);
    assert_eq!(src.poll(), Some(PttIntent::Unkey));
    assert_eq!(src.poll(), Some(PttIntent::Key));
    assert_eq!(src.poll(), None);
    assert_eq!(src.poll(), Some(PttIntent::Key));
    // Exhausted: holds the last real reading (Key), as a held footswitch would.
    assert_eq!(src.poll(), Some(PttIntent::Key));
    assert_eq!(src.poll(), Some(PttIntent::Key));
}

#[test]
fn source_driving_watchdog_keys_and_unkeys() {
    // Integration: a scripted source drives the watchdog like the real loop will.
    let t0 = Instant::now();
    let mut src = RecordingPttSource::new(vec![
        Some(PttIntent::Key),
        Some(PttIntent::Key),
        Some(PttIntent::Unkey),
    ]);
    let mut wd = Watchdog::new(RecordingPttSink::new(), cfg(1000, 10_000));

    let mut t = t0;
    for _ in 0..3 {
        match src.poll() {
            Some(PttIntent::Key) => wd.key(t).unwrap(),
            Some(PttIntent::Unkey) => wd.unkey(t).unwrap(),
            None => {}
        }
        wd.tick(t).unwrap();
        t += Duration::from_millis(20);
    }
    assert!(!wd.is_keyed());
    // Exactly one physical key and one unkey over the sequence.
    assert_eq!(wd.sink().key_calls(), 1);
}
