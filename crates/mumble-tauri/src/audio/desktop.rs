//! Cpal-based audio capture and mixing playback implementations.
//!
//! Bridges the OS audio subsystem (via `cpal`) to the protocol
//! library's [`AudioCapture`] trait and the [`MixingPlayback`] trait
//! so that real hardware can be driven by the mixer infrastructure.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tracing::{debug, error, warn};

use mumble_protocol::audio::capture::AudioCapture;
use mumble_protocol::audio::resample::LinearResampler;
use mumble_protocol::audio::sample::{AudioFormat, AudioFrame};
use mumble_protocol::error::{Error, Result};

/// Channel count of the inbound mixer's interleaved speaker buffers. The
/// rodio backend plays these back in stereo; this cpal fallback folds them
/// to mono (summing L+R per frame), so stereo separation is only available
/// on the default rodio backend. Kept correct, not feature-complete.
const SRC_CHANNELS: usize = 2;

// -- Capture --------------------------------------------------------

/// Captures microphone input via cpal and makes it available as
/// [`AudioFrame`]s through the [`AudioCapture`] trait.
///
/// Internally a cpal input stream pushes samples into a lock-based
/// ring buffer. [`read_frame`](AudioCapture::read_frame) drains
/// exactly one frame's worth of samples (960 @ 48 kHz = 20 ms).
pub struct CpalCapture {
    format: AudioFormat,
    /// Samples per channel per frame (e.g. 960 for 20 ms @ 48 kHz).
    frame_size: usize,
    sequence: u64,
    buffer: Arc<Mutex<VecDeque<f32>>>,
    stream: Option<cpal::Stream>,
    device: cpal::Device,
    /// Number of channels the hardware actually uses.
    hw_channels: u16,
    /// When true, capture produces interleaved STEREO frames (L R L R) instead
    /// of downmixing to mono — used by the radio Station so a dual-receiver rig
    /// (different receivers on L and R) keeps its channel separation on the air.
    stereo: bool,
    /// Live input volume multiplier (`f32` bits in `AtomicU32`).
    volume: Arc<AtomicU32>,
    /// Suppresses repeated overflow warnings from the cpal callback.
    /// Set to `true` on first overflow, cleared when the consumer
    /// catches up in `read_frame`.
    overflow_warned: Arc<AtomicBool>,
}

// SAFETY: On Windows / WASAPI the underlying COM objects use the MTA
// model and are safe to send between threads.  The `!Send` marker in
// cpal is a conservative cross-platform guard that does not apply here.
#[allow(unsafe_code, reason = "WASAPI COM objects are MTA-safe; cpal's !Send is a conservative cross-platform guard")]
unsafe impl Send for CpalCapture {}

impl CpalCapture {
    /// Create a new capture source.
    ///
    /// * `device_name` - choose a specific device, or `None` for default.
    /// * `frame_size` - samples per channel per frame (480 for Mumble).
    /// * `volume` - shared atomic volume multiplier (f32 bits as u32).
    pub fn new(device_name: Option<&str>, frame_size: usize, volume: Arc<AtomicU32>) -> Result<Self> {
        Self::new_with_channels(device_name, frame_size, volume, false)
    }

    /// Create a capture source, optionally in stereo (interleaved L R) mode.
    pub fn new_with_channels(
        device_name: Option<&str>,
        frame_size: usize,
        volume: Arc<AtomicU32>,
        stereo: bool,
    ) -> Result<Self> {
        let host = cpal::default_host();

        let device = if let Some(name) = device_name {
            host.input_devices()
                .map_err(|e| Error::InvalidState(e.to_string()))?
                .find(|d| {
                    d.description()
                        .ok()
                        .map(|desc| desc.name().to_string())
                        .as_deref()
                        == Some(name)
                })
                .ok_or_else(|| {
                    Error::InvalidState(format!("Input device '{name}' not found"))
                })?
        } else {
            host.default_input_device()
                .ok_or_else(|| Error::InvalidState("No default input device".into()))?
        };

        // Use the device's preferred channel count so we don't fail on
        // devices that only support stereo.
        let default_cfg = device
            .default_input_config()
            .map_err(|e| Error::InvalidState(e.to_string()))?;
        let hw_channels = default_cfg.channels();

        // Format is deterministic from the request: a Station always emits
        // interleaved stereo. If the hardware is mono, the input handler
        // duplicates the single channel into L and R (dual-mono), so the frame
        // is always valid stereo and matches the stereo encoder.
        Ok(Self {
            format: if stereo {
                AudioFormat::STEREO_48KHZ_F32
            } else {
                AudioFormat::MONO_48KHZ_F32
            },
            frame_size,
            sequence: 0,
            buffer: Arc::new(Mutex::new(VecDeque::with_capacity(9_600))),
            stream: None,
            device,
            hw_channels,
            stereo,
            volume,
            overflow_warned: Arc::new(AtomicBool::new(false)),
        })
    }
}

/// Per-callback resampling state. Mono uses one resampler; stereo uses two
/// (one per channel) so L and R are resampled independently and never
/// interpolate into each other.
struct CaptureResample {
    /// Left (or mono) channel resampler.
    left: LinearResampler,
    /// Right channel resampler (stereo only).
    right: LinearResampler,
    /// Scratch buffers, reused across callbacks to avoid per-callback alloc.
    scratch_a: Vec<f32>,
    scratch_b: Vec<f32>,
    scratch_c: Vec<f32>,
}

#[allow(clippy::too_many_arguments)]
fn handle_cpal_input(
    buffer: &Arc<Mutex<VecDeque<f32>>>,
    data: &[f32],
    hw_channels: u16,
    stereo: bool,
    overflow_warned: &Arc<AtomicBool>,
    rs: &mut CaptureResample,
) {
    let Ok(mut buf) = buffer.lock() else { return };

    if stereo {
        // STEREO: take hardware channels 0 and 1 as L and R (duplicating a mono
        // device's single channel into both), resample each independently, and
        // re-interleave to L R L R. Independent resamplers keep the two receivers
        // from bleeding into each other at non-48k rates.
        let ch = hw_channels.max(1) as usize;
        let (lin, rin, out_l, out_r) =
            (&mut rs.scratch_a, &mut rs.scratch_b, &mut rs.scratch_c, &mut Vec::new());
        lin.clear();
        rin.clear();
        for frame in data.chunks(ch) {
            let l = frame[0];
            // Mono device: duplicate L into R (dual-mono). Stereo: use channel 1.
            let r = if frame.len() > 1 { frame[1] } else { l };
            lin.push(l);
            rin.push(r);
        }
        out_l.clear();
        rs.left.process(lin, out_l);
        rs.right.process(rin, out_r);
        // Re-interleave; the two resamplers emit the same count for equal input.
        let n = out_l.len().min(out_r.len());
        buf.reserve(n * 2);
        for i in 0..n {
            buf.push_back(out_l[i]);
            buf.push_back(out_r[i]);
        }
    } else {
        // MONO: downmix to mono, then resample the mono stream to 48 kHz.
        rs.scratch_a.clear();
        if hw_channels <= 1 {
            rs.left.process(data, &mut rs.scratch_a);
        } else {
            let mut mono: Vec<f32> = Vec::with_capacity(data.len() / hw_channels as usize + 1);
            for chunk in data.chunks(hw_channels as usize) {
                let sum: f32 = chunk.iter().sum();
                mono.push(sum / hw_channels as f32);
            }
            rs.left.process(&mono, &mut rs.scratch_a);
        }
        buf.extend(rs.scratch_a.iter().copied());
    }

    const MAX_SAMPLES: usize = 9_600;
    if buf.len() > MAX_SAMPLES {
        if !overflow_warned.swap(true, Ordering::Relaxed) {
            warn!("cpal capture buffer overflow, discarding oldest samples");
        }
        let excess = buf.len() - MAX_SAMPLES;
        let _ = buf.drain(..excess);
    }
}

impl AudioCapture for CpalCapture {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn read_frame(&mut self) -> Result<AudioFrame> {
        // One frame is frame_size samples PER CHANNEL; stereo interleaves two.
        let chans = if self.stereo { 2 } else { 1 };
        let frame_total = self.frame_size * chans;

        let mut buf = self
            .buffer
            .lock()
            .map_err(|e| Error::InvalidState(e.to_string()))?;

        if buf.len() < frame_total {
            return Err(Error::NotEnoughSamples);
        }

        // If the buffer has accumulated significantly more than one
        // frame, the encoding loop fell behind.  Skip old audio to
        // avoid sending stale voice packets. Keep interleave alignment by
        // dropping whole frames (multiple of frame_total).
        let max_queued = frame_total * 5; // ~100 ms at 48 kHz
        if buf.len() > max_queued {
            let excess = ((buf.len() - frame_total) / chans) * chans;
            warn!(
                "capture buffer overflow: {} samples ({:.0} ms), dropping {} stale samples",
                buf.len(),
                buf.len() as f32 / 48.0,
                excess,
            );
            let _ = buf.drain(..excess);
        }

        self.overflow_warned.store(false, Ordering::Relaxed);

        let vol = f32::from_bits(self.volume.load(Ordering::Relaxed));
        let mut data = Vec::with_capacity(frame_total * 4);
        for s in buf.drain(..frame_total) {
            data.extend_from_slice(&(s * vol).to_ne_bytes());
        }

        self.sequence += 1;
        Ok(AudioFrame {
            data,
            format: self.format,
            sequence: self.sequence,
            is_silent: false,
        })
    }

    fn start(&mut self) -> Result<()> {
        let buffer = self.buffer.clone();
        let hw_channels = self.hw_channels;
        let stereo = self.stereo;
        let overflow_warned = self.overflow_warned.clone();

        // Open at the device's own default rate rather than demanding 48 kHz.
        // Hardcoding 48 kHz made a 44.1 kHz-only device fail to open at all; a
        // `SignaLink` or USB dongle at 44.1 kHz is exactly the rig interface this
        // app must support. Read the real rate and resample to 48 kHz in the
        // input callback, so the samples we hand the pipeline are genuinely
        // 48 kHz.
        let default_cfg = self
            .device
            .default_input_config()
            .map_err(|e| Error::InvalidState(format!("input config query: {e}")))?;
        let device_rate = default_cfg.sample_rate();

        let stream_config = cpal::StreamConfig {
            channels: hw_channels,
            sample_rate: device_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        // Resamplers owned by the callback so their interpolation phase is
        // continuous across callback invocations. Stereo uses one per channel.
        let mut rs = CaptureResample {
            left: LinearResampler::new(device_rate, 48_000),
            right: LinearResampler::new(device_rate, 48_000),
            scratch_a: Vec::new(),
            scratch_b: Vec::new(),
            scratch_c: Vec::new(),
        };
        if !rs.left.is_identity() {
            debug!(
                "cpal capture: device rate {device_rate} Hz != 48 kHz; resampling on capture"
            );
        }

        let stream = self
            .device
            .build_input_stream(
                &stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    handle_cpal_input(
                        &buffer,
                        data,
                        hw_channels,
                        stereo,
                        &overflow_warned,
                        &mut rs,
                    );
                },
                |err| error!("cpal input error: {err}"),
                None,
            )
            .map_err(|e| Error::InvalidState(e.to_string()))?;

        stream
            .play()
            .map_err(|e| Error::InvalidState(e.to_string()))?;

        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.stream = None;
        if let Ok(mut buf) = self.buffer.lock() {
            buf.clear();
        }
        Ok(())
    }
}

// -- Factory -------------------------------------------------------

/// Desktop audio device factory backed by cpal.
pub struct CpalAudioFactory;

impl super::AudioDeviceFactory for CpalAudioFactory {
    fn create_capture(
        device_name: Option<&str>,
        frame_size: usize,
        volume: Arc<AtomicU32>,
    ) -> std::result::Result<Box<dyn AudioCapture>, String> {
        CpalCapture::new(device_name, frame_size, volume)
            .map(|c| Box::new(c) as _)
            .map_err(|e| format!("Capture init: {e}"))
    }

    fn create_capture_channels(
        device_name: Option<&str>,
        frame_size: usize,
        volume: Arc<AtomicU32>,
        stereo: bool,
    ) -> std::result::Result<Box<dyn AudioCapture>, String> {
        CpalCapture::new_with_channels(device_name, frame_size, volume, stereo)
            .map(|c| Box::new(c) as _)
            .map_err(|e| format!("Capture init: {e}"))
    }

    fn create_mixing_playback(
        device_name: Option<&str>,
        volume: Arc<AtomicU32>,
        buffers: mumble_protocol::audio::mixer::SpeakerBuffers,
        speaker_volumes: mumble_protocol::audio::mixer::SpeakerVolumes,
        stereo: Arc<mumble_protocol::audio::mixer::StereoControl>,
    ) -> std::result::Result<Box<dyn super::MixingPlayback>, String> {
        CpalMixingPlayback::new(device_name, volume, buffers, speaker_volumes, stereo)
            .map(|c| Box::new(c) as _)
            .map_err(|e| format!("Mixing playback init: {e}"))
    }
}

// -- Mixing playback -----------------------------------------------

/// Batch-drain up to `mono_needed` samples from every active speaker
/// into `mixed_buf` (summed/mixed).
///
/// Per-speaker volume is applied from `speaker_vols` (0.0-2.0,
/// defaulting to 1.0 when absent).
///
/// Returns `(had_data, valid_count, max_buf_before)`: whether any
/// speaker contributed, the number of valid mixed samples (max drained
/// from any single speaker), and the maximum buffer level across all
/// speakers before draining.  Positions beyond `valid_count` in
/// `mixed_buf` are zero and should be treated as underrun by the caller.
pub(super) fn batch_drain_speakers(
    bufs: &mut HashMap<u32, VecDeque<f32>>,
    speaker_vols: &HashMap<u32, f32>,
    mixed_buf: &mut Vec<f32>,
    samples_needed: usize,
) -> (bool, usize, usize) {
    // The sum is a plain element-wise add, so this is correct whether the
    // per-speaker buffers hold mono or interleaved stereo samples - the
    // caller just asks for frames*channels samples. Interleaved positions
    // line up because every speaker buffer uses the same channel layout.
    mixed_buf.clear();
    mixed_buf.resize(samples_needed, 0.0_f32);
    let mut any = false;
    let mut max_drained: usize = 0;
    let mut max_buf_before: usize = 0;

    for (session, buf) in bufs.iter_mut() {
        if buf.is_empty() {
            continue;
        }
        any = true;
        max_buf_before = max_buf_before.max(buf.len());
        let vol = speaker_vols.get(session).copied().unwrap_or(1.0);
        let n = buf.len().min(samples_needed);
        max_drained = max_drained.max(n);
        let (a, b) = buf.as_slices();
        let from_a = n.min(a.len());
        for (dst, src) in mixed_buf[..from_a].iter_mut().zip(&a[..from_a]) {
            *dst += *src * vol;
        }
        if from_a < n {
            let from_b = n - from_a;
            for (dst, src) in mixed_buf[from_a..n].iter_mut().zip(&b[..from_b]) {
                *dst += *src * vol;
            }
        }
        let _ = buf.drain(..n);
    }

    (any, max_drained, max_buf_before)
}

/// Multi-speaker mixing playback backed by cpal.
///
/// Instead of receiving frames via `write_frame`, this device reads
/// decoded samples directly from per-speaker ring buffers (managed by
/// [`AudioMixer`](mumble_protocol::audio::mixer::AudioMixer)) and sums
/// them in the cpal output callback.
pub struct CpalMixingPlayback {
    stream: Option<cpal::Stream>,
    device: cpal::Device,
    volume: Arc<AtomicU32>,
    buffers: mumble_protocol::audio::mixer::SpeakerBuffers,
    speaker_volumes: mumble_protocol::audio::mixer::SpeakerVolumes,
    stereo: Arc<mumble_protocol::audio::mixer::StereoControl>,
}

/// Mutable per-callback underrun tracking state for `CpalMixingPlayback`.
struct PlaybackState {
    last_sample: f32,
    in_underrun: bool,
    ramp_pos: usize,
    underrun_samples: usize,
}

/// Try to drain speaker buffers into `mixed_buf`. Returns
/// `Some((had_data, valid_count, buf_depth))` on success, or `None`
/// when the caller should fill zeros and return early (not yet primed
/// or lock failure).
fn try_drain_speakers_checked(
    buffers: &mumble_protocol::audio::mixer::SpeakerBuffers,
    speaker_volumes: &mumble_protocol::audio::mixer::SpeakerVolumes,
    primed_cb: &AtomicBool,
    mixed_buf: &mut Vec<f32>,
    samples_needed: usize,
) -> Option<(bool, usize, usize)> {
    // ~100 ms, scaled by the interleaved channel count of the speaker buffers.
    const PRE_BUFFER_SAMPLES: usize = 4800 * SRC_CHANNELS;
    let Ok(mut bufs) = buffers.lock() else { return None };
    if !primed_cb.load(Ordering::Relaxed) {
        let max_available = bufs.values().map(VecDeque::len).max().unwrap_or(0);
        if max_available < PRE_BUFFER_SAMPLES {
            return None;
        }
        primed_cb.store(true, Ordering::Relaxed);
    }
    // try_lock avoids blocking the real-time audio thread on a second
    // mutex; on contention we fall back to default volumes (1.0).
    // Borrow the guard instead of cloning the HashMap - this runs on
    // every output callback and a clone would allocate each time.
    let sv_guard = speaker_volumes.try_lock().ok();
    let empty = HashMap::new();
    let sv = sv_guard.as_deref().unwrap_or(&empty);
    Some(batch_drain_speakers(&mut bufs, sv, mixed_buf, samples_needed))
}

/// Apply a short anti-pop ramp when exiting an underrun, then return
/// the output sample. Updates `state` in-place.
fn apply_underrun_ramp(sample: f32, state: &mut PlaybackState) -> f32 {
    if !state.in_underrun {
        return sample;
    }
    const MAX_RAMP: usize = 48; // 1 ms at 48 kHz
    let ramp_len = state.underrun_samples.clamp(8, MAX_RAMP);
    state.ramp_pos += 1;
    if state.ramp_pos >= ramp_len {
        state.in_underrun = false;
        state.ramp_pos = 0;
        state.underrun_samples = 0;
        sample
    } else {
        let t = state.ramp_pos as f32 / ramp_len as f32;
        // Simple linear crossfade from last held value to new audio.
        state.last_sample * (1.0 - t) + sample * t
    }
}

/// Linearly interpolate one mono sample from the interleaved stereo
/// `mixed_buf` at the fractional source *frame* position given by
/// `out_index * src_ratio`. The two channels of each source frame are
/// averaged into one mono value (this cpal fallback is mono-out).
/// Returns `None` if the computed source frame falls outside `valid_frames`.
fn resample_linear(
    mixed_buf: &[f32],
    valid_frames: usize,
    drained: bool,
    out_index: usize,
    src_ratio: f64,
) -> Option<f32> {
    if !drained {
        return None;
    }
    let src_pos = out_index as f64 * src_ratio;
    let idx = src_pos as usize;
    if idx >= valid_frames {
        return None;
    }
    let frac = (src_pos - idx as f64) as f32;
    // Average the interleaved L/R of each source frame into mono.
    let mono_at = |frame: usize| -> f32 {
        let base = frame * SRC_CHANNELS;
        let mut sum = 0.0;
        for ch in 0..SRC_CHANNELS {
            sum += mixed_buf.get(base + ch).copied().unwrap_or(0.0);
        }
        sum / SRC_CHANNELS as f32
    };
    let s0 = mono_at(idx);
    let s1 = if idx + 1 < valid_frames { mono_at(idx + 1) } else { s0 };
    Some(s0 + (s1 - s0) * frac)
}

/// Write one output frame (mono sample duplicated to all channels) with
/// volume, underrun decay, and anti-pop ramp. Updates `state` in-place.
fn write_output_frame(
    frame: &mut [f32],
    sample_opt: Option<f32>,
    state: &mut PlaybackState,
    vol: f32,
) {
    const DECAY: f32 = 0.997;
    if let Some(raw) = sample_opt {
        let out = apply_underrun_ramp(raw, state);
        state.last_sample = out;
        let v = super::soft_clip(out * vol);
        for ch in frame.iter_mut() {
            *ch = v;
        }
    } else {
        state.in_underrun = true;
        state.ramp_pos = 0;
        state.underrun_samples += 1;
        state.last_sample *= DECAY;
        if state.last_sample.abs() < 1e-6 {
            state.last_sample = 0.0;
        }
        let v = super::soft_clip(state.last_sample * vol);
        for ch in frame.iter_mut() {
            *ch = v;
        }
    }
}

// SAFETY: See CpalCapture.
#[allow(unsafe_code, reason = "WASAPI COM objects are MTA-safe; cpal's !Send is a conservative cross-platform guard")]
unsafe impl Send for CpalMixingPlayback {}

impl CpalMixingPlayback {
    pub fn new(
        device_name: Option<&str>,
        volume: Arc<AtomicU32>,
        buffers: mumble_protocol::audio::mixer::SpeakerBuffers,
        speaker_volumes: mumble_protocol::audio::mixer::SpeakerVolumes,
        stereo: Arc<mumble_protocol::audio::mixer::StereoControl>,
    ) -> Result<Self> {
        let host = cpal::default_host();
        let device = if let Some(name) = device_name {
            host.output_devices()
                .map_err(|e| Error::InvalidState(e.to_string()))?
                .find(|d| {
                    d.description()
                        .ok()
                        .map(|desc| desc.name() == name)
                        .unwrap_or(false)
                })
                .ok_or_else(|| {
                    Error::InvalidState(format!("Output device not found: {name}"))
                })?
        } else {
            host.default_output_device()
                .ok_or_else(|| Error::InvalidState("No default output device".into()))?
        };

        Ok(Self {
            stream: None,
            device,
            volume,
            buffers,
            speaker_volumes,
            stereo,
        })
    }
}

/// Diagnostic counters for the playback callback, logged periodically.
struct CallbackDiag {
    callbacks: u64,
    underrun: u64,
    partial: u64,
    none: u64,
    peak: f32,
    buf_depth: usize,
}

impl CallbackDiag {
    fn log_if_due(&self, src_needed: usize, valid_count: usize, out_frames: usize, src_ratio: f64) {
        if self.callbacks.is_multiple_of(500) {
            warn!(
                "audio diag: cb={}, none={}, underrun={}, partial={}, \
                 src_needed={}, valid={}, out_frames={}, ratio={:.4}, \
                 peak={:.4}, buf={}",
                self.callbacks, self.none, self.underrun, self.partial,
                src_needed, valid_count, out_frames, src_ratio,
                self.peak, self.buf_depth,
            );
        }
    }
}

impl super::MixingPlayback for CpalMixingPlayback {
    fn start(&mut self) -> Result<()> {
        let buffers = self.buffers.clone();
        let volume = self.volume.clone();
        let speaker_volumes = self.speaker_volumes.clone();
        let stereo = self.stereo.clone();

        // Query the device's preferred output format. On WASAPI shared
        // mode the system mixer rate is fixed; hardcoding 48 kHz when
        // the device runs at a different rate causes the callback to
        // fire at the native rate while we feed 48 kHz data, starving
        // the speaker buffers.
        let default_config = self
            .device
            .default_output_config()
            .map_err(|e| Error::InvalidState(format!("output config query: {e}")))?;
        let device_rate = default_config.sample_rate();
        let device_channels = default_config.channels();

        warn!(
            "cpal output device: rate={} Hz, channels={}, format={:?}",
            device_rate, device_channels, default_config.sample_format()
        );

        let stream_config = cpal::StreamConfig {
            channels: device_channels,
            sample_rate: device_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        // Source-to-output sample ratio for resampling.
        // < 1.0 when the device rate exceeds 48 kHz (upsampling).
        let src_ratio: f64 = 48_000.0 / device_rate as f64;
        let out_channels = device_channels as usize;

        let primed = Arc::new(AtomicBool::new(false));
        let primed_cb = primed.clone();

        let mut diag = CallbackDiag { callbacks: 0, underrun: 0, partial: 0, none: 0, peak: 0.0, buf_depth: 0 };
        let mut pb_state = PlaybackState {
            last_sample: 0.0,
            in_underrun: false,
            ramp_pos: 0,
            underrun_samples: 0,
        };
        let mut mixed_buf: Vec<f32> = Vec::new();
        let mut consecutive_empty: u32 = 0;

        let stream = self
            .device
            .build_output_stream(
                &stream_config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let vol = f32::from_bits(volume.load(Ordering::Relaxed));
                    let out_frames = data.len() / out_channels;
                    // Source frames needed, then samples (interleaved stereo).
                    let src_frames_needed =
                        ((out_frames as f64 * src_ratio).ceil() as usize).max(1);
                    let src_needed = src_frames_needed * SRC_CHANNELS;

                    diag.callbacks += 1;

                    let drain_result = try_drain_speakers_checked(
                        &buffers,
                        &speaker_volumes,
                        &primed_cb,
                        &mut mixed_buf,
                        src_needed,
                    );
                    let Some((drained, valid_count, buf_depth)) = drain_result else {
                        diag.none += 1;
                        for frame in data.chunks_exact_mut(out_channels) {
                            write_output_frame(frame, None, &mut pb_state, vol);
                        }
                        return;
                    };
                    diag.buf_depth = buf_depth;

                    if !drained || valid_count == 0 {
                        diag.underrun += 1;
                        consecutive_empty += 1;
                        // Only reprime after sustained silence (1.5 s).
                        // Natural speech pauses (100-500 ms) are absorbed
                        // by the buffer; repriming during those pauses
                        // would introduce ~100 ms audible gaps.
                        const REPRIME_AFTER: u32 = 150;
                        if consecutive_empty >= REPRIME_AFTER {
                            primed_cb.store(false, Ordering::Relaxed);
                        }
                    } else {
                        consecutive_empty = 0;
                        if valid_count < src_needed {
                            diag.partial += 1;
                        }
                    }
                    diag.log_if_due(src_needed, valid_count, out_frames, src_ratio);

                    // Apply the operator's stereo-fold / per-channel gain to
                    // the interleaved mix before it is folded to mono below.
                    stereo.apply(&mut mixed_buf[..valid_count]);
                    let valid_frames = valid_count / SRC_CHANNELS;
                    for (i, frame) in data.chunks_exact_mut(out_channels).enumerate() {
                        let sample_opt = resample_linear(
                            &mixed_buf, valid_frames, drained, i, src_ratio,
                        );
                        if let Some(s) = &sample_opt {
                            diag.peak = diag.peak.max(s.abs());
                        }
                        write_output_frame(frame, sample_opt, &mut pb_state, vol);
                    }
                },
                |err| error!("cpal mixing output error: {err}"),
                None,
            )
            .map_err(|e| Error::InvalidState(e.to_string()))?;

        stream
            .play()
            .map_err(|e| Error::InvalidState(e.to_string()))?;

        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.stream = None;
        if let Ok(mut bufs) = self.buffers.lock() {
            bufs.clear();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, unused_results, reason = "acceptable in test code")]
    use super::*;
    use mumble_protocol::audio::sample::AudioFormat;

    #[test]
    #[ignore = "requires audio hardware - run with --ignored"]
    fn capture_reports_mono_48khz_f32_format() {
        let vol = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let Ok(capture) = CpalCapture::new(None, 960, vol) else {
            eprintln!("Skipping: no audio input device available");
            return;
        };
        assert_eq!(capture.format(), AudioFormat::MONO_48KHZ_F32);
    }

    #[test]
    #[ignore = "requires audio hardware - run with --ignored"]
    fn mixing_playback_can_be_created() {
        let vol = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let bufs: mumble_protocol::audio::mixer::SpeakerBuffers = Arc::new(
            Mutex::new(HashMap::new()),
        );
        let svols: mumble_protocol::audio::mixer::SpeakerVolumes =
            Arc::new(Mutex::new(HashMap::new()));
        let stereo = Arc::new(mumble_protocol::audio::mixer::StereoControl::default());
        let Ok(_p) = CpalMixingPlayback::new(None, vol, bufs, svols, stereo) else {
            eprintln!("Skipping: no audio output device available");
            return;
        };
    }

    #[test]
    #[ignore = "requires audio hardware - run with --ignored"]
    fn capture_read_before_start_returns_error() {
        let vol = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let Ok(mut capture) = CpalCapture::new(None, 960, vol) else { return };
        // Without calling start(), the buffer is empty so read_frame should fail.
        assert!(capture.read_frame().is_err());
    }

    #[test]
    fn batch_drain_sums_multiple_speakers() {
        let mut bufs = HashMap::new();
        bufs.insert(1u32, VecDeque::from(vec![0.5_f32; 10]));
        bufs.insert(2, VecDeque::from(vec![0.25; 10]));
        let speaker_vols: HashMap<u32, f32> = HashMap::new();
        let mut mixed = Vec::new();

        let (had, valid, _depth) = batch_drain_speakers(&mut bufs, &speaker_vols, &mut mixed, 10);
        assert!(had);
        assert_eq!(valid, 10);
        assert_eq!(mixed.len(), 10);
        for &s in &mixed {
            assert!((s - 0.75).abs() < 1e-6, "expected 0.75, got {s}");
        }
        // Both speakers fully drained (entries kept with empty buffers).
        for buf in bufs.values() {
            assert!(buf.is_empty());
        }
    }

    #[test]
    fn batch_drain_partial_speaker() {
        let mut bufs = HashMap::new();
        bufs.insert(1u32, VecDeque::from(vec![1.0_f32; 5]));
        let speaker_vols: HashMap<u32, f32> = HashMap::new();
        let mut mixed = Vec::new();

        let (had, valid, _depth) = batch_drain_speakers(&mut bufs, &speaker_vols, &mut mixed, 10);
        assert!(had);
        assert_eq!(valid, 5);
        // First 5 samples mixed, rest zero-filled.
        assert_eq!(mixed.len(), 10);
        for &s in &mixed[..5] {
            assert!((s - 1.0).abs() < 1e-6);
        }
        for &s in &mixed[5..] {
            assert!(s.abs() < 1e-6);
        }
    }

    #[test]
    fn batch_drain_empty_returns_false() {
        let mut bufs: HashMap<u32, VecDeque<f32>> = HashMap::new();
        let speaker_vols: HashMap<u32, f32> = HashMap::new();
        let mut mixed = Vec::new();
        assert!(!batch_drain_speakers(&mut bufs, &speaker_vols, &mut mixed, 10).0);
    }

    #[test]
    fn batch_drain_retains_leftover_samples() {
        let mut bufs = HashMap::new();
        bufs.insert(1u32, VecDeque::from(vec![0.5_f32; 20]));
        let speaker_vols: HashMap<u32, f32> = HashMap::new();
        let mut mixed = Vec::new();

        batch_drain_speakers(&mut bufs, &speaker_vols, &mut mixed, 10);
        // 10 drained, 10 remain.
        assert_eq!(bufs[&1].len(), 10);
    }

    #[test]
    fn batch_drain_applies_per_speaker_volume() {
        let mut bufs = HashMap::new();
        bufs.insert(1u32, VecDeque::from(vec![1.0_f32; 4]));
        bufs.insert(2u32, VecDeque::from(vec![1.0_f32; 4]));

        let mut speaker_vols = HashMap::new();
        speaker_vols.insert(1u32, 0.5_f32); // speaker 1 at 50%
        speaker_vols.insert(2u32, 1.5_f32); // speaker 2 at 150%

        let mut mixed = Vec::new();
        let (had, valid, _depth) = batch_drain_speakers(&mut bufs, &speaker_vols, &mut mixed, 4);
        assert!(had);
        assert_eq!(valid, 4);
        // Each sample = 1.0*0.5 + 1.0*1.5 = 2.0
        for &s in &mixed {
            assert!((s - 2.0).abs() < 1e-6, "expected 2.0, got {s}");
        }
    }

    #[test]
    fn batch_drain_default_volume_is_unity() {
        let mut bufs = HashMap::new();
        bufs.insert(1u32, VecDeque::from(vec![0.8_f32; 4]));

        // No entry for speaker 1 means default volume (1.0)
        let speaker_vols: HashMap<u32, f32> = HashMap::new();
        let mut mixed = Vec::new();
        batch_drain_speakers(&mut bufs, &speaker_vols, &mut mixed, 4);

        for &s in &mixed {
            assert!((s - 0.8).abs() < 1e-6, "expected 0.8 (unity), got {s}");
        }
    }

    fn make_speaker_buffers(
        data: HashMap<u32, VecDeque<f32>>,
    ) -> mumble_protocol::audio::mixer::SpeakerBuffers {
        Arc::new(Mutex::new(data))
    }

    fn make_speaker_volumes(
        data: HashMap<u32, f32>,
    ) -> mumble_protocol::audio::mixer::SpeakerVolumes {
        Arc::new(Mutex::new(data))
    }

    #[test]
    fn try_drain_returns_none_before_primed() {
        let bufs = make_speaker_buffers(HashMap::from([(1, VecDeque::from(vec![1.0; 100]))]));
        let vols = make_speaker_volumes(HashMap::new());
        let primed = AtomicBool::new(false);
        let mut mixed = Vec::new();

        // 100 samples is below PRE_BUFFER_SAMPLES (4800) -> returns None
        let result = try_drain_speakers_checked(&bufs, &vols, &primed, &mut mixed, 100);
        assert!(result.is_none());
        assert!(!primed.load(Ordering::Relaxed));
    }

    #[test]
    fn try_drain_primes_when_buffer_sufficient() {
        // Buffers are interleaved stereo now, so the prime threshold is
        // 4800*SRC_CHANNELS (9600) samples. 10000 clears it.
        let bufs = make_speaker_buffers(HashMap::from([(1, VecDeque::from(vec![1.0; 10000]))]));
        let vols = make_speaker_volumes(HashMap::new());
        let primed = AtomicBool::new(false);
        let mut mixed = Vec::new();

        let result = try_drain_speakers_checked(&bufs, &vols, &primed, &mut mixed, 480);
        assert!(result.is_some());
        assert!(primed.load(Ordering::Relaxed));
        let (had_data, valid, _depth) = result.unwrap();
        assert!(had_data);
        assert_eq!(valid, 480);
    }

    #[test]
    fn try_drain_stays_primed_when_empty() {
        let bufs = make_speaker_buffers(HashMap::from([(1, VecDeque::from(vec![1.0; 10000]))]));
        let vols = make_speaker_volumes(HashMap::new());
        let primed = AtomicBool::new(false);
        let mut mixed = Vec::new();

        // Prime the buffer
        let _ = try_drain_speakers_checked(&bufs, &vols, &primed, &mut mixed, 480);
        assert!(primed.load(Ordering::Relaxed));

        // Drain all remaining data
        {
            let mut locked = bufs.lock().unwrap();
            locked.get_mut(&1).unwrap().clear();
        }

        // Once primed, stays primed even when empty (returns zero-filled data)
        let result = try_drain_speakers_checked(&bufs, &vols, &primed, &mut mixed, 480);
        assert!(result.is_some());
        assert!(primed.load(Ordering::Relaxed));
        let (had_data, _valid, _depth) = result.unwrap();
        assert!(!had_data);
    }

    #[test]
    fn try_drain_stays_primed_with_data() {
        let bufs = make_speaker_buffers(HashMap::from([(1, VecDeque::from(vec![1.0; 10000]))]));
        let vols = make_speaker_volumes(HashMap::new());
        let primed = AtomicBool::new(false);
        let mut mixed = Vec::new();

        // Prime
        let _ = try_drain_speakers_checked(&bufs, &vols, &primed, &mut mixed, 480);
        assert!(primed.load(Ordering::Relaxed));

        // Drain again - still has data, should stay primed
        let result = try_drain_speakers_checked(&bufs, &vols, &primed, &mut mixed, 480);
        assert!(result.is_some());
        assert!(primed.load(Ordering::Relaxed));
    }

    #[test]
    fn resample_linear_returns_none_when_not_drained() {
        // Interleaved stereo buffer (5 frames); not drained -> None.
        let buf = vec![1.0_f32; 10];
        assert!(resample_linear(&buf, 5, false, 0, 1.0).is_none());
    }

    #[test]
    fn resample_linear_identity_at_ratio_one() {
        // Interleaved stereo: each frame is (L, R); the resampler returns
        // the per-frame L+R average. Here L==R so the average == the value.
        let frames = [0.0, 0.25, 0.5, 0.75, 1.0];
        let mut buf = Vec::new();
        for &v in &frames {
            buf.push(v); // L
            buf.push(v); // R
        }
        for (i, &expected) in frames.iter().enumerate() {
            let s = resample_linear(&buf, frames.len(), true, i, 1.0).unwrap();
            assert!((s - expected).abs() < 1e-6, "frame {i}: expected {expected}, got {s}");
        }
    }

    #[test]
    fn resample_linear_averages_stereo_frame_to_mono() {
        // One frame L=0.2, R=0.8 -> mono average 0.5.
        let buf = vec![0.2, 0.8];
        let s = resample_linear(&buf, 1, true, 0, 1.0).unwrap();
        assert!((s - 0.5).abs() < 1e-6, "got {s}");
    }

    #[test]
    fn resample_linear_upsamples() {
        // 2 source frames (L==R): frame0=0.0, frame1=1.0; ratio 0.5.
        let buf = vec![0.0, 0.0, 1.0, 1.0];
        let s0 = resample_linear(&buf, 2, true, 0, 0.5).unwrap();
        let s1 = resample_linear(&buf, 2, true, 1, 0.5).unwrap();
        assert!((s0 - 0.0).abs() < 1e-6);
        assert!((s1 - 0.5).abs() < 1e-6);
    }
}
