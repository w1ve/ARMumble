//! Oboe-based audio capture and mixing playback for Android.
//!
//! Mirrors the desktop cpal implementation: callbacks push/pull samples
//! through shared ring buffers, while the mixer drives speaker-level I/O
//! via the [`AudioCapture`] and [`MixingPlayback`](super::MixingPlayback) traits.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use oboe::{
    AudioInputCallback, AudioOutputCallback, AudioOutputStreamSafe, AudioStream,
    AudioStreamAsync, AudioStreamBuilder, ContentType, DataCallbackResult, Input,
    InputPreset, Mono, Output, PerformanceMode, SessionId, SharingMode, Stereo, Usage,
};
use tracing::{error, warn};

use mumble_protocol::audio::capture::AudioCapture;
use mumble_protocol::audio::mixer::{SpeakerBuffers, SpeakerVolumes, StereoControl};
use mumble_protocol::audio::sample::{AudioFormat, AudioFrame};
use mumble_protocol::error::{Error, Result};

// -- Capture --------------------------------------------------------

/// Callback struct that receives microphone samples from oboe and
/// pushes them into a shared ring buffer.
struct CaptureCallback {
    buffer: Arc<Mutex<VecDeque<f32>>>,
    overflow_warned: Arc<AtomicBool>,
}

impl AudioInputCallback for CaptureCallback {
    type FrameType = (f32, Mono);

    fn on_audio_ready(
        &mut self,
        _stream: &mut dyn oboe::AudioInputStreamSafe,
        frames: &[f32],
    ) -> DataCallbackResult {
        if let Ok(mut buf) = self.buffer.lock() {
            buf.extend(frames.iter().copied());
            // Cap at ~200 ms (9 600 samples at 48 kHz) to avoid
            // accumulating stale audio when the encoding loop is
            // throttled (e.g. app backgrounded on Android).
            if buf.len() > 9_600 {
                if !self.overflow_warned.swap(true, Ordering::Relaxed) {
                    warn!("oboe capture buffer overflow, discarding oldest samples");
                }
                let excess = buf.len() - 9_600;
                let _ = buf.drain(..excess);
            }
        }
        DataCallbackResult::Continue
    }

    fn on_error_before_close(
        &mut self,
        _stream: &mut dyn oboe::AudioInputStreamSafe,
        e: oboe::Error,
    ) {
        error!("oboe capture error: {e:?}");
    }
}

/// Captures microphone input via oboe (Android) and makes it available
/// as [`AudioFrame`]s through the [`AudioCapture`] trait.
pub struct OboeCapture {
    format: AudioFormat,
    frame_size: usize,
    sequence: u64,
    buffer: Arc<Mutex<VecDeque<f32>>>,
    stream: Option<AudioStreamAsync<Input, CaptureCallback>>,
    volume: Arc<AtomicU32>,
    overflow_warned: Arc<AtomicBool>,
}

// SAFETY: The oboe stream is only accessed from the pipeline thread
// after construction. The audio callback thread communicates solely
// through the `Arc<Mutex<VecDeque>>` buffer.
#[allow(unsafe_code, reason = "oboe stream accessed from single pipeline thread; callback uses Arc<Mutex>")]
unsafe impl Send for OboeCapture {}

impl OboeCapture {
    /// Create a new capture source for Android.
    ///
    /// * `frame_size` - samples per channel per frame (960 for Mumble 20 ms).
    /// * `volume` - shared atomic volume multiplier (f32 bits as u32).
    pub fn new(frame_size: usize, volume: Arc<AtomicU32>) -> Result<Self> {
        Ok(Self {
            format: AudioFormat::MONO_48KHZ_F32,
            frame_size,
            sequence: 0,
            buffer: Arc::new(Mutex::new(VecDeque::with_capacity(9_600))),
            stream: None,
            volume,
            overflow_warned: Arc::new(AtomicBool::new(false)),
        })
    }
}

impl AudioCapture for OboeCapture {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn read_frame(&mut self) -> Result<AudioFrame> {
        let mut buf = self
            .buffer
            .lock()
            .map_err(|e| Error::InvalidState(e.to_string()))?;

        if buf.len() < self.frame_size {
            return Err(Error::NotEnoughSamples);
        }

        // If the buffer has accumulated significantly more than one
        // frame, the encoding loop fell behind (e.g. app
        // backgrounded).  Skip old audio to avoid sending stale
        // voice packets that cause the chipmunk effect.
        let max_queued = self.frame_size * 5; // ~100 ms at 48 kHz
        if buf.len() > max_queued {
            let excess = buf.len() - self.frame_size;
            warn!(
                "capture buffer overflow: {} samples ({:.0} ms), dropping {} stale samples",
                buf.len(),
                buf.len() as f32 / 48.0,
                excess,
            );
            let _ = buf.drain(..excess);
        }

        let vol = f32::from_bits(self.volume.load(Ordering::Relaxed));
        self.overflow_warned.store(false, Ordering::Relaxed);
        let mut data = Vec::with_capacity(self.frame_size * 4);
        for s in buf.drain(..self.frame_size) {
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
        let callback = CaptureCallback {
            buffer: self.buffer.clone(),
            overflow_warned: self.overflow_warned.clone(),
        };

        let mut stream = AudioStreamBuilder::default()
            .set_input()
            .set_performance_mode(PerformanceMode::LowLatency)
            .set_sharing_mode(SharingMode::Shared)
            .set_format::<f32>()
            .set_channel_count::<Mono>()
            .set_sample_rate(48_000)
            .set_input_preset(InputPreset::VoiceRecognition)
            .set_session_id(SessionId::None)
            .set_callback(callback)
            .open_stream()
            .map_err(|e| Error::InvalidState(format!("oboe input open: {e:?}")))?;

        stream
            .start()
            .map_err(|e| Error::InvalidState(format!("oboe input start: {e:?}")))?;

        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(mut stream) = self.stream.take() {
            let _ = stream.stop();
        }
        if let Ok(mut buf) = self.buffer.lock() {
            buf.clear();
        }
        Ok(())
    }
}

// -- Factory -------------------------------------------------------

/// Android audio device factory backed by oboe.
pub struct OboeAudioFactory;

impl super::AudioDeviceFactory for OboeAudioFactory {
    fn create_capture(
        _device_name: Option<&str>,
        frame_size: usize,
        volume: Arc<AtomicU32>,
    ) -> std::result::Result<Box<dyn AudioCapture>, String> {
        OboeCapture::new(frame_size, volume)
            .map(|c| Box::new(c) as _)
            .map_err(|e| format!("Capture init: {e}"))
    }

    fn create_mixing_playback(
        _device_name: Option<&str>,
        volume: Arc<AtomicU32>,
        buffers: SpeakerBuffers,
        speaker_volumes: SpeakerVolumes,
        stereo: Arc<StereoControl>,
    ) -> std::result::Result<Box<dyn super::MixingPlayback>, String> {
        OboeMixingPlayback::new(volume, buffers, speaker_volumes, stereo)
            .map(|c| Box::new(c) as _)
            .map_err(|e| format!("Mixing playback init: {e}"))
    }
}

// -- Mixing playback -----------------------------------------------

/// Callback for the mixing playback stream.
///
/// The [`AudioMixer`] decodes every speaker's Opus into INTERLEAVED STEREO
/// (L,R,L,R...) and appends it to that speaker's `VecDeque<f32>`. So this is a
/// STEREO output stream: each output frame is an `(L, R)` pair, and we pop two
/// samples (L then R) from each speaker buffer, sum per channel, apply the
/// per-speaker volumes, then the operator's [`StereoControl`] (Mix/LR/Swap/pan)
/// on the summed pair, soft-clip, and scale by the master output volume.
///
/// A previous version opened a MONO stream and popped a single sample per
/// speaker, which read the interleaved L,R buffer as consecutive mono samples
/// and played everything at half speed / wrong pitch. Matching the mixer's
/// stereo layout fixes that and gives true stereo receive on the phone.
struct MixingPlaybackCallback {
    buffers: SpeakerBuffers,
    speaker_volumes: SpeakerVolumes,
    stereo: Arc<StereoControl>,
    volume: Arc<AtomicU32>,
    /// Held amplitude per channel for the underrun fade-out (L, R).
    last: (f32, f32),
}

impl MixingPlaybackCallback {
    /// Pop one interleaved stereo frame (L,R) summed across all speakers,
    /// weighted by each speaker's volume. Returns `None` if no speaker had a
    /// full stereo frame available (underrun).
    fn mix_frame(
        bufs: &mut std::collections::HashMap<u32, VecDeque<f32>>,
        svols: &std::collections::HashMap<u32, f32>,
    ) -> Option<(f32, f32)> {
        let mut l = 0.0f32;
        let mut r = 0.0f32;
        let mut any = false;
        for (session, buf) in bufs.iter_mut() {
            // Need a full L,R pair; a buffer with an odd straggler is skipped
            // this frame (it realigns on the next decode).
            if buf.len() >= 2 {
                let sv = svols.get(session).copied().unwrap_or(1.0);
                let ls = buf.pop_front().unwrap_or(0.0);
                let rs = buf.pop_front().unwrap_or(0.0);
                l += ls * sv;
                r += rs * sv;
                any = true;
            }
        }
        any.then_some((l, r))
    }
}

impl AudioOutputCallback for MixingPlaybackCallback {
    type FrameType = (f32, Stereo);

    fn on_audio_ready(
        &mut self,
        _stream: &mut dyn AudioOutputStreamSafe,
        frames: &mut [(f32, f32)],
    ) -> DataCallbackResult {
        use mumble_protocol::audio::mixer::StereoOutMode;
        let vol = f32::from_bits(self.volume.load(Ordering::Relaxed));
        let (mode, lg, rg) = self.stereo.get();

        let Ok(mut bufs) = self.buffers.lock() else {
            for f in frames.iter_mut() {
                *f = (0.0, 0.0);
            }
            return DataCallbackResult::Continue;
        };
        let svols = self.speaker_volumes.lock().ok();
        let empty = std::collections::HashMap::new();
        let sv = svols.as_deref().unwrap_or(&empty);

        for out in frames.iter_mut() {
            let (raw_l, raw_r) = match Self::mix_frame(&mut bufs, sv) {
                Some(pair) => {
                    self.last = pair;
                    pair
                }
                None => {
                    // Underrun: decay the held pair toward silence (no click).
                    self.last.0 *= 0.99;
                    self.last.1 *= 0.99;
                    if self.last.0.abs() < 1e-6 {
                        self.last.0 = 0.0;
                    }
                    if self.last.1.abs() < 1e-6 {
                        self.last.1 = 0.0;
                    }
                    self.last
                }
            };

            // Operator stereo rendering: per-channel gain first, then the
            // fold mode, matching mixer::apply_stereo_mode exactly (one frame).
            let gl = raw_l * lg;
            let gr = raw_r * rg;
            let (l, r) = match mode {
                StereoOutMode::Stereo => (gl, gr),
                StereoOutMode::Mono => {
                    let m = (gl + gr) * 0.5;
                    (m, m)
                }
                StereoOutMode::SwapLr => (gr, gl),
            };

            *out = (super::soft_clip(l * vol), super::soft_clip(r * vol));
        }
        bufs.retain(|_, b| !b.is_empty());

        DataCallbackResult::Continue
    }

    fn on_error_before_close(
        &mut self,
        _stream: &mut dyn AudioOutputStreamSafe,
        e: oboe::Error,
    ) {
        error!("oboe mixing playback error: {e:?}");
    }
}

/// Multi-speaker mixing playback backed by oboe (Android), stereo output.
pub struct OboeMixingPlayback {
    stream: Option<AudioStreamAsync<Output, MixingPlaybackCallback>>,
    volume: Arc<AtomicU32>,
    buffers: SpeakerBuffers,
    speaker_volumes: SpeakerVolumes,
    stereo: Arc<StereoControl>,
}

// SAFETY: Same justification as OboeCapture.
#[allow(unsafe_code, reason = "oboe stream accessed from single pipeline thread; callback uses Arc<Mutex>")]
unsafe impl Send for OboeMixingPlayback {}

impl OboeMixingPlayback {
    pub fn new(
        volume: Arc<AtomicU32>,
        buffers: SpeakerBuffers,
        speaker_volumes: SpeakerVolumes,
        stereo: Arc<StereoControl>,
    ) -> Result<Self> {
        Ok(Self {
            stream: None,
            volume,
            buffers,
            speaker_volumes,
            stereo,
        })
    }
}

impl super::MixingPlayback for OboeMixingPlayback {
    fn start(&mut self) -> Result<()> {
        let callback = MixingPlaybackCallback {
            buffers: self.buffers.clone(),
            speaker_volumes: self.speaker_volumes.clone(),
            stereo: self.stereo.clone(),
            volume: self.volume.clone(),
            last: (0.0, 0.0),
        };

        let mut stream = AudioStreamBuilder::default()
            .set_performance_mode(PerformanceMode::LowLatency)
            .set_sharing_mode(SharingMode::Shared)
            .set_format::<f32>()
            .set_channel_count::<Stereo>()
            .set_sample_rate(48_000)
            .set_usage(Usage::VoiceCommunication)
            .set_content_type(ContentType::Speech)
            .set_callback(callback)
            .open_stream()
            .map_err(|e| Error::InvalidState(format!("oboe mixing output open: {e:?}")))?;

        stream
            .start()
            .map_err(|e| Error::InvalidState(format!("oboe mixing output start: {e:?}")))?;

        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(mut stream) = self.stream.take() {
            let _ = stream.stop();
        }
        if let Ok(mut bufs) = self.buffers.lock() {
            bufs.clear();
        }
        Ok(())
    }
}
