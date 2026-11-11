//! Running input and output streams.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Device, FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use rr_audio::playout::Playout;
use rr_client::InputPipe;

use crate::devices::{choose, find_device};

/// Backend failures.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// The requested device does not exist.
    #[error("no such audio device: {0}")]
    NoDevice(String),
    /// No usable configuration.
    #[error("audio configuration: {0}")]
    Config(String),
    /// The stream could not be built or started.
    #[error("audio stream: {0}")]
    Stream(String),
    /// The device uses a sample format this backend does not handle.
    #[error("unsupported sample format {0:?}")]
    Format(SampleFormat),
}

/// Stream health, readable from any thread.
#[derive(Debug, Default)]
pub struct Health {
    errors: AtomicU64,
    last: Mutex<String>,
}

impl Health {
    fn record(&self, e: &cpal::StreamError) {
        let _ = self.errors.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut s) = self.last.lock() {
            *s = e.to_string();
        }
        tracing::warn!("audio stream error: {e}");
    }

    /// Number of stream errors (dropouts, device removal) so far.
    #[must_use]
    pub fn error_count(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }

    /// The most recent error message.
    #[must_use]
    pub fn last_error(&self) -> String {
        self.last.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// A running capture stream. Dropping it stops capture.
pub struct RunningInput {
    _stream: Stream,
    /// Error counters.
    pub health: Arc<Health>,
}

impl fmt::Debug for RunningInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunningInput").finish_non_exhaustive()
    }
}

/// A running playback stream. Dropping it stops playback.
pub struct RunningOutput {
    _stream: Stream,
    /// Error counters.
    pub health: Arc<Health>,
}

impl fmt::Debug for RunningOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunningOutput").finish_non_exhaustive()
    }
}

fn build_input<T>(dev: &Device, cfg: &StreamConfig, mut pipe: InputPipe, health: &Arc<Health>) -> Result<Stream, BackendError>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let mut buf: Vec<f32> = Vec::new();
    let h = Arc::clone(health);
    dev.build_input_stream(
        cfg,
        move |data: &[T], _| {
            buf.clear();
            buf.extend(data.iter().map(|s| f32::from_sample(*s)));
            pipe.process(&buf);
        },
        move |e| h.record(&e),
        None,
    )
    .map_err(|e| BackendError::Stream(e.to_string()))
}

fn build_output<T>(
    dev: &Device,
    cfg: &StreamConfig,
    mut play: Playout,
    health: &Arc<Health>,
) -> Result<Stream, BackendError>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let channels = usize::from(cfg.channels);
    let mut stereo: Vec<f32> = Vec::new();
    let h = Arc::clone(health);
    dev.build_output_stream(
        cfg,
        move |data: &mut [T], _| {
            let frames = data.len() / channels;
            stereo.resize(frames * 2, 0.0);
            play.render(&mut stereo);
            for (out, s) in data.chunks_exact_mut(channels).zip(stereo.as_chunks::<2>().0) {
                map_frame(out, s[0], s[1]);
            }
        },
        move |e| h.record(&e),
        None,
    )
    .map_err(|e| BackendError::Stream(e.to_string()))
}

/// Lay a stereo frame onto `out`: mono devices get the average, wider devices
/// get left and right on the first two channels and silence elsewhere.
fn map_frame<T: Sample + FromSample<f32>>(out: &mut [T], l: f32, r: f32) {
    match out.len() {
        1 => out[0] = T::from_sample(0.5 * (l + r)),
        _ => {
            out[0] = T::from_sample(l);
            out[1] = T::from_sample(r);
            for o in &mut out[2..] {
                *o = T::from_sample(0.0_f32);
            }
        }
    }
}

/// Start capturing into `pipe`. Use [`crate::input_params`] first so the
/// capture pipeline matches the rate and channel count chosen here.
pub fn start_input(device_id: Option<&str>, pipe: InputPipe) -> Result<RunningInput, BackendError> {
    let dev = find_device(device_id, true)?;
    let sc = choose(&dev, true)?;
    let cfg = sc.config();
    let health = Arc::new(Health::default());
    let stream = match sc.sample_format() {
        SampleFormat::F32 => build_input::<f32>(&dev, &cfg, pipe, &health),
        SampleFormat::I16 => build_input::<i16>(&dev, &cfg, pipe, &health),
        SampleFormat::I32 => build_input::<i32>(&dev, &cfg, pipe, &health),
        SampleFormat::U16 => build_input::<u16>(&dev, &cfg, pipe, &health),
        other => Err(BackendError::Format(other)),
    }?;
    stream.play().map_err(|e| BackendError::Stream(e.to_string()))?;
    Ok(RunningInput { _stream: stream, health })
}

/// Start playing the engine's output.
pub fn start_output(device_id: Option<&str>, play: Playout) -> Result<RunningOutput, BackendError> {
    let dev = find_device(device_id, false)?;
    let sc = choose(&dev, false)?;
    let cfg = sc.config();
    let health = Arc::new(Health::default());
    let stream = match sc.sample_format() {
        SampleFormat::F32 => build_output::<f32>(&dev, &cfg, play, &health),
        SampleFormat::I16 => build_output::<i16>(&dev, &cfg, play, &health),
        SampleFormat::I32 => build_output::<i32>(&dev, &cfg, play, &health),
        SampleFormat::U16 => build_output::<u16>(&dev, &cfg, play, &health),
        other => Err(BackendError::Format(other)),
    }?;
    stream.play().map_err(|e| BackendError::Stream(e.to_string()))?;
    Ok(RunningOutput { _stream: stream, health })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_maps_straight_through() {
        let mut o = [0.0_f32; 2];
        map_frame(&mut o, 0.25, -0.5);
        assert_eq!(o, [0.25, -0.5]);
    }

    #[test]
    fn mono_gets_the_average_and_wide_gets_silence() {
        let mut m = [0.0_f32; 1];
        map_frame(&mut m, 0.5, 0.25);
        assert!((m[0] - 0.375).abs() < 1e-6);
        let mut w = [1.0_f32; 4];
        map_frame(&mut w, 0.1, 0.2);
        assert_eq!(w, [0.1, 0.2, 0.0, 0.0]);
    }
}
