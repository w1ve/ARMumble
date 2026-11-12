//! Multi-talker playout: jitter buffering, decoding, rate conversion and
//! mixing into the output device buffer.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::stream::{SpeakerStats, Stream};
use crate::util::AtomicF32;

/// Remove a silent talker after this long (s).
const IDLE_REMOVE_S: f64 = 10.0;
/// How often the statistics snapshot is refreshed (s of audio).
const STATS_PERIOD_S: f64 = 0.25;

/// How the stereo output is presented.
///
/// In two-radio operation the station sends radio 1 on the left channel and
/// radio 2 on the right, which the modes below select from or steer into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StereoMode {
    /// True stereo.
    Stereo,
    /// Left and right averaged to both sides (for a single speaker).
    Mono,
    /// The left channel (radio 1) in both ears.
    LeftOnly,
    /// The right channel (radio 2) in both ears.
    RightOnly,
    /// Everything, mixed to mono, on the left channel only; right is silent
    /// (a station whose radio 1 mic input is the left side of one card).
    ToLeft,
    /// Everything, mixed to mono, on the right channel only; left is silent.
    ToRight,
}

impl StereoMode {
    fn code(self) -> u8 {
        match self {
            Self::Stereo => 0,
            Self::Mono => 1,
            Self::LeftOnly => 2,
            Self::RightOnly => 3,
            Self::ToLeft => 4,
            Self::ToRight => 5,
        }
    }

    fn from_code(c: u8) -> Self {
        match c {
            1 => Self::Mono,
            2 => Self::LeftOnly,
            3 => Self::RightOnly,
            4 => Self::ToLeft,
            5 => Self::ToRight,
            _ => Self::Stereo,
        }
    }

    /// Present one stereo frame.
    #[inline]
    #[must_use]
    pub fn apply(self, l: f32, r: f32) -> (f32, f32) {
        match self {
            Self::Stereo => (l, r),
            Self::Mono => {
                let m = 0.5 * (l + r);
                (m, m)
            }
            Self::LeftOnly => (l, l),
            Self::RightOnly => (r, r),
            Self::ToLeft => (0.5 * (l + r), 0.0),
            Self::ToRight => (0.0, 0.5 * (l + r)),
        }
    }
}

#[derive(Debug)]
enum Msg {
    Packet { speaker: u32, seq: u64, data: Vec<u8>, terminator: bool, arrival_s: f64 },
    UserGain { speaker: u32, gain: f32 },
}

#[derive(Debug)]
struct Shared {
    master: AtomicF32,
    mode: AtomicU8,
    muted: AtomicBool,
    peak_l: AtomicF32,
    peak_r: AtomicF32,
    clips: AtomicU64,
    stats: Mutex<Vec<SpeakerStats>>,
}

/// Cloneable control/ingest handle for a [`Playout`].
#[derive(Debug, Clone)]
pub struct PlayoutHandle {
    tx: Sender<Msg>,
    shared: Arc<Shared>,
    epoch: Instant,
}

impl PlayoutHandle {
    /// Deliver a received voice packet, timestamped now.
    pub fn push_packet(&self, speaker: u32, seq: u64, data: Vec<u8>, terminator: bool) {
        self.push_at(speaker, seq, data, terminator, self.epoch.elapsed());
    }

    /// Deliver a packet with an explicit arrival time (for deterministic tests).
    pub fn push_at(&self, speaker: u32, seq: u64, data: Vec<u8>, terminator: bool, arrival: Duration) {
        // Mumble's own client flags the *last audio frame* as the terminator, so
        // a terminator may carry data: deliver the audio first, then the marker.
        if terminator && !data.is_empty() {
            let units = crate::codec::FrameDecoder::packet_frames(&data).map_or(2, |f| (f as u64 / 480).max(1));
            self.push_at(speaker, seq, data, false, arrival);
            self.push_at(speaker, seq + units, Vec::new(), true, arrival);
            return;
        }
        let _ = self.tx.send(Msg::Packet {
            speaker,
            seq,
            data,
            terminator,
            arrival_s: arrival.as_secs_f64(),
        });
    }

    /// Per-talker volume (linear, 1.0 = unity).
    pub fn set_user_gain(&self, speaker: u32, gain: f32) {
        let _ = self.tx.send(Msg::UserGain { speaker, gain: gain.clamp(0.0, 4.0) });
    }

    /// Master output volume (linear, 1.0 = unity).
    pub fn set_master_gain(&self, gain: f32) {
        self.shared.master.store(gain.clamp(0.0, 4.0));
    }

    /// Silence this output while it keeps running. Jitter buffers and drift
    /// tracking stay live, so un-muting is instant and click-free. Used to
    /// route audio between two output devices.
    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }

    /// Choose how the stereo output is presented.
    pub fn set_stereo_mode(&self, mode: StereoMode) {
        self.shared.mode.store(mode.code(), Ordering::Relaxed);
    }

    /// Output peak levels (linear, left and right) since the last call.
    #[must_use]
    pub fn take_peaks(&self) -> (f32, f32) {
        (self.shared.peak_l.take(), self.shared.peak_r.take())
    }

    /// Samples that exceeded full scale in the final mix and were clamped.
    #[must_use]
    pub fn clip_count(&self) -> u64 {
        self.shared.clips.load(Ordering::Relaxed)
    }

    /// Latest per-talker statistics.
    #[must_use]
    pub fn stats(&self) -> Vec<SpeakerStats> {
        self.shared.stats.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// The audio-thread side of playout.
#[derive(Debug)]
pub struct Playout {
    rx: Receiver<Msg>,
    shared: Arc<Shared>,
    out_rate: u32,
    streams: HashMap<u32, Stream>,
    gains: HashMap<u32, f32>,
    tmp: Vec<f32>,
    since_stats: f64,
}

/// Create a playout engine for an output device running at `out_rate`.
#[must_use]
pub fn playout(out_rate: u32) -> (Playout, PlayoutHandle) {
    let (tx, rx) = channel();
    let shared = Arc::new(Shared {
        master: AtomicF32::new(1.0),
        mode: AtomicU8::new(0),
        muted: AtomicBool::new(false),
        peak_l: AtomicF32::new(0.0),
        peak_r: AtomicF32::new(0.0),
        clips: AtomicU64::new(0),
        stats: Mutex::new(Vec::new()),
    });
    let handle = PlayoutHandle { tx, shared: Arc::clone(&shared), epoch: Instant::now() };
    let engine = Playout {
        rx,
        shared,
        out_rate,
        streams: HashMap::new(),
        gains: HashMap::new(),
        tmp: Vec::new(),
        since_stats: 0.0,
    };
    (engine, handle)
}

impl Playout {
    fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::UserGain { speaker, gain } => {
                    let _ = self.gains.insert(speaker, gain);
                }
                Msg::Packet { speaker, seq, data, terminator, arrival_s } => {
                    if let Some(s) = self.stream_for(speaker) {
                        s.push(seq, data, terminator, arrival_s);
                    }
                }
            }
        }
    }

    fn stream_for(&mut self, speaker: u32) -> Option<&mut Stream> {
        if let Entry::Vacant(slot) = self.streams.entry(speaker) {
            match Stream::new(speaker, self.out_rate) {
                Ok(s) => {
                    let _ = slot.insert(s);
                }
                Err(e) => {
                    tracing::warn!("cannot create stream for {speaker}: {e}");
                    return None;
                }
            }
        }
        self.streams.get_mut(&speaker)
    }

    /// Fill `out` (interleaved stereo at the output rate) with the mixed audio
    /// of all talkers. Call this from the audio callback.
    pub fn render(&mut self, out: &mut [f32]) {
        self.drain_messages();
        out.fill(0.0);
        let frames = out.len() / 2;
        let dt = frames as f64 / f64::from(self.out_rate);
        for (id, stream) in &mut self.streams {
            stream.pull(frames, dt, &mut self.tmp);
            let g = self.gains.get(id).copied().unwrap_or(1.0);
            for (o, s) in out.iter_mut().zip(&self.tmp) {
                *o += s * g;
            }
        }
        self.finish_mix(out);
        self.streams.retain(|_, s| !(s.is_finished() && s.idle_s > IDLE_REMOVE_S));
        self.since_stats += dt;
        if self.since_stats >= STATS_PERIOD_S {
            self.since_stats = 0.0;
            self.publish_stats();
        }
    }

    /// Master gain, optional mono fold-down, hard limit at full scale, meters.
    fn finish_mix(&self, out: &mut [f32]) {
        if self.shared.muted.load(Ordering::Relaxed) {
            out.fill(0.0);
            return;
        }
        let master = self.shared.master.load();
        let mode = StereoMode::from_code(self.shared.mode.load(Ordering::Relaxed));
        let (mut pl, mut pr) = (0.0_f32, 0.0_f32);
        let mut clips = 0_u64;
        for f in out.as_chunks_mut::<2>().0.iter_mut() {
            let (l, r) = mode.apply(f[0] * master, f[1] * master);
            clips += u64::from(l.abs() > 1.0) + u64::from(r.abs() > 1.0);
            f[0] = l.clamp(-1.0, 1.0);
            f[1] = r.clamp(-1.0, 1.0);
            pl = pl.max(f[0].abs());
            pr = pr.max(f[1].abs());
        }
        self.shared.peak_l.max_assign(pl);
        self.shared.peak_r.max_assign(pr);
        if clips > 0 {
            let _ = self.shared.clips.fetch_add(clips, Ordering::Relaxed);
        }
    }

    fn publish_stats(&self) {
        if let Ok(mut guard) = self.shared.stats.try_lock() {
            guard.clear();
            guard.extend(self.streams.values().map(|s| s.stats.clone()));
        }
    }
}
