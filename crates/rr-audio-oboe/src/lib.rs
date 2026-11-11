//! Android sound backend (Oboe) for the `RemoteRadio` audio engine.
//!
//! * Input is opened with the **Unprocessed** preset, falling back to
//!   **Generic**. The voice-communication and voice-recognition presets are
//!   never used because they enable the phone's own AGC / noise suppression.
//!   Unprocessed audio is quiet; use the input level control to raise it.
//! * Output is a media-usage stereo stream.
//! * Both run at 48 kHz stereo, so the engine needs no rate conversion here.
//!   When the hardware differs, Oboe's high-quality converter bridges it.
//! * If a device disappears (headset, USB interface) the streams are
//!   re-opened automatically.
//!
//! The streams are not `Send`; [`run`] creates and owns them on the calling
//! thread until asked to stop, so call it from a dedicated audio thread.
#![cfg(target_os = "android")]

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use oboe::{
    AudioInputCallback, AudioInputStreamSafe, AudioOutputCallback, AudioOutputStreamSafe, AudioStream,
    AudioStreamAsync, AudioStreamBuilder, ContentType, DataCallbackResult, Input, InputPreset, Mono, Output,
    PerformanceMode, SampleRateConversionQuality, SessionId, SharingMode, Stereo, Usage,
};
use rr_audio::playout::Playout;
use rr_client::InputPipe;

/// The only rate and layout the app asks the engine for on Android.
pub const RATE: u32 = 48_000;
/// Channels delivered to the engine.
pub const CHANNELS: u16 = 2;
/// How long to wait before re-opening after a device error.
const REOPEN_DELAY: Duration = Duration::from_millis(500);

/// Backend failures.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// A stream could not be opened or started.
    #[error("audio stream: {0}")]
    Stream(String),
}

/// Stream health shared with the UI.
#[derive(Debug, Default)]
pub struct Health {
    errors: AtomicU64,
    failed: AtomicBool,
    last: Mutex<String>,
}

impl Health {
    fn record(&self, who: &str, e: &dyn fmt::Debug) {
        let _ = self.errors.fetch_add(1, Ordering::Relaxed);
        self.failed.store(true, Ordering::Relaxed);
        let text = format!("{who}: {e:?}");
        tracing::warn!("oboe error: {text}");
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = text;
    }

    /// Number of stream errors so far.
    #[must_use]
    pub fn error_count(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }

    /// The latest error text.
    #[must_use]
    pub fn last_error(&self) -> String {
        self.last.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

type SharedPipe = Arc<Mutex<InputPipe>>;
type SharedPlay = Arc<Mutex<Playout>>;

fn feed(pipe: &SharedPipe, samples: &[f32]) {
    if let Ok(mut p) = pipe.lock() {
        p.process(samples);
    }
}

struct CaptureStereo {
    pipe: SharedPipe,
    buf: Vec<f32>,
    health: Arc<Health>,
}

impl AudioInputCallback for CaptureStereo {
    type FrameType = (f32, Stereo);

    fn on_audio_ready(&mut self, _s: &mut dyn AudioInputStreamSafe, frames: &[(f32, f32)]) -> DataCallbackResult {
        self.buf.clear();
        for (l, r) in frames {
            self.buf.push(*l);
            self.buf.push(*r);
        }
        feed(&self.pipe, &self.buf);
        DataCallbackResult::Continue
    }

    fn on_error_after_close(&mut self, _s: &mut dyn AudioInputStreamSafe, e: oboe::Error) {
        self.health.record("input", &e);
    }
}

struct CaptureMono {
    pipe: SharedPipe,
    buf: Vec<f32>,
    health: Arc<Health>,
}

impl AudioInputCallback for CaptureMono {
    type FrameType = (f32, Mono);

    fn on_audio_ready(&mut self, _s: &mut dyn AudioInputStreamSafe, frames: &[f32]) -> DataCallbackResult {
        self.buf.clear();
        for s in frames {
            self.buf.push(*s);
            self.buf.push(*s);
        }
        feed(&self.pipe, &self.buf);
        DataCallbackResult::Continue
    }

    fn on_error_after_close(&mut self, _s: &mut dyn AudioInputStreamSafe, e: oboe::Error) {
        self.health.record("input", &e);
    }
}

struct Playback {
    play: SharedPlay,
    buf: Vec<f32>,
    health: Arc<Health>,
}

impl AudioOutputCallback for Playback {
    type FrameType = (f32, Stereo);

    fn on_audio_ready(&mut self, _s: &mut dyn AudioOutputStreamSafe, frames: &mut [(f32, f32)]) -> DataCallbackResult {
        self.buf.clear();
        self.buf.resize(frames.len() * 2, 0.0);
        if let Ok(mut p) = self.play.lock() {
            p.render(&mut self.buf);
        }
        for (out, s) in frames.iter_mut().zip(self.buf.as_chunks::<2>().0) {
            *out = (s[0], s[1]);
        }
        DataCallbackResult::Continue
    }

    fn on_error_after_close(&mut self, _s: &mut dyn AudioOutputStreamSafe, e: oboe::Error) {
        self.health.record("output", &e);
    }
}

/// An open input stream of either layout.
enum InputStream {
    Stereo(AudioStreamAsync<Input, CaptureStereo>),
    Mono(AudioStreamAsync<Input, CaptureMono>),
}

impl InputStream {
    fn stop(&mut self) {
        let _ = match self {
            Self::Stereo(s) => s.stop(),
            Self::Mono(s) => s.stop(),
        };
    }
}

fn open_input(pipe: &SharedPipe, health: &Arc<Health>) -> Result<InputStream, BackendError> {
    let stereo = |preset: InputPreset| {
        let cb = CaptureStereo { pipe: Arc::clone(pipe), buf: Vec::new(), health: Arc::clone(health) };
        AudioStreamBuilder::default()
            .set_input()
            .set_performance_mode(PerformanceMode::LowLatency)
            .set_sharing_mode(SharingMode::Shared)
            .set_format::<f32>()
            .set_channel_count::<Stereo>()
            .set_sample_rate(RATE as i32)
            .set_sample_rate_conversion_quality(SampleRateConversionQuality::High)
            .set_input_preset(preset)
            .set_session_id(SessionId::None)
            .set_callback(cb)
            .open_stream()
    };
    let mono = |preset: InputPreset| {
        let cb = CaptureMono { pipe: Arc::clone(pipe), buf: Vec::new(), health: Arc::clone(health) };
        AudioStreamBuilder::default()
            .set_input()
            .set_performance_mode(PerformanceMode::LowLatency)
            .set_sharing_mode(SharingMode::Shared)
            .set_format::<f32>()
            .set_channel_count::<Mono>()
            .set_sample_rate(RATE as i32)
            .set_sample_rate_conversion_quality(SampleRateConversionQuality::High)
            .set_input_preset(preset)
            .set_session_id(SessionId::None)
            .set_callback(cb)
            .open_stream()
    };
    // Prefer true stereo, then mono; prefer unprocessed, then generic. Never a
    // voice preset: those switch on the phone's AGC and noise suppression.
    for preset in [InputPreset::Unprocessed, InputPreset::Generic] {
        if let Ok(s) = stereo(preset) {
            return Ok(InputStream::Stereo(s));
        }
    }
    for preset in [InputPreset::Unprocessed, InputPreset::Generic] {
        match mono(preset) {
            Ok(s) => return Ok(InputStream::Mono(s)),
            Err(e) if preset == InputPreset::Generic => return Err(BackendError::Stream(format!("input open: {e:?}"))),
            Err(_) => {}
        }
    }
    Err(BackendError::Stream("input open failed".into()))
}

fn open_output(play: &SharedPlay, health: &Arc<Health>) -> Result<AudioStreamAsync<Output, Playback>, BackendError> {
    let cb = Playback { play: Arc::clone(play), buf: Vec::new(), health: Arc::clone(health) };
    AudioStreamBuilder::default()
        .set_output()
        .set_performance_mode(PerformanceMode::LowLatency)
        .set_sharing_mode(SharingMode::Shared)
        .set_format::<f32>()
        .set_channel_count::<Stereo>()
        .set_sample_rate(RATE as i32)
        .set_sample_rate_conversion_quality(SampleRateConversionQuality::High)
        .set_usage(Usage::Media)
        .set_content_type(ContentType::Music)
        .set_callback(cb)
        .open_stream()
        .map_err(|e| BackendError::Stream(format!("output open: {e:?}")))
}

/// Open and start both streams once.
fn open_both(
    pipe: &SharedPipe,
    play: &SharedPlay,
    health: &Arc<Health>,
) -> Result<(InputStream, AudioStreamAsync<Output, Playback>), BackendError> {
    let mut input = open_input(pipe, health)?;
    let mut output = open_output(play, health)?;
    health.failed.store(false, Ordering::Relaxed);
    let started = match &mut input {
        InputStream::Stereo(s) => s.start(),
        InputStream::Mono(s) => s.start(),
    };
    started.map_err(|e| BackendError::Stream(format!("input start: {e:?}")))?;
    output.start().map_err(|e| BackendError::Stream(format!("output start: {e:?}")))?;
    Ok((input, output))
}

/// Run the audio streams until `stop` fires or its sender is dropped.
///
/// `ready` receives the first open's result so the caller can report a failure
/// to the user. After that, device errors are retried until stopped.
pub fn run(
    pipe: InputPipe,
    play: Playout,
    health: &Arc<Health>,
    ready: &std::sync::mpsc::Sender<Result<(), String>>,
    stop: &Receiver<()>,
) {
    let pipe: SharedPipe = Arc::new(Mutex::new(pipe));
    let play: SharedPlay = Arc::new(Mutex::new(play));
    let mut announced = false;
    loop {
        match open_both(&pipe, &play, health) {
            Ok((mut input, mut output)) => {
                if !announced {
                    announced = true;
                    let _ = ready.send(Ok(()));
                }
                // Wait for a stop request, or for an error that closed a stream.
                let stopped = loop {
                    match stop.recv_timeout(Duration::from_millis(250)) {
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => break true,
                        Err(RecvTimeoutError::Timeout) if health.failed.load(Ordering::Relaxed) => break false,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                };
                input.stop();
                let _ = output.stop();
                drop((input, output));
                if stopped {
                    return;
                }
            }
            Err(e) => {
                if !announced {
                    let _ = ready.send(Err(e.to_string()));
                    return;
                }
                health.record("reopen", &e);
            }
        }
        match stop.recv_timeout(REOPEN_DELAY) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}
