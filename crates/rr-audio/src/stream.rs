//! One remote talker: jitter buffer, decoder, concealment and drift control.

use std::collections::BTreeMap;

use crate::asrc::Resampler;
use crate::codec::{FrameDecoder, SAMPLE_RATE};
use crate::error::Result;
use crate::jitter::JitterEstimator;

/// Largest buffering we prime to before starting playback (ms).
const MAX_PRIME_MS: f64 = 300.0;
/// Give up waiting to prime after this long (s).
const PRIME_TIMEOUT_S: f64 = 0.6;
/// Consecutive concealed frames before we stop and re-prime.
const MAX_PLC_RUN: u32 = 3;
/// A gap in sequence numbers larger than this (10 ms units) is a restart.
const GAP_RESYNC_UNITS: u64 = 30;
/// Packets this far behind (10 ms units) mean the sender restarted its counter.
const FAR_BEHIND_UNITS: u64 = 100;
/// Buffered audio beyond this many ms over the target is discarded.
const OVERFLOW_MS: f64 = 1_200.0;
/// Hard bound on queued packets.
const MAX_QUEUE: usize = 600;
/// Time constant for smoothing the buffer depth (s).
const DEPTH_TAU_S: f64 = 2.0;
/// Depth error that is tolerated without adjusting the rate (ms).
const DEADBAND_MS: f64 = 10.0;
/// Time over which a depth error is worked off (ms).
const CORRECTION_SPAN_MS: f64 = 8_000.0;
/// Largest rate adjustment (fraction).
const MAX_RATE_ADJUST: f64 = 0.004;

/// Counters and gauges for one talker.
#[derive(Debug, Clone, Default)]
pub struct SpeakerStats {
    /// Mumble session id of the talker.
    pub speaker: u32,
    /// True while audio is flowing from this talker.
    pub active: bool,
    /// Buffered audio in milliseconds (smoothed).
    pub depth_ms: f32,
    /// Buffering target in milliseconds.
    pub target_ms: f32,
    /// Largest recent network delay variation in milliseconds.
    pub jitter_ms: f32,
    /// Drift correction currently applied, in parts per million.
    pub ratio_ppm: f32,
    /// Packets received.
    pub packets: u64,
    /// Frames concealed because the packet never arrived in time.
    pub lost: u64,
    /// Packets that arrived too late to use.
    pub late: u64,
    /// Times the buffer ran dry.
    pub underruns: u64,
    /// Times the buffer overflowed and was trimmed.
    pub overflows: u64,
    /// Times the stream was re-synchronised after a gap.
    pub resyncs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Priming,
    Playing,
}

#[derive(Debug)]
struct Queued {
    data: Vec<u8>,
    units: u64,
}

/// Receive-side state for one talker.
#[derive(Debug)]
pub(crate) struct Stream {
    dec: FrameDecoder,
    rs: Resampler,
    est: JitterEstimator,
    queue: BTreeMap<u64, Queued>,
    state: State,
    next_seq: u64,
    last_units: u64,
    term_seq: Option<u64>,
    plc_run: u32,
    far_behind: u32,
    prime_wait_s: f64,
    fifo: Vec<f32>,
    fifo_pos: usize,
    tmp: Vec<f32>,
    rs_out: Vec<f32>,
    nominal_ratio: f64,
    out_rate: u32,
    depth_avg_ms: f64,
    pub(crate) idle_s: f64,
    pub(crate) stats: SpeakerStats,
}

impl Stream {
    pub(crate) fn new(speaker: u32, out_rate: u32) -> Result<Self> {
        let rs = Resampler::new(2, SAMPLE_RATE, out_rate);
        Ok(Self {
            dec: FrameDecoder::new()?,
            nominal_ratio: rs.ratio(),
            rs,
            est: JitterEstimator::new(),
            queue: BTreeMap::new(),
            state: State::Idle,
            next_seq: 0,
            last_units: 2,
            term_seq: None,
            plc_run: 0,
            far_behind: 0,
            prime_wait_s: 0.0,
            fifo: Vec::new(),
            fifo_pos: 0,
            tmp: Vec::new(),
            rs_out: Vec::new(),
            out_rate,
            depth_avg_ms: 0.0,
            idle_s: 0.0,
            stats: SpeakerStats { speaker, ..SpeakerStats::default() },
        })
    }

    /// True when nothing is playing or buffered.
    pub(crate) fn is_finished(&self) -> bool {
        self.state == State::Idle && self.fifo_frames() == 0
    }

    fn fifo_frames(&self) -> usize {
        (self.fifo.len() - self.fifo_pos) / 2
    }

    fn span_units(&self) -> u64 {
        match self.queue.iter().next_back() {
            Some((k, q)) => (k + q.units).saturating_sub(self.next_seq),
            None => 0,
        }
    }

    fn depth_ms(&self) -> f64 {
        self.span_units() as f64 * 10.0 + self.fifo_frames() as f64 * 1000.0 / f64::from(self.out_rate)
    }

    fn begin_talkspurt(&mut self) {
        self.state = State::Priming;
        self.queue.clear();
        self.term_seq = None;
        self.plc_run = 0;
        self.far_behind = 0;
        self.prime_wait_s = 0.0;
        self.est.reset_baseline();
        if let Err(e) = self.dec.reset() {
            tracing::warn!("decoder reset failed: {e}");
        }
    }

    /// Accept a packet from the network.
    pub(crate) fn push(&mut self, seq: u64, data: Vec<u8>, terminator: bool, arrival_s: f64) {
        self.idle_s = 0.0;
        if terminator {
            if self.state != State::Idle {
                self.term_seq = Some(seq);
            }
            return;
        }
        let Ok(frames) = FrameDecoder::packet_frames(&data) else {
            return;
        };
        let units = (frames as u64 / 480).max(1);
        self.stats.packets += 1;
        if self.state == State::Idle {
            self.begin_talkspurt();
        }
        self.est.on_packet(arrival_s, seq as f64 * 0.01, units as f64 * 10.0);
        self.stats.jitter_ms = self.est.jitter_ms() as f32;
        self.stats.target_ms = self.est.target_ms() as f32;
        self.last_units = units;
        if self.state == State::Playing && seq < self.next_seq {
            self.handle_late(seq, data, units);
            return;
        }
        let _ = self.queue.insert(seq, Queued { data, units });
        if self.state == State::Priming {
            if let Some((first, _)) = self.queue.iter().next() {
                self.next_seq = *first;
            }
        } else {
            self.trim_overflow();
        }
        while self.queue.len() > MAX_QUEUE {
            let _ = self.queue.pop_first();
        }
    }

    fn handle_late(&mut self, seq: u64, data: Vec<u8>, units: u64) {
        if seq + FAR_BEHIND_UNITS < self.next_seq {
            // The sender restarted its counter: treat as a new talk-spurt.
            self.far_behind += 1;
            if self.far_behind >= 3 {
                self.stats.resyncs += 1;
                self.begin_talkspurt();
                self.next_seq = seq;
                let _ = self.queue.insert(seq, Queued { data, units });
            }
        } else {
            self.stats.late += 1;
        }
    }

    fn trim_overflow(&mut self) {
        let limit = self.est.target_ms() + OVERFLOW_MS;
        if self.span_units() as f64 * 10.0 <= limit {
            return;
        }
        let Some((k, q)) = self.queue.iter().next_back().map(|(k, q)| (*k, q.units)) else {
            return;
        };
        let keep_units = (self.est.target_ms() / 10.0) as u64;
        let new_next = (k + q).saturating_sub(keep_units);
        self.queue.retain(|s, _| *s >= new_next);
        if let Some((first, _)) = self.queue.iter().next() {
            self.next_seq = *first;
        }
        self.stats.overflows += 1;
    }

    fn maybe_start(&mut self) {
        if self.state != State::Priming || self.queue.is_empty() {
            return;
        }
        let need = self.est.target_ms().min(MAX_PRIME_MS);
        if self.span_units() as f64 * 10.0 >= need || self.prime_wait_s >= PRIME_TIMEOUT_S {
            self.state = State::Playing;
            self.depth_avg_ms = self.est.target_ms();
            self.rs.set_ratio(self.nominal_ratio);
        }
    }

    /// Adjust the conversion ratio so the buffer depth converges on the target.
    fn steer(&mut self, dt: f64) {
        let alpha = 1.0 - (-dt / DEPTH_TAU_S).exp();
        self.depth_avg_ms += (self.depth_ms() - self.depth_avg_ms) * alpha;
        let err = self.depth_avg_ms - self.est.target_ms();
        let eff = err.signum() * (err.abs() - DEADBAND_MS).max(0.0);
        let adjust = (eff / CORRECTION_SPAN_MS).clamp(-MAX_RATE_ADJUST, MAX_RATE_ADJUST);
        self.rs.set_ratio(self.nominal_ratio * (1.0 + adjust));
        self.stats.ratio_ppm = (adjust * 1e6) as f32;
        self.stats.depth_ms = self.depth_avg_ms as f32;
    }

    /// Decode (or conceal) the next frame into the output FIFO. Returns false
    /// when no audio could be produced.
    fn produce(&mut self) -> bool {
        self.maybe_start();
        if self.state != State::Playing {
            return false;
        }
        if self.queue.is_empty() && self.term_seq.is_some_and(|t| self.next_seq >= t) {
            self.finish();
            return false;
        }
        self.tmp.clear();
        let decoded = if let Some(pkt) = self.queue.remove(&self.next_seq) {
            self.plc_run = 0;
            self.next_seq += pkt.units;
            self.dec.decode(&pkt.data, &mut self.tmp).is_ok()
        } else {
            self.conceal()
        };
        if !decoded || self.tmp.is_empty() {
            return self.state == State::Playing && decoded;
        }
        self.rs_out.clear();
        self.rs.process(&self.tmp, &mut self.rs_out);
        self.fifo.extend_from_slice(&self.rs_out);
        true
    }

    /// Fill `self.tmp` with a concealed frame, or switch to re-priming.
    fn conceal(&mut self) -> bool {
        if let Some((first, _)) = self.queue.iter().next() {
            let gap = *first - self.next_seq;
            if gap > GAP_RESYNC_UNITS {
                self.next_seq = *first;
                self.stats.resyncs += 1;
                return true;
            }
        } else if self.plc_run >= MAX_PLC_RUN {
            self.state = State::Priming;
            self.prime_wait_s = 0.0;
            self.stats.underruns += 1;
            return false;
        }
        self.plc_run += 1;
        self.stats.lost += 1;
        let units = self.last_units;
        self.next_seq += units;
        self.dec.conceal(units as usize * 480, &mut self.tmp).is_ok()
    }

    fn finish(&mut self) {
        // Push the filter tail through so the last milliseconds are not lost.
        let zeros = vec![0.0_f32; 128];
        self.rs_out.clear();
        self.rs.process(&zeros, &mut self.rs_out);
        self.fifo.extend_from_slice(&self.rs_out);
        self.rs.reset();
        self.state = State::Idle;
        self.stats.active = false;
    }

    /// Produce `frames` interleaved stereo frames at the output rate into
    /// `out` (missing audio is silence).
    pub(crate) fn pull(&mut self, frames: usize, dt: f64, out: &mut Vec<f32>) {
        if self.state == State::Priming {
            self.prime_wait_s += dt;
        }
        if self.state == State::Idle && self.fifo_frames() == 0 {
            self.idle_s += dt;
        }
        if self.state == State::Playing {
            self.steer(dt);
        }
        while self.fifo_frames() < frames && self.produce() {}
        self.stats.active = self.state == State::Playing || self.fifo_frames() > 0;
        out.clear();
        let have = self.fifo_frames().min(frames);
        out.extend_from_slice(&self.fifo[self.fifo_pos..self.fifo_pos + have * 2]);
        out.resize(frames * 2, 0.0);
        self.fifo_pos += have * 2;
        if self.fifo_pos > 8192 {
            let _ = self.fifo.drain(..self.fifo_pos);
            self.fifo_pos = 0;
        }
    }
}
