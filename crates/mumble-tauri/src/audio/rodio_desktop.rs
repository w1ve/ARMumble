//! Rodio-based audio capture and mixing playback implementations.
//!
//! Alternative desktop audio backend using rodio (which wraps cpal
//! internally but provides a higher-level, push-based API).  This
//! avoids the low-level callback complexity of raw cpal and lets
//! rodio handle device threading, sample-rate conversion, and mixing.

use std::collections::VecDeque;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread;

const MONO_CHANNELS: NonZero<u16> = NonZero::new(1).unwrap();
/// Playback is stereo so dual-receiver radio audio keeps its channels
/// separate. The inbound mixer decodes to interleaved L R; a stereo decoder
/// upmixes mono senders, so mono traffic plays identically on both channels.
const STEREO_CHANNELS: NonZero<u16> = NonZero::new(2).unwrap();
/// Output channel count the mixer source produces (interleaved).
const PLAYBACK_CHANNELS: usize = 2;
const SAMPLE_RATE_48K: NonZero<u32> = NonZero::new(48_000).unwrap();

use mumble_protocol::audio::capture::AudioCapture;
use mumble_protocol::audio::mixer::{SpeakerBuffers, SpeakerVolumes, StereoControl};
use mumble_protocol::audio::resample::LinearResampler;
use mumble_protocol::audio::sample::{AudioFormat, AudioFrame};
use mumble_protocol::error::{Error, Result};
use rodio::microphone::{available_inputs, MicrophoneBuilder};
use rodio::source::Source;
use tracing::{debug, trace, warn};

// -- Capture (Microphone) -------------------------------------------

/// Captures microphone input via rodio's [`Microphone`] source using a
/// background thread so that [`read_frame`](AudioCapture::read_frame)
/// is non-blocking.
///
/// Rodio's `Microphone::next()` blocks until hardware delivers a
/// sample, which is incompatible with the outbound pipeline's drain
/// loops (they expect `NotEnoughSamples` when no data is ready).
/// A dedicated thread reads from the Microphone and sends mono `f32`
/// samples through a bounded channel in [`CAPTURE_CHUNK`]-sized
/// batches (one channel operation per 10 ms instead of one per
/// sample).  `read_frame()` drains the channel with `try_recv()` and
/// returns `NotEnoughSamples` when fewer than `frame_size` samples
/// are available.
pub struct RodioCapture {
    format: AudioFormat,
    frame_size: usize,
    sequence: u64,
    sample_rx: Option<mpsc::Receiver<Vec<f32>>>,
    _capture_thread: Option<thread::JoinHandle<()>>,
    _radio2_thread: Option<thread::JoinHandle<()>>,
    /// Set to stop a synthesised test-tone 2nd-radio thread (it has no device
    /// to signal EOF). `None` when the 2nd radio is a real device or absent.
    _radio2_stop: Option<Arc<AtomicBool>>,
    volume: Arc<AtomicU32>,
    pending: Vec<f32>,
    device_name: Option<String>,
    /// Second radio input device (Station dual-radio). When set, capture opens
    /// a 2nd device whose mono audio can feed the RIGHT channel.
    device_name_2: Option<String>,
    /// Live right-channel source: 0 = Radio 1 (normal stereo), 1 = Radio 2
    /// (Left = radio1 mono, Right = radio2 mono). Read in the capture mix loop
    /// so the operator can switch it without a pipeline restart.
    right_source: Arc<AtomicU8>,
    /// When true, capture produces interleaved STEREO frames (L R L R) — the
    /// radio Station case (dual receivers on L/R). Mono otherwise.
    stereo: bool,
}

/// Right-channel source values shared with the capture thread via an atomic.
pub const RIGHT_SOURCE_RADIO1: u8 = 0;
pub const RIGHT_SOURCE_RADIO2: u8 = 1;

impl RodioCapture {
    pub fn new(
        device_name: Option<&str>,
        frame_size: usize,
        volume: Arc<AtomicU32>,
    ) -> Result<Self> {
        Self::new_with_channels(device_name, frame_size, volume, false)
    }

    pub fn new_with_channels(
        device_name: Option<&str>,
        frame_size: usize,
        volume: Arc<AtomicU32>,
        stereo: bool,
    ) -> Result<Self> {
        // The format is DETERMINISTIC from the request: a Station always
        // produces interleaved stereo frames. If the hardware opens mono, the
        // capture thread duplicates the single channel into L and R (dual-mono),
        // so the frame is still valid stereo and matches the stereo encoder.
        // This avoids a second device open (a probe) that was leaving the device
        // busy and killing capture entirely.
        Ok(Self {
            format: if stereo {
                AudioFormat::STEREO_48KHZ_F32
            } else {
                AudioFormat::MONO_48KHZ_F32
            },
            frame_size,
            sequence: 0,
            sample_rx: None,
            _capture_thread: None,
            _radio2_thread: None,
            _radio2_stop: None,
            volume,
            pending: Vec::with_capacity(frame_size * 2),
            device_name: device_name.map(str::to_owned),
            device_name_2: None,
            right_source: Arc::new(AtomicU8::new(RIGHT_SOURCE_RADIO1)),
            stereo,
        })
    }

    /// Dual-radio Station capture: always interleaved stereo. `device_name` is
    /// radio 1, `device_name_2` is radio 2. `right_source` is a live-switchable
    /// atomic (RIGHT_SOURCE_RADIO1/RADIO2) controlling what the right channel
    /// carries without a pipeline restart.
    pub fn new_dual(
        device_name: Option<&str>,
        device_name_2: Option<&str>,
        frame_size: usize,
        volume: Arc<AtomicU32>,
        right_source: Arc<AtomicU8>,
    ) -> Result<Self> {
        let mut c = Self::new_with_channels(device_name, frame_size, volume, true)?;
        c.device_name_2 = device_name_2.map(str::to_owned);
        c.right_source = right_source;
        Ok(c)
    }

    /// Look up an input device by display name (as returned by
    /// `get_audio_devices`).  Returns `None` if no matching device is
    /// found, in which case the caller should fall back to the
    /// default device.
    fn find_input_by_name(name: &str) -> Option<rodio::microphone::Input> {
        let inputs = available_inputs().ok()?;
        inputs.into_iter().find(|i| i.to_string() == name)
    }
}

/// Enumerate input device display names via rodio's `available_inputs()`.
///
/// These names are produced by the SAME source that `find_input_by_name`
/// matches against (`Input::to_string()`), so a name returned here is
/// guaranteed to resolve back to the correct device when the user selects
/// it — unlike cpal `description().name()`, which can diverge from rodio's
/// naming and cause a silent fall-back to the default device.
///
/// Returns `(name, is_default)` pairs. The default is reported by comparing
/// against cpal's default input device description, best-effort.
pub fn enumerate_input_names() -> Vec<(String, bool)> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let default_name = cpal::default_host()
        .default_input_device()
        .and_then(|d| d.description().ok().map(|desc| desc.name().to_string()))
        .or_else(|| {
            #[allow(deprecated)]
            cpal::default_host()
                .default_input_device()
                .and_then(|d| d.name().ok())
        });

    let Ok(inputs) = available_inputs() else {
        return Vec::new();
    };
    inputs
        .into_iter()
        .map(|i| {
            let name = i.to_string();
            // rodio's Input display name and cpal's default name may differ
            // in formatting; treat a case-insensitive substring match either
            // way as "this is the default".
            let is_default = default_name
                .as_deref()
                .map(|d| {
                    let (a, b) = (d.to_lowercase(), name.to_lowercase());
                    a == b || a.contains(&b) || b.contains(&a)
                })
                .unwrap_or(false);
            (name, is_default)
        })
        .collect()
}

impl AudioCapture for RodioCapture {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn read_frame(&mut self) -> Result<AudioFrame> {
        let rx = self
            .sample_rx
            .as_ref()
            .ok_or_else(|| Error::InvalidState("Microphone not started".into()))?;

        loop {
            match rx.try_recv() {
                Ok(chunk) => self.pending.extend_from_slice(&chunk),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err(Error::InvalidState("Microphone stream ended".into()));
                }
            }
        }

        // One frame is frame_size samples PER CHANNEL; stereo interleaves two.
        let frame_total = self.frame_size * if self.stereo { 2 } else { 1 };
        if self.pending.len() < frame_total {
            return Err(Error::NotEnoughSamples);
        }

        let vol = f32::from_bits(self.volume.load(Ordering::Relaxed));
        let mut data = Vec::with_capacity(frame_total * 4);
        // Collect the volume-adjusted samples for the input oscilloscope tap
        // (cheap no-op when no scope is subscribed).
        let mut scope_samples: Vec<f32> = Vec::with_capacity(frame_total);
        for s in self.pending.drain(..frame_total) {
            let adjusted = s * vol;
            scope_samples.push(adjusted);
            data.extend_from_slice(&adjusted.to_ne_bytes());
        }
        // Feed the transmit (input) oscilloscope.
        crate::audio::scope::push(
            crate::audio::scope::ScopeSource::Input,
            &scope_samples,
            if self.stereo { 2 } else { 1 },
        );
        self.sequence += 1;
        Ok(AudioFrame {
            data,
            format: self.format,
            sequence: self.sequence,
            is_silent: false,
        })
    }

    fn start(&mut self) -> Result<()> {
        let device_builder = MicrophoneBuilder::new();
        let with_device = if let Some(name) = self.device_name.as_deref() {
            match Self::find_input_by_name(name) {
                Some(input) => {
                    debug!("rodio capture: using selected input device '{name}'");
                    device_builder
                        .device(input)
                        .map_err(|e| Error::InvalidState(format!("Open input '{name}': {e}")))?
                }
                None => {
                    warn!(
                        "rodio capture: selected input device '{name}' not found, falling back to default"
                    );
                    device_builder
                        .default_device()
                        .map_err(|e| Error::InvalidState(format!("No input device: {e}")))?
                }
            }
        } else {
            device_builder
                .default_device()
                .map_err(|e| Error::InvalidState(format!("No input device: {e}")))?
        };
        let want_channels = if self.stereo { STEREO_CHANNELS } else { MONO_CHANNELS };
        let builder = with_device
            .default_config()
            .map_err(|e| Error::InvalidState(format!("Input config: {e}")))?
            .prefer_channel_counts([want_channels])
            .prefer_sample_rates([SAMPLE_RATE_48K]);

        let mic = builder
            .open_stream()
            .map_err(|e| Error::InvalidState(format!("Open microphone: {e}")))?;

        let config = mic.config();
        let device_rate = config.sample_rate.get();
        debug!(
            "rodio microphone opened: rate={}, channels={}",
            device_rate,
            config.channel_count.get(),
        );
        if device_rate != SAMPLE_RATE_48K.get() {
            // `prefer_sample_rates` is only a preference; a device that does not
            // offer 48 kHz (SignaLink and many USB dongles are 44.1 kHz) opens
            // at its own rate. Resample on capture so the frames we label 48 kHz
            // really are 48 kHz — otherwise the far end hears the radio sped up.
            debug!(
                "rodio capture: device rate {device_rate} Hz != 48 kHz; resampling on capture"
            );
        }

        let mic_channels = config.channel_count.get() as usize;
        // The output format is fixed at construction (self.stereo). If a Station
        // asked for stereo but the device opened mono, the capture thread
        // duplicates the single channel into L and R (dual-mono) so the emitted
        // frame is still valid interleaved stereo and matches the stereo encoder.
        let emit_stereo = self.stereo;
        debug!(
            "rodio capture: hw_channels={mic_channels}, emit_stereo={emit_stereo}"
        );

        // ~1 s of audio in CAPTURE_CHUNK-sized batches.
        let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(48_000 / CAPTURE_CHUNK);

        // Dual-radio: open the 2nd radio and spawn a thread that resamples its
        // mono to 48 kHz into a shared ring buffer. The primary stereo thread
        // reads the latest radio-2 samples when right_source == Radio2.
        let radio2: Option<Radio2Buffer> = if emit_stereo {
            self.device_name_2.as_deref().and_then(|name2| {
                // Test-tone pseudo-device: no hardware is opened; a thread
                // synthesises a 48 kHz sine straight into the ring so the
                // operator can exercise the dual-radio right channel with no
                // second radio present.
                if crate::commands::audio::is_test_tone(Some(name2)) {
                    let buf = Radio2Buffer::new();
                    let buf_feed = buf.clone();
                    let stop = Arc::new(AtomicBool::new(false));
                    self._radio2_stop = Some(stop.clone());
                    return match thread::Builder::new()
                        .name("rodio-radio2-tone".into())
                        .spawn(move || tone_capture_thread(buf_feed, stop))
                    {
                        Ok(h) => {
                            self._radio2_thread = Some(h);
                            debug!("rodio capture: 2nd radio = test tone");
                            Some(buf)
                        }
                        Err(e) => {
                            warn!("rodio capture: failed to spawn tone thread: {e}");
                            None
                        }
                    };
                }
                match open_radio2(name2) {
                    Ok((mic2, ch2, rate2)) => {
                        let buf = Radio2Buffer::new();
                        let buf_feed = buf.clone();
                        match thread::Builder::new()
                            .name("rodio-radio2-reader".into())
                            .spawn(move || radio2_capture_thread(mic2, ch2, rate2, buf_feed))
                        {
                            Ok(h) => {
                                self._radio2_thread = Some(h);
                                debug!("rodio capture: 2nd radio '{name2}' opened (rate={rate2}, ch={ch2})");
                                Some(buf)
                            }
                            Err(e) => {
                                warn!("rodio capture: failed to spawn radio2 thread: {e}");
                                None
                            }
                        }
                    }
                    Err(e) => {
                        warn!("rodio capture: failed to open 2nd radio '{name2}': {e}");
                        None
                    }
                }
            })
        } else {
            None
        };

        let right_source = self.right_source.clone();
        let handle = thread::Builder::new()
            .name("rodio-mic-reader".into())
            .spawn(move || {
                capture_thread(mic, mic_channels, device_rate, emit_stereo, radio2, right_source, tx)
            })
            .map_err(|e| Error::InvalidState(format!("Spawn mic thread: {e}")))?;

        self.sample_rx = Some(rx);
        self._capture_thread = Some(handle);
        self.pending.clear();
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.sample_rx = None;
        self._capture_thread = None;
        // A test-tone thread has no device to deliver EOF, so signal it to
        // exit before dropping the handle (a real radio2 thread ends when its
        // Microphone is dropped by the capture thread's end).
        if let Some(stop) = self._radio2_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self._radio2_thread = None;
        self.pending.clear();
        Ok(())
    }
}

/// Shared ring buffer carrying resampled 48 kHz mono samples from the 2nd
/// radio to the primary stereo capture thread. Bounded so a stalled consumer
/// can't grow it without limit; the primary thread drains it in lock-step.
#[derive(Clone)]
struct Radio2Buffer {
    inner: Arc<std::sync::Mutex<VecDeque<f32>>>,
}

impl Radio2Buffer {
    fn new() -> Self {
        Self { inner: Arc::new(std::sync::Mutex::new(VecDeque::with_capacity(48_000))) }
    }
    /// Push resampled mono samples (producer = radio2 thread).
    fn push(&self, samples: &[f32]) {
        if let Ok(mut q) = self.inner.lock() {
            // Cap at ~0.5 s to bound latency/memory if the consumer stalls.
            const MAX: usize = 24_000;
            for &s in samples {
                if q.len() >= MAX {
                    let _ = q.pop_front();
                }
                q.push_back(s);
            }
        }
    }
    /// Pop one sample (consumer = primary thread); 0.0 when starved.
    fn pop_one(&self) -> f32 {
        self.inner.lock().ok().and_then(|mut q| q.pop_front()).unwrap_or(0.0)
    }
}

/// Open the 2nd radio input device for dual-radio capture. Returns the
/// Microphone, its hardware channel count, and its native sample rate.
fn open_radio2(name: &str) -> Result<(rodio::microphone::Microphone, usize, u32)> {
    let builder = MicrophoneBuilder::new();
    let with_device = match RodioCapture::find_input_by_name(name) {
        Some(input) => builder
            .device(input)
            .map_err(|e| Error::InvalidState(format!("Open radio2 '{name}': {e}")))?,
        None => builder
            .default_device()
            .map_err(|e| Error::InvalidState(format!("No radio2 device: {e}")))?,
    };
    let mic = with_device
        .default_config()
        .map_err(|e| Error::InvalidState(format!("radio2 config: {e}")))?
        .prefer_sample_rates([SAMPLE_RATE_48K])
        .open_stream()
        .map_err(|e| Error::InvalidState(format!("Open radio2 mic: {e}")))?;
    let cfg = mic.config();
    let channels = cfg.channel_count.get() as usize;
    let rate = cfg.sample_rate.get();
    Ok((mic, channels, rate))
}

/// Radio-2 capture thread: downmix all channels to mono, resample to 48 kHz,
/// push into the shared ring for the primary stereo thread to consume.
fn radio2_capture_thread(
    mut mic: rodio::microphone::Microphone,
    channels: usize,
    device_rate: u32,
    buf: Radio2Buffer,
) {
    let mut resampler = LinearResampler::new(device_rate, SAMPLE_RATE_48K.get());
    let mut mono_in: Vec<f32> = Vec::with_capacity(CAPTURE_CHUNK);
    let mut out_48k: Vec<f32> = Vec::with_capacity(CAPTURE_CHUNK * 2);
    loop {
        let mut sum = 0.0f32;
        for _ in 0..channels {
            match mic.next() {
                Some(s) => sum += s,
                None => return,
            }
        }
        mono_in.push(sum / channels as f32);
        if mono_in.len() >= CAPTURE_CHUNK {
            resampler.process(&mono_in, &mut out_48k);
            mono_in.clear();
            buf.push(&out_48k);
            out_48k.clear();
        }
    }
}

/// Synthetic 2nd-radio source: generate a sine "test tone" into the shared
/// ring in place of a real device, so an operator can verify the dual-radio
/// right channel with no second radio connected. Paced in real time (one
/// 10 ms chunk per 10 ms) so the ring stays matched to the ~48 kHz consumer
/// and never overflows. A slow 1 Hz gate (0.5 s on / 0.5 s off) makes the tone
/// an unmistakable beep rather than a fatiguing drone, and the on/off edges are
/// ramped to avoid clicks. Exits promptly when `stop` is set (it has no device
/// to signal EOF).
fn tone_capture_thread(buf: Radio2Buffer, stop: Arc<AtomicBool>) {
    use std::f32::consts::TAU;
    const RATE: f32 = 48_000.0;
    const FREQ: f32 = 587.33; // D5 — clear of typical left-channel speech
    const AMP: f32 = 0.25; // headroom; the operator still has volume control
    const GATE_SAMPLES: usize = 24_000; // 0.5 s on, 0.5 s off (1 Hz beep)
    const RAMP: usize = 240; // 5 ms click-free fade on each edge
    let mut phase: f32 = 0.0;
    let phase_inc = TAU * FREQ / RATE;
    let mut n: usize = 0; // sample index within the current gate period
    let mut chunk: Vec<f32> = Vec::with_capacity(CAPTURE_CHUNK);
    while !stop.load(Ordering::Relaxed) {
        chunk.clear();
        for _ in 0..CAPTURE_CHUNK {
            let pos = n % (GATE_SAMPLES * 2);
            // Gate envelope: on for the first GATE_SAMPLES, off after, with a
            // short linear ramp at both edges so there's no click.
            let env = if pos < GATE_SAMPLES {
                let into = pos;
                let out = GATE_SAMPLES - pos;
                (into.min(out).min(RAMP) as f32) / RAMP as f32
            } else {
                0.0
            };
            chunk.push(phase.sin() * AMP * env);
            phase += phase_inc;
            if phase >= TAU {
                phase -= TAU;
            }
            n += 1;
        }
        buf.push(&chunk);
        // Real-time pacing: 480 samples @ 48 kHz = 10 ms of audio.
        thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Mono samples per channel send from the capture thread (10 ms at
/// 48 kHz).  Batching keeps the per-sample cost to a `Vec` push;
/// the channel synchronization (lock + receiver wakeup) happens only
/// 100 times per second instead of 48 000.
const CAPTURE_CHUNK: usize = 480;

#[allow(clippy::too_many_arguments)]
fn capture_thread(
    mut mic: rodio::microphone::Microphone,
    channels: usize,
    device_rate: u32,
    emit_stereo: bool,
    radio2: Option<Radio2Buffer>,
    right_source: Arc<AtomicU8>,
    tx: SyncSender<Vec<f32>>,
) {
    if emit_stereo {
        capture_thread_stereo(&mut mic, channels, device_rate, radio2, &right_source, &tx);
    } else {
        capture_thread_mono(&mut mic, channels, device_rate, &tx);
    }
}

/// Mono capture: downmix all hardware channels to mono, resample to 48 kHz, and
/// dispatch in CAPTURE_CHUNK batches.
fn capture_thread_mono(
    mic: &mut rodio::microphone::Microphone,
    channels: usize,
    device_rate: u32,
    tx: &SyncSender<Vec<f32>>,
) {
    let mut resampler = LinearResampler::new(device_rate, SAMPLE_RATE_48K.get());
    let mut mono_in: Vec<f32> = Vec::with_capacity(CAPTURE_CHUNK);
    let mut out_48k: Vec<f32> = Vec::with_capacity(CAPTURE_CHUNK * 2);

    fn drain(
        resampler: &mut LinearResampler,
        mono_in: &mut Vec<f32>,
        out_48k: &mut Vec<f32>,
        tx: &SyncSender<Vec<f32>>,
    ) -> bool {
        resampler.process(mono_in, out_48k);
        mono_in.clear();
        while out_48k.len() >= CAPTURE_CHUNK {
            let batch: Vec<f32> = out_48k.drain(..CAPTURE_CHUNK).collect();
            if tx.send(batch).is_err() {
                return false;
            }
        }
        true
    }

    loop {
        let mut sum = 0.0f32;
        for _ in 0..channels {
            match mic.next() {
                Some(s) => sum += s,
                None => {
                    let _ = drain(&mut resampler, &mut mono_in, &mut out_48k, tx);
                    if !out_48k.is_empty() {
                        let _ = tx.send(std::mem::take(&mut out_48k));
                    }
                    return;
                }
            }
        }
        mono_in.push(sum / channels as f32);
        if mono_in.len() >= CAPTURE_CHUNK && !drain(&mut resampler, &mut mono_in, &mut out_48k, tx) {
            return;
        }
    }
}

/// Stereo capture: take hardware channels 0 and 1 as L and R, resample each
/// independently (so they never bleed into each other at non-48k rates), and
/// dispatch interleaved L R batches. The batch length is CAPTURE_CHUNK*2 so a
/// batch is always a whole number of stereo sample pairs.
fn capture_thread_stereo(
    mic: &mut rodio::microphone::Microphone,
    channels: usize,
    device_rate: u32,
    radio2: Option<Radio2Buffer>,
    right_source: &Arc<AtomicU8>,
    tx: &SyncSender<Vec<f32>>,
) {
    let mut rs_l = LinearResampler::new(device_rate, SAMPLE_RATE_48K.get());
    let mut rs_r = LinearResampler::new(device_rate, SAMPLE_RATE_48K.get());
    let mut lin: Vec<f32> = Vec::with_capacity(CAPTURE_CHUNK);
    let mut rin: Vec<f32> = Vec::with_capacity(CAPTURE_CHUNK);
    let mut out_l: Vec<f32> = Vec::with_capacity(CAPTURE_CHUNK * 2);
    let mut out_r: Vec<f32> = Vec::with_capacity(CAPTURE_CHUNK * 2);
    // Interleaved L R awaiting dispatch; drained in CAPTURE_CHUNK*2 batches.
    let mut inter: Vec<f32> = Vec::with_capacity(CAPTURE_CHUNK * 4);
    const STEREO_BATCH: usize = CAPTURE_CHUNK * 2;

    #[allow(clippy::too_many_arguments)]
    fn drain(
        rs_l: &mut LinearResampler,
        rs_r: &mut LinearResampler,
        lin: &mut Vec<f32>,
        rin: &mut Vec<f32>,
        out_l: &mut Vec<f32>,
        out_r: &mut Vec<f32>,
        inter: &mut Vec<f32>,
        radio2: Option<&Radio2Buffer>,
        right_source: &Arc<AtomicU8>,
        tx: &SyncSender<Vec<f32>>,
    ) -> bool {
        out_l.clear();
        out_r.clear();
        rs_l.process(lin, out_l);
        rs_r.process(rin, out_r);
        lin.clear();
        rin.clear();
        let n = out_l.len().min(out_r.len());

        // Dual-radio: when the operator has selected Radio 2 for the right
        // channel AND a 2nd radio is open, override the mix so that
        //   Left  = radio 1 mixed to mono  (out_l + out_r) / 2
        //   Right = radio 2 (already 48 kHz, pulled sample-aligned from the ring)
        // Reading the atomic here makes the switch live, with no restart.
        let dual = radio2.is_some()
            && right_source.load(Ordering::Relaxed) == RIGHT_SOURCE_RADIO2;

        for i in 0..n {
            if dual {
                let mono1 = (out_l[i] + out_r[i]) * 0.5;
                let r2 = radio2.map(|b| b.pop_one()).unwrap_or(0.0);
                inter.push(mono1);
                inter.push(r2);
            } else {
                inter.push(out_l[i]);
                inter.push(out_r[i]);
            }
        }
        while inter.len() >= STEREO_BATCH {
            let batch: Vec<f32> = inter.drain(..STEREO_BATCH).collect();
            if tx.send(batch).is_err() {
                return false;
            }
        }
        true
    }

    loop {
        // Read one hardware frame (one sample per channel). Use channels 0/1 as
        // L/R; ignore any extra channels. For a MONO device (channels==1),
        // duplicate the single sample into both L and R (dual-mono) so the
        // emitted stereo frame is always valid.
        let mut l = 0.0f32;
        let mut r = 0.0f32;
        let mut got_r = false;
        let mut ended = false;
        for c in 0..channels {
            match mic.next() {
                Some(s) => {
                    if c == 0 {
                        l = s;
                    } else if c == 1 {
                        r = s;
                        got_r = true;
                    }
                }
                None => {
                    ended = true;
                    break;
                }
            }
        }
        if !got_r {
            r = l; // mono device: dual-mono
        }
        if ended {
            let _ = drain(
                &mut rs_l, &mut rs_r, &mut lin, &mut rin, &mut out_l, &mut out_r, &mut inter,
                radio2.as_ref(), right_source, tx,
            );
            if !inter.is_empty() {
                let _ = tx.send(std::mem::take(&mut inter));
            }
            return;
        }
        lin.push(l);
        rin.push(r);
        if lin.len() >= CAPTURE_CHUNK
            && !drain(
                &mut rs_l, &mut rs_r, &mut lin, &mut rin, &mut out_l, &mut out_r, &mut inter,
                radio2.as_ref(), right_source, tx,
            )
        {
            return;
        }
    }
}

// -- Custom Source for Mumble audio mixing --------------------------

/// Number of audio frames to mix per refill (20 ms at 48 kHz). One frame is
/// `PLAYBACK_CHANNELS` interleaved samples, so the mixed buffer holds
/// `MIX_CHUNK_SIZE * PLAYBACK_CHANNELS` f32s.
const MIX_CHUNK_SIZE: usize = 960;
/// Interleaved sample count mixed per refill (frames * channels).
const MIX_CHUNK_SAMPLES: usize = MIX_CHUNK_SIZE * PLAYBACK_CHANNELS;
/// Minimum buffered samples before playback begins (~100 ms). Scaled by the
/// playback channel count because the speaker buffers are interleaved.
const PRE_BUFFER_SAMPLES: usize = 4800 * PLAYBACK_CHANNELS;
/// Refill back-off (in mono samples) when a refill returns no data.
/// 5 ms keeps the speaker buffer mutex contention bounded (max
/// ~200 lock attempts/s per source) while letting a transient jitter
/// recover within a few ms instead of forcing a full 20 ms of decay.
const UNDERRUN_BACKOFF_SAMPLES: usize = 240;
/// Consecutive empty refills before re-priming the buffer.  Each
/// empty refill represents one [`UNDERRUN_BACKOFF_SAMPLES`] period
/// (5 ms), so 300 corresponds to ~1.5 s of sustained silence.
const REPRIME_AFTER: u32 = 300;

/// A rodio [`Source`] that reads from per-speaker ring buffers and
/// yields mixed mono `f32` samples at 48 kHz.
///
/// rodio's background output thread calls `Iterator::next()` to pull
/// samples. Mixing is done in chunks of [`MIX_CHUNK_SIZE`] to avoid
/// locking the speaker buffers on every single sample.
struct MumbleMixerSource {
    buffers: SpeakerBuffers,
    speaker_volumes: SpeakerVolumes,
    volume: Arc<AtomicU32>,
    stereo: Arc<StereoControl>,
    mixed_chunk: Vec<f32>,
    chunk_pos: usize,
    chunk_valid: usize,
    primed: bool,
    consecutive_empty: u32,
    running: Arc<AtomicBool>,
    last_sample: f32,
    in_underrun: bool,
    ramp_pos: usize,
    underrun_samples: usize,
    /// Amplitude captured at the moment underrun begins.  Used as the
    /// starting point of the cosine fade-out toward silence.  Without
    /// a fade the abrupt step from `last_sample` to 0 (or holding
    /// `last_sample` at a non-zero DC value before decay) creates a
    /// broadband click - especially audible at the end of a short
    /// utterance like a single word.
    fade_anchor: f32,
    /// Remaining samples to skip before the next `refill_chunk()` call.
    /// Prevents per-sample refill attempts during underrun by throttling
    /// them to one every [`UNDERRUN_BACKOFF_SAMPLES`] (5 ms).  Short
    /// enough that transient jitter recovers within a few ms instead
    /// of forcing a full chunk of decay/silence.
    underrun_cooldown: usize,
    diag: MixerDiag,
}

/// Periodic diagnostics for the rodio mixer source.
struct MixerDiag {
    samples_pulled: u64,
    refills: u64,
    underrun_refills: u64,
    partial_refills: u64,
    ramps_applied: u64,
    peak: f32,
    max_buf_depth: usize,
    lock_failures: u64,
    reprime_count: u64,
}

impl MumbleMixerSource {
    fn new(
        buffers: SpeakerBuffers,
        speaker_volumes: SpeakerVolumes,
        volume: Arc<AtomicU32>,
        stereo: Arc<StereoControl>,
        running: Arc<AtomicBool>,
    ) -> Self {
        Self {
            buffers,
            speaker_volumes,
            volume,
            stereo,
            mixed_chunk: vec![0.0; MIX_CHUNK_SAMPLES],
            chunk_pos: 0,
            chunk_valid: 0,
            primed: false,
            consecutive_empty: 0,
            running,
            last_sample: 0.0,
            in_underrun: false,
            ramp_pos: 0,
            underrun_samples: 0,
            fade_anchor: 0.0,
            underrun_cooldown: 0,
            diag: MixerDiag {
                samples_pulled: 0,
                refills: 0,
                underrun_refills: 0,
                partial_refills: 0,
                ramps_applied: 0,
                peak: 0.0,
                max_buf_depth: 0,
                lock_failures: 0,
                reprime_count: 0,
            },
        }
    }

    fn refill_chunk(&mut self) {
        let Ok(mut bufs) = self.buffers.lock() else {
            self.diag.lock_failures += 1;
            self.chunk_pos = 0;
            self.chunk_valid = 0;
            self.underrun_cooldown = UNDERRUN_BACKOFF_SAMPLES;
            return;
        };

        if !self.primed {
            let max_available = bufs.values().map(VecDeque::len).max().unwrap_or(0);
            if max_available < PRE_BUFFER_SAMPLES {
                self.chunk_pos = 0;
                self.chunk_valid = 0;
                self.underrun_cooldown = UNDERRUN_BACKOFF_SAMPLES;
                return;
            }
            self.primed = true;
        }

        // try_lock avoids blocking the rodio output thread; on
        // contention we fall back to default volumes (1.0).  Borrow
        // the guard directly instead of cloning the HashMap - this
        // runs ~50-200 times per second and a clone would allocate
        // on every refill.
        let sv_guard = self.speaker_volumes.try_lock().ok();
        let empty = std::collections::HashMap::new();
        let sv = sv_guard.as_deref().unwrap_or(&empty);

        let (drained, valid_count, buf_depth) =
            super::desktop::batch_drain_speakers(&mut bufs, sv, &mut self.mixed_chunk, MIX_CHUNK_SAMPLES);

        self.diag.refills += 1;
        self.diag.max_buf_depth = self.diag.max_buf_depth.max(buf_depth);

        if !drained || valid_count == 0 {
            self.consecutive_empty += 1;
            self.diag.underrun_refills += 1;
            if self.consecutive_empty >= REPRIME_AFTER {
                self.primed = false;
                self.diag.reprime_count += 1;
            }
            self.chunk_pos = 0;
            self.chunk_valid = 0;
            self.underrun_cooldown = UNDERRUN_BACKOFF_SAMPLES;
        } else {
            if valid_count < MIX_CHUNK_SAMPLES {
                self.diag.partial_refills += 1;
            }
            // Apply the operator's stereo-fold / per-channel gain to the
            // interleaved mix before it is consumed sample-by-sample below.
            // No-op fast path when Stereo + unity gains, so the common case
            // is free.
            self.stereo.apply(&mut self.mixed_chunk[..valid_count]);
            self.consecutive_empty = 0;
            self.chunk_pos = 0;
            self.chunk_valid = valid_count;
            // Feed the output oscilloscope with the final interleaved mix
            // (cheap no-op when no output scope is subscribed).
            crate::audio::scope::push(
                crate::audio::scope::ScopeSource::Output,
                &self.mixed_chunk[..valid_count],
                PLAYBACK_CHANNELS,
            );
        }
    }
}

impl Iterator for MumbleMixerSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if !self.running.load(Ordering::Relaxed) {
            return None;
        }

        if self.underrun_cooldown > 0 {
            self.underrun_cooldown -= 1;
        } else if self.chunk_pos >= self.chunk_valid {
            self.refill_chunk();
        }

        let vol = f32::from_bits(self.volume.load(Ordering::Relaxed));

        self.diag.samples_pulled += 1;
        // Log diagnostics every ~1 second (48000 samples at 48 kHz).
        if self.diag.samples_pulled.is_multiple_of(48_000) {
            trace!(
                "rodio mixer diag: pulled={}, refills={}, underrun={}, partial={}, \
                 ramps={}, peak={:.4}, max_buf={}, lock_fail={}, reprimes={}, \
                 consec_empty={}, in_underrun={}",
                self.diag.samples_pulled,
                self.diag.refills,
                self.diag.underrun_refills,
                self.diag.partial_refills,
                self.diag.ramps_applied,
                self.diag.peak,
                self.diag.max_buf_depth,
                self.diag.lock_failures,
                self.diag.reprime_count,
                self.consecutive_empty,
                self.in_underrun,
            );
        }

        if self.chunk_pos < self.chunk_valid {
            let raw = self.mixed_chunk[self.chunk_pos];
            self.chunk_pos += 1;

            let sample = if self.in_underrun {
                self.diag.ramps_applied += 1;
                // Scale the fade-in length with how deep the silence was:
                // a brief jitter recovers in ~1 ms, but coming out of
                // a multi-second silence (e.g. someone starts talking
                // for the first time) requires a much longer fade or
                // the abrupt onset is heard as a click/pop at the
                // ramp's fundamental frequency.
                const MIN_RAMP: usize = 48;   // ~1 ms - jitter recovery
                const MAX_RAMP: usize = 480;  // ~10 ms - speech onset
                let ramp_len = self.underrun_samples.clamp(MIN_RAMP, MAX_RAMP);
                self.ramp_pos += 1;
                if self.ramp_pos >= ramp_len {
                    self.in_underrun = false;
                    self.ramp_pos = 0;
                    self.underrun_samples = 0;
                    raw
                } else {
                    // Equal-power cosine fade: smoother perceptually
                    // than linear, no derivative discontinuity at the
                    // endpoints, and avoids the audible "ramp tone"
                    // that a short linear fade can produce.
                    let t = self.ramp_pos as f32 / ramp_len as f32;
                    let w = 0.5 - 0.5 * (std::f32::consts::PI * t).cos();
                    self.last_sample * (1.0 - w) + raw * w
                }
            } else {
                raw
            };

            self.diag.peak = self.diag.peak.max(sample.abs());
            self.last_sample = sample;
            Some(super::soft_clip(sample * vol))
        } else {
            // Underrun strategy: cosine fade-out from the amplitude
            // we held at the moment underrun began (`fade_anchor`)
            // toward zero over `FADE_OUT_LEN` samples, then silence.
            //
            // Why a cosine fade instead of DC-hold-then-decay:
            // holding a non-zero DC value for a few ms (e.g. when a
            // speaker stops mid-word) is itself a step in the
            // waveform, which is broadband and audible as a click.
            // The cosine fade has zero derivative at both endpoints
            // and decays smoothly from `fade_anchor` to 0, so the
            // end of an utterance is inaudible and brief mid-word
            // jitter only causes a small, smooth amplitude dip
            // (~15 % at 5 ms, fully recovers when audio resumes).
            const FADE_OUT_LEN: usize = 960; // 20 ms at 48 kHz

            // Capture the anchor on the first underrun sample only;
            // this preserves the smooth fade across the entire gap
            // even when individual `next()` calls are interleaved
            // with cooldown decrements.
            if !self.in_underrun {
                self.fade_anchor = self.last_sample;
            }
            self.in_underrun = true;
            self.ramp_pos = 0;
            self.underrun_samples += 1;

            if self.underrun_samples >= FADE_OUT_LEN {
                self.last_sample = 0.0;
            } else {
                let t = self.underrun_samples as f32 / FADE_OUT_LEN as f32;
                let w = 0.5 + 0.5 * (std::f32::consts::PI * t).cos();
                self.last_sample = self.fade_anchor * w;
            }
            Some(super::soft_clip(self.last_sample * vol))
        }
    }
}

impl Source for MumbleMixerSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> NonZero<u16> {
        STEREO_CHANNELS
    }

    fn sample_rate(&self) -> NonZero<u32> {
        SAMPLE_RATE_48K
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

// -- Mixing Playback ------------------------------------------------

/// Rodio-based mixing playback device.
///
/// Opens the default output via rodio's `Mixer` and feeds a
/// [`MumbleMixerSource`] that reads from the shared per-speaker
/// buffers. Rodio handles the output thread, sample-rate conversion
/// (if the device is not 48 kHz), and buffer management.
pub struct RodioMixingPlayback {
    /// Keeps the device sink alive; dropping it stops playback.
    _device_sink: Option<rodio::stream::MixerDeviceSink>,
    running: Arc<AtomicBool>,
    buffers: SpeakerBuffers,
    speaker_volumes: SpeakerVolumes,
    volume: Arc<AtomicU32>,
    stereo: Arc<StereoControl>,
    device_name: Option<String>,
}

impl RodioMixingPlayback {
    pub fn new(
        device_name: Option<&str>,
        volume: Arc<AtomicU32>,
        buffers: SpeakerBuffers,
        speaker_volumes: SpeakerVolumes,
        stereo: Arc<StereoControl>,
    ) -> Result<Self> {
        Ok(Self {
            _device_sink: None,
            running: Arc::new(AtomicBool::new(false)),
            buffers,
            speaker_volumes,
            volume,
            stereo,
            device_name: device_name.map(str::to_owned),
        })
    }

    /// Look up a cpal output device by description name (matching the
    /// names returned by `get_output_devices`).
    fn find_output_by_name(name: &str) -> Option<cpal::Device> {
        use cpal::traits::{DeviceTrait, HostTrait};
        let host = cpal::default_host();
        host.output_devices().ok()?.find(|d| {
            d.description()
                .ok()
                .map(|desc| desc.name().to_string())
                .as_deref()
                == Some(name)
        })
    }
}

impl super::MixingPlayback for RodioMixingPlayback {
    fn start(&mut self) -> Result<()> {
        let builder = if let Some(name) = self.device_name.as_deref() {
            match Self::find_output_by_name(name) {
                Some(device) => {
                    debug!("rodio playback: using selected output device '{name}'");
                    rodio::stream::DeviceSinkBuilder::from_device(device).map_err(|e| {
                        Error::InvalidState(format!("Open output '{name}': {e}"))
                    })?
                }
                None => {
                    warn!(
                        "rodio playback: selected output device '{name}' not found, falling back to default"
                    );
                    rodio::stream::DeviceSinkBuilder::from_default_device()
                        .map_err(|e| Error::InvalidState(format!("Open output device: {e}")))?
                }
            }
        } else {
            rodio::stream::DeviceSinkBuilder::from_default_device()
                .map_err(|e| Error::InvalidState(format!("Open output device: {e}")))?
        };
        // Pin the OS-sink to 48 kHz stereo (the rate/channels our mixer source
        // actually produces) instead of accepting the device default. rodio's
        // `from_device` otherwise adopts the device's default config — on Linux
        // (Pulse/PipeWire) that is frequently 44.1 kHz with a ~50 ms period, so
        // the Mixer resamples 48 kHz -> 44.1 kHz and the device period beats
        // against our fixed 20 ms mix chunk, periodically starving the source
        // and firing its underrun fade-out/fade-in — an audible ~2 Hz pulse.
        // WASAPI on Windows defaults to 48 kHz with a clean period, which is
        // why the same code is clean there. Forcing 48 kHz removes the extra
        // resample/period beat; a fixed 2048-frame buffer (~43 ms) is rodio's
        // recommended "general use / media playback" size for glitch-free out.
        let device_sink = builder
            .with_sample_rate(SAMPLE_RATE_48K)
            .with_channels(STEREO_CHANNELS)
            .with_buffer_size(cpal::BufferSize::Fixed(2048))
            .open_stream()
            .map_err(|e| Error::InvalidState(format!("Open output stream: {e}")))?;
        let mixer = device_sink.mixer().clone();

        let cfg = device_sink.config();
        debug!(
            "rodio playback opened: device_rate={} Hz, channels={}",
            cfg.sample_rate().get(),
            cfg.channel_count().get(),
        );

        self.running.store(true, Ordering::Relaxed);

        let source = MumbleMixerSource::new(
            self.buffers.clone(),
            self.speaker_volumes.clone(),
            self.volume.clone(),
            self.stereo.clone(),
            self.running.clone(),
        );
        mixer.add(source);

        self._device_sink = Some(device_sink);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.running.store(false, Ordering::Relaxed);
        if let Some(mut sink) = self._device_sink.take() {
            sink.log_on_drop(false);
        }
        if let Ok(mut bufs) = self.buffers.lock() {
            bufs.values_mut().for_each(VecDeque::clear);
        }
        Ok(())
    }
}

// -- Factory --------------------------------------------------------

/// Desktop audio device factory backed by rodio.
pub struct RodioAudioFactory;

impl super::AudioDeviceFactory for RodioAudioFactory {
    fn create_capture(
        device_name: Option<&str>,
        frame_size: usize,
        volume: Arc<AtomicU32>,
    ) -> std::result::Result<Box<dyn AudioCapture>, String> {
        RodioCapture::new(device_name, frame_size, volume)
            .map(|c| Box::new(c) as _)
            .map_err(|e| format!("Rodio capture init: {e}"))
    }

    fn create_capture_channels(
        device_name: Option<&str>,
        frame_size: usize,
        volume: Arc<AtomicU32>,
        stereo: bool,
    ) -> std::result::Result<Box<dyn AudioCapture>, String> {
        RodioCapture::new_with_channels(device_name, frame_size, volume, stereo)
            .map(|c| Box::new(c) as _)
            .map_err(|e| format!("Rodio capture init: {e}"))
    }

    fn create_mixing_playback(
        device_name: Option<&str>,
        volume: Arc<AtomicU32>,
        buffers: SpeakerBuffers,
        speaker_volumes: SpeakerVolumes,
        stereo: Arc<StereoControl>,
    ) -> std::result::Result<Box<dyn super::MixingPlayback>, String> {
        RodioMixingPlayback::new(device_name, volume, buffers, speaker_volumes, stereo)
            .map(|c| Box::new(c) as _)
            .map_err(|e| format!("Rodio mixing playback init: {e}"))
    }
}

/// Create a dual-radio Station capture (always stereo): radio 1 on
/// `device_name`, radio 2 on `device_name_2`, with a live-switchable
/// `right_source` atomic. Rodio-only (the active desktop backend).
pub fn create_capture_dual(
    device_name: Option<&str>,
    device_name_2: Option<&str>,
    frame_size: usize,
    volume: Arc<AtomicU32>,
    right_source: Arc<AtomicU8>,
) -> std::result::Result<Box<dyn AudioCapture>, String> {
    RodioCapture::new_dual(device_name, device_name_2, frame_size, volume, right_source)
        .map(|c| Box::new(c) as _)
        .map_err(|e| format!("Rodio dual capture init: {e}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "unwrap is acceptable in test code")]
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn make_source(buffers: SpeakerBuffers, speaker_volumes: SpeakerVolumes) -> MumbleMixerSource {
        let volume = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let running = Arc::new(AtomicBool::new(true));
        let stereo = Arc::new(StereoControl::default());
        MumbleMixerSource::new(buffers, speaker_volumes, volume, stereo, running)
    }

    #[test]
    fn underrun_decays_smoothly_instead_of_hard_silence() {
        let buffers: SpeakerBuffers = Arc::new(Mutex::new(HashMap::new()));
        let svols: SpeakerVolumes = Arc::new(Mutex::new(HashMap::new()));

        let total_samples = PRE_BUFFER_SAMPLES + MIX_CHUNK_SIZE;
        {
            let mut bufs = buffers.lock().unwrap();
            let buf = bufs.entry(1).or_default();
            for _ in 0..total_samples {
                buf.push_back(0.5);
            }
        }

        let mut src = make_source(buffers, svols);

        // Drain all real audio samples.
        for _ in 0..total_samples {
            let s = src.next().unwrap();
            assert!((s - 0.5).abs() < 0.01, "expected ~0.5, got {s}");
        }

        // First underrun sample: cosine fade-out at t=1/960 starts
        // essentially at the anchor amplitude (~0.5), NOT a hard 0.
        let first_underrun = src.next().unwrap();
        assert!(
            first_underrun.abs() > 0.4,
            "first underrun sample should still be near anchor 0.5, got {first_underrun}"
        );

        // After the full fade-out length (960 samples = 20 ms),
        // output should be silence.
        for _ in 0..960 {
            let _ = src.next();
        }
        let after_fade = src.next().unwrap();
        assert!(
            after_fade.abs() < 1e-3,
            "after fade-out should be silence, got {after_fade}"
        );
    }

    #[test]
    fn resume_after_underrun_ramps_smoothly() {
        let buffers: SpeakerBuffers = Arc::new(Mutex::new(HashMap::new()));
        let svols: SpeakerVolumes = Arc::new(Mutex::new(HashMap::new()));

        let total_samples = PRE_BUFFER_SAMPLES + MIX_CHUNK_SIZE;
        {
            let mut bufs = buffers.lock().unwrap();
            let buf = bufs.entry(1).or_default();
            for _ in 0..total_samples {
                buf.push_back(0.5);
            }
        }

        let mut src = make_source(buffers.clone(), svols);

        // Drain all real audio.
        for _ in 0..total_samples {
            let _ = src.next();
        }

        // Enter underrun for 20 samples (short underrun -> ramp_len = 20).
        for _ in 0..20 {
            let _ = src.next();
        }
        assert!(src.in_underrun, "should be in underrun state");

        // Refill speaker buffer with new audio at a different level.
        {
            let mut bufs = buffers.lock().unwrap();
            let buf = bufs.entry(1).or_default();
            for _ in 0..MIX_CHUNK_SIZE {
                buf.push_back(-0.3);
            }
        }

        // Drain remaining cooldown - the source continues decay output
        // until the next refill attempt at the chunk boundary.  Each
        // of these samples also bumps `underrun_samples`, deepening
        // the perceived underrun for the upcoming ramp.
        let remaining = src.underrun_cooldown;
        for _ in 0..remaining {
            let _ = src.next();
        }
        let expected_ramp = src.underrun_samples.clamp(48, 480);

        // First samples after resume should ramp from decayed last_sample
        // toward -0.3, NOT jump directly to -0.3.
        let resume_sample = src.next().unwrap();
        // The cosine fade at t=1/ramp_len starts essentially at last_sample,
        // so the first ramp output should be far closer to ~0.49 than to -0.3.
        assert!(
            resume_sample > -0.2,
            "resume should ramp, not jump to -0.3; got {resume_sample}"
        );

        // After the full ramp completes, samples should be ~-0.3.
        // Drain enough samples to be safely past the ramp end.
        for _ in 0..expected_ramp {
            let _ = src.next();
        }
        let settled = src.next().unwrap();
        assert!(
            (settled - (-0.3)).abs() < 0.05,
            "after ramp ({expected_ramp} samples) should settle to -0.3, got {settled}"
        );
    }

    #[test]
    fn brief_underrun_does_not_trigger_reprime() {
        let buffers: SpeakerBuffers = Arc::new(Mutex::new(HashMap::new()));
        let svols: SpeakerVolumes = Arc::new(Mutex::new(HashMap::new()));

        let total_samples = PRE_BUFFER_SAMPLES + MIX_CHUNK_SIZE;
        {
            let mut bufs = buffers.lock().unwrap();
            let buf = bufs.entry(1).or_default();
            for _ in 0..total_samples {
                buf.push_back(0.5);
            }
        }

        let mut src = make_source(buffers.clone(), svols);

        for _ in 0..total_samples {
            let _ = src.next();
        }
        assert!(src.primed, "should be primed after initial playback");

        // Drain one full chunk's worth of underrun (MIX_CHUNK_SIZE samples).
        // With the 5 ms back-off, this counts as
        // MIX_CHUNK_SIZE / UNDERRUN_BACKOFF_SAMPLES empty refills.
        for _ in 0..MIX_CHUNK_SIZE {
            let _ = src.next();
        }

        let expected_empty = (MIX_CHUNK_SIZE / UNDERRUN_BACKOFF_SAMPLES) as u32;
        assert!(src.primed, "should still be primed after a single-chunk underrun");
        assert_eq!(
            src.consecutive_empty, expected_empty,
            "one chunk of underrun should count as {expected_empty} empty refills, not {}", src.consecutive_empty
        );
        assert_eq!(src.diag.reprime_count, 0, "no repriming should have occurred");
        assert!(
            expected_empty < REPRIME_AFTER,
            "REPRIME_AFTER ({REPRIME_AFTER}) must be larger than a single-chunk underrun ({expected_empty})"
        );
    }

    #[test]
    fn speech_onset_after_long_silence_uses_long_fade_in() {
        // Regression: when audio starts after the source has been
        // silent for a long time (typical "user starts talking"), the
        // fade-in must be long enough that the onset is not perceived
        // as a click/pop.  A 1 ms ramp from 0.0 to ~0.5 produces an
        // audible transient at ~500 Hz; we want at least ~10 ms.
        let buffers: SpeakerBuffers = Arc::new(Mutex::new(HashMap::new()));
        let svols: SpeakerVolumes = Arc::new(Mutex::new(HashMap::new()));

        let mut src = make_source(buffers.clone(), svols);

        // Pull enough samples to be deeply in underrun (simulate
        // ~1 second of silence before any audio arrives).
        for _ in 0..48_000 {
            let _ = src.next();
        }
        assert!(src.in_underrun, "should be in underrun after long silence");
        assert!(
            src.underrun_samples > 1_000,
            "underrun_samples should accumulate during long silence, got {}",
            src.underrun_samples
        );

        // Now simulate a speaker beginning to talk: fill the buffer.
        {
            let mut bufs = buffers.lock().unwrap();
            let buf = bufs.entry(1).or_default();
            for _ in 0..(PRE_BUFFER_SAMPLES + MIX_CHUNK_SIZE) {
                buf.push_back(0.5);
            }
        }

        // Drain pending cooldown so the next call refills the chunk.
        let cooldown = src.underrun_cooldown;
        for _ in 0..cooldown {
            let _ = src.next();
        }

        // Confirm the ramp will use the long-silence path (>=480 samples).
        // After the cooldown drain underrun_samples is huge, so the
        // ramp_len clamps at MAX_RAMP=480.
        assert!(
            src.underrun_samples >= 480,
            "underrun must be deep enough to trigger MAX_RAMP, got {}",
            src.underrun_samples
        );

        // The first 48 samples (the OLD ramp_len) must NOT yet have
        // approached the target amplitude - that was the bug.  With a
        // 480-sample cosine fade, the first 48 samples (10% of the
        // ramp) keep the output well below half target.
        let mut peak_in_first_ms = 0.0_f32;
        for _ in 0..48 {
            let s = src.next().unwrap();
            peak_in_first_ms = peak_in_first_ms.max(s.abs());
        }
        assert!(
            peak_in_first_ms < 0.20,
            "first 1 ms of fade-in should stay well below target 0.5, got peak {peak_in_first_ms}"
        );

        // After enough samples to clearly clear the ramp, output must
        // have reached the target amplitude ~0.5.
        for _ in 0..MAX_RAMP_GUARD {
            let _ = src.next();
        }
        let settled = src.next().unwrap();
        assert!(
            (settled - 0.5).abs() < 0.02,
            "after fade-in completes, should reach target 0.5, got {settled}"
        );
        assert!(!src.in_underrun, "ramp should be complete by now");
    }

    /// A safety margin larger than `MAX_RAMP` (480 samples) used by
    /// fade-in tests to drain past the cosine fade.
    const MAX_RAMP_GUARD: usize = 600;

    #[test]
    fn end_of_utterance_fades_smoothly_no_step() {
        // Regression: when a speaker stops mid-word (single-word
        // utterance), the buffer suddenly empties at a non-zero
        // amplitude.  Holding that amplitude flat for any duration
        // creates a DC step in the waveform, audible as a click.
        // The cosine fade-out must produce a strictly monotonic,
        // step-free decay from the anchor amplitude to zero.
        let buffers: SpeakerBuffers = Arc::new(Mutex::new(HashMap::new()));
        let svols: SpeakerVolumes = Arc::new(Mutex::new(HashMap::new()));

        // Fill with a constant non-zero amplitude (e.g. peak of a
        // word's vowel formant).
        let total_samples = PRE_BUFFER_SAMPLES + MIX_CHUNK_SIZE;
        const ANCHOR: f32 = 0.7;
        {
            let mut bufs = buffers.lock().unwrap();
            let buf = bufs.entry(1).or_default();
            for _ in 0..total_samples {
                buf.push_back(ANCHOR);
            }
        }

        let mut src = make_source(buffers, svols);
        for _ in 0..total_samples {
            let _ = src.next();
        }

        // Collect the full fade-out window plus a margin.
        let mut samples = Vec::with_capacity(1100);
        for _ in 0..1100 {
            samples.push(src.next().unwrap());
        }

        // Step 1: the very first underrun sample must NOT be zero
        // (which would be a hard cliff) and must NOT exceed the
        // anchor (which would be amplification).
        assert!(
            samples[0] > 0.5 && samples[0] <= ANCHOR + 1e-3,
            "first fade-out sample should start near anchor, got {}", samples[0]
        );

        // Step 2: monotonically decreasing amplitude (no plateau,
        // no oscillation) for the full fade window.
        for i in 1..960 {
            assert!(
                samples[i] <= samples[i - 1] + 1e-4,
                "fade-out must be monotonic; samples[{}]={} > samples[{}]={}",
                i, samples[i], i - 1, samples[i - 1]
            );
        }

        // Step 3: after the fade-out length, output must be at zero
        // (true silence, not a residual DC offset).
        for &s in &samples[1000..] {
            assert!(
                s.abs() < 1e-4,
                "after fade-out must be exactly silence, got {s}"
            );
        }

        // Step 4: there must be no step larger than ~1.5 % of the
        // anchor between consecutive samples (smooth derivative).
        const MAX_STEP: f32 = 0.012;
        for i in 1..960 {
            let delta = (samples[i] - samples[i - 1]).abs();
            assert!(
                delta < MAX_STEP,
                "step between samples[{}] and samples[{}] = {} exceeds smoothness budget {}",
                i - 1, i, delta, MAX_STEP
            );
        }
    }

    #[test]
    fn brief_underrun_holds_amplitude_then_decays() {
        // Regression: a 1-2 packet network jitter spike in the middle
        // of a word must NOT cause a hard step in amplitude (which
        // sounds like a click/warble).  The cosine fade-out keeps
        // the first ~5 ms of underrun at >= 85 % of the anchor
        // amplitude, so brief jitter is barely audible.
        let buffers: SpeakerBuffers = Arc::new(Mutex::new(HashMap::new()));
        let svols: SpeakerVolumes = Arc::new(Mutex::new(HashMap::new()));

        let total_samples = PRE_BUFFER_SAMPLES + MIX_CHUNK_SIZE;
        {
            let mut bufs = buffers.lock().unwrap();
            let buf = bufs.entry(1).or_default();
            for _ in 0..total_samples {
                buf.push_back(0.5);
            }
        }

        let mut src = make_source(buffers.clone(), svols);
        for _ in 0..total_samples {
            let _ = src.next();
        }
        // Now in steady-state, last_sample = 0.5.

        // 5 ms (240 samples) of underrun: cosine fade at t=240/960=0.25
        // gives w = 0.5 + 0.5*cos(pi*0.25) ~= 0.854, so the sample at
        // 240 should be ~0.427.  Throughout the first 5 ms the
        // amplitude must stay above ~80 % of the anchor.
        for i in 0..240 {
            let s = src.next().unwrap();
            assert!(
                s >= 0.40,
                "sample {i} during early fade-out should stay >= 0.40, got {s}"
            );
        }

        // After the full 20 ms fade-out, output should be at silence.
        for _ in 0..2000 {
            let _ = src.next();
        }
        let after_fade = src.next().unwrap();
        assert!(
            after_fade.abs() < 1e-3,
            "after fade-out should be silence, got {after_fade}"
        );
    }
}
