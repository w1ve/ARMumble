//! Worker threads that run the keyer and the footswitch poller.

use std::sync::mpsc::{channel, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rr_ptt::{PttSink, PttSource};

use crate::keyer::StationKeyer;
use crate::message::{PttMessage, REFRESH_MS};
use crate::settings::StationPttSettings;

/// Worker tick interval. Imperceptible for PTT, negligible serial traffic.
const TICK: Duration = Duration::from_millis(15);

enum Cmd {
    Remote(bool),
    Audio(bool),
    Stop,
}

/// Handle to the thread that keys the rig. Dropping it releases the key line.
#[derive(Debug)]
pub struct StationPtt {
    tx: Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
}

impl StationPtt {
    /// Start keying `sink` according to `settings`.
    ///
    /// # Errors
    /// Returns the OS error if the worker thread cannot be spawned.
    pub fn spawn<S>(sink: S, settings: &StationPttSettings) -> std::io::Result<Self>
    where
        S: PttSink + Send + 'static,
    {
        let (tx, rx) = channel::<Cmd>();
        let settings = settings.clone();
        let thread = thread::Builder::new().name("rr-ptt-station".into()).spawn(move || {
            let mut keyer = StationKeyer::new(sink, &settings);
            loop {
                loop {
                    match rx.try_recv() {
                        Ok(Cmd::Remote(d)) => keyer.set_remote(d, Instant::now()),
                        Ok(Cmd::Audio(a)) => keyer.set_audio_active(a, Instant::now()),
                        Ok(Cmd::Stop) | Err(TryRecvError::Disconnected) => return,
                        Err(TryRecvError::Empty) => break,
                    }
                }
                keyer.tick(Instant::now());
                thread::sleep(TICK);
            }
        })?;
        Ok(Self { tx, thread: Some(thread) })
    }

    /// A remote PTT message arrived (already sequence-filtered).
    pub fn set_remote(&self, down: bool) {
        let _ = self.tx.send(Cmd::Remote(down));
    }

    /// Inbound audio started or stopped (for VOX).
    pub fn set_audio_active(&self, active: bool) {
        let _ = self.tx.send(Cmd::Audio(active));
    }
}

impl Drop for StationPtt {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Handle to the footswitch poller. Dropping it stops the thread.
#[derive(Debug)]
pub struct LocalPtt {
    tx: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl LocalPtt {
    /// Poll `source` and call `on_message` whenever the footswitch changes, and
    /// again every [`REFRESH_MS`] while it stays down.
    ///
    /// # Errors
    /// Returns the OS error if the worker thread cannot be spawned.
    pub fn spawn<P, F>(mut source: P, on_message: F) -> std::io::Result<Self>
    where
        P: PttSource + Send + 'static,
        F: Fn(PttMessage) + Send + 'static,
    {
        let (tx, rx) = channel::<()>();
        let thread = thread::Builder::new().name("rr-ptt-local".into()).spawn(move || {
            let mut down = false;
            let mut seq = 0_u64;
            let mut last_sent = Instant::now();
            loop {
                match rx.recv_timeout(TICK) {
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                }
                let Some(intent) = source.poll() else { continue };
                let now = Instant::now();
                let changed = intent.is_key() != down;
                let refresh = down && now.duration_since(last_sent).as_millis() >= u128::from(REFRESH_MS);
                if changed {
                    down = intent.is_key();
                    seq += 1;
                }
                if changed || refresh {
                    last_sent = now;
                    on_message(PttMessage { down, seq });
                }
            }
        })?;
        Ok(Self { tx, thread: Some(thread) })
    }
}

impl Drop for LocalPtt {
    fn drop(&mut self) {
        let _ = self.tx.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use rr_ptt::{PttIntent, RecordingPttSource};

    use super::*;

    #[test]
    fn footswitch_reports_edges_and_refreshes() {
        // Released, pressed for a long time, released.
        let mut script = vec![Some(PttIntent::Unkey); 3];
        script.extend(vec![Some(PttIntent::Key); 90]);
        script.extend(vec![Some(PttIntent::Unkey); 5]);
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&got);
        let local = LocalPtt::spawn(RecordingPttSource::new(script), move |m| {
            if let Ok(mut v) = sink.lock() {
                v.push(m);
            }
        })
        .expect("spawn");
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
            if got.lock().is_ok_and(|v| v.last().is_some_and(|m| !m.down)) {
                break;
            }
        }
        drop(local);
        let msgs = got.lock().map(|v| v.clone()).unwrap_or_default();
        assert!(msgs.first().is_some_and(|m| m.down), "{msgs:?}");
        assert!(msgs.iter().filter(|m| m.down).count() >= 2, "needs a refresh: {msgs:?}");
        assert!(msgs.last().is_some_and(|m| !m.down), "{msgs:?}");
        assert!(msgs.windows(2).all(|w| w[1].seq >= w[0].seq));
    }
}
