//! "Play Stereo Test" — transmit an embedded stereo white-noise test tone to
//! the server so a remote operator can judge left/all/right channel separation
//! on the air.
//!
//! This is the headline capability of a *stereo* Mumble client, so the test
//! must transmit genuine stereo: a dedicated Opus encoder is built in STEREO at
//! a bitrate above the ~96 kbps separation cliff with phase-inversion disabled,
//! and the embedded 48 kHz stereo WAV is streamed through it straight to the
//! network — bypassing the mono mic pipeline entirely. The mic is paused for
//! the duration so the two streams don't interleave on the same sender session.

use std::io::Cursor;
use std::time::Duration;

use tauri::Emitter;
use tracing::{info, warn};

use mumble_protocol::audio::encoder::{
    AudioEncoder, ForceChannels, OpusApplication, OpusEncoder, OpusEncoderConfig,
};
use mumble_protocol::audio::sample::{AudioFormat, AudioFrame};
use mumble_protocol::message::UdpMessage;
use mumble_protocol::proto::mumble_udp;

use super::AppState;

/// The embedded stereo test asset: 48 kHz, 2-channel, 16-bit PCM WAV.
/// White noise panned all / left / right so separation is obvious by ear.
static STEREO_TEST_WAV: &[u8] = include_bytes!("../../assets/stereotest.wav");

/// 20 ms frame at 48 kHz = 960 samples per channel.
const FRAME_SAMPLES_PER_CH: usize = 960;
/// Bitrate for the test: must sit above the ~96 kbps stereo-separation cliff,
/// or the channels bleed together and the test is meaningless.
const STEREO_TEST_BITRATE: i32 = 128_000;

impl AppState {
    /// Start streaming the embedded stereo test tone to the server. Pauses the
    /// mic for the duration, emits `stereo-test-state` (true now, false when it
    /// finishes or is stopped), and stores the task handle so it can be aborted.
    pub fn play_stereo_test(&self) -> Result<(), String> {
        // Must be connected.
        let handle = {
            let st = self.inner.snapshot();
            let state = st.lock().map_err(|e| e.to_string())?;
            state.conn.client_handle.clone()
        }
        .ok_or("Not connected")?;

        // Only one test at a time.
        self.stop_stereo_test();

        // Pause the mic so the test stream owns the sender session cleanly.
        // Record (on AppState) whether it was live so either the task's normal
        // completion OR a `stop_stereo_test` abort can resume it exactly once.
        let resume_mic = self.voice_state() == super::types::VoiceState::Active;
        self.stereo_test_resume_mic
            .store(resume_mic, std::sync::atomic::Ordering::Relaxed);
        if resume_mic {
            self.stop_outbound();
        }

        let app = self.app_handle().ok_or("No app handle")?;
        let outbound_target = self.outbound_target.clone();

        let task = tauri::async_runtime::spawn(async move {
            use tauri::Manager;
            let _ = app.emit("stereo-test-state", true);

            // Force the normal target (own channel) for the test; restore after.
            let prev_target = outbound_target.load(std::sync::atomic::Ordering::Relaxed);
            outbound_target.store(0, std::sync::atomic::Ordering::Relaxed);

            if let Err(e) = stream_stereo_test(&handle).await {
                warn!("stereo test: {e}");
            }

            outbound_target.store(prev_target, std::sync::atomic::Ordering::Relaxed);

            // Resume the mic if the test paused a live one (claim the flag so a
            // racing stop_stereo_test doesn't resume twice). Clear the handle so
            // stop() after natural completion is a no-op.
            let state = app.state::<AppState>();
            if state
                .stereo_test_resume_mic
                .swap(false, std::sync::atomic::Ordering::Relaxed)
            {
                if let Err(e) = state.restart_outbound() {
                    warn!("stereo test: failed to resume mic: {e}");
                }
            }
            if let Ok(mut s) = state.inner.snapshot().lock() {
                s.audio.stereo_test_handle = None;
            }
            let _ = app.emit("stereo-test-state", false);
        });

        if let Ok(mut state) = self.inner.snapshot().lock() {
            state.audio.stereo_test_handle = Some(task);
        }
        Ok(())
    }

    /// Stop the stereo test if it is running (aborts the transmit task) and
    /// resume the mic if the test had paused a live one.
    pub fn stop_stereo_test(&self) {
        let was_running = if let Ok(mut state) = self.inner.snapshot().lock() {
            state.audio.stereo_test_handle.take().map(|h| h.abort()).is_some()
        } else {
            false
        };
        if was_running {
            // The aborted task can't run its own resume, so do it here.
            if self.stereo_test_resume_mic.swap(false, std::sync::atomic::Ordering::Relaxed) {
                let _ = self.restart_outbound();
            }
            if let Some(app) = self.app_handle() {
                let _ = app.emit("stereo-test-state", false);
            }
        }
    }
}

/// Decode the embedded WAV and stream it as stereo Opus, paced at real time.
async fn stream_stereo_test(
    handle: &mumble_protocol::client::ClientHandle,
) -> Result<(), String> {
    let pcm = decode_wav_stereo_f32()?;

    let mut encoder = OpusEncoder::new(
        OpusEncoderConfig {
            bitrate: STEREO_TEST_BITRATE,
            frame_size: FRAME_SAMPLES_PER_CH,
            application: OpusApplication::Audio,
            vbr: true,
            complexity: 10,
            fec: false,
            packet_loss_percent: 0,
            dtx: false,
            // Pin stereo and keep phase so the far end hears true separation.
            force_channels: ForceChannels::Stereo,
            phase_inversion_disabled: true,
        },
        AudioFormat::STEREO_48KHZ_F32,
    )
    .map_err(|e| format!("encoder init: {e}"))?;

    let interleaved_per_frame = FRAME_SAMPLES_PER_CH * 2; // L R interleaved

    // Mumble's `frame_number` is counted in 10 ms units (480 samples/channel at
    // 48 kHz). A 20 ms frame therefore advances the frame number by TWO, not one.
    // Stepping by 1 made the receiver's jitter buffer see a growing sequence
    // mismatch and insert silence padding every frame — audibly "broken up".
    const SEQ_UNITS_PER_FRAME: u64 = (FRAME_SAMPLES_PER_CH as u64) / 480; // = 2 for 20 ms
    let mut frame_number: u64 = 0;
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    // Burst to catch up rather than stretch the gap: a stretched gap is what the
    // receiver hears as a dropout. Skipping keeps the timeline aligned.
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    info!(
        "stereo test: streaming {} frames of stereo tone",
        pcm.len() / interleaved_per_frame
    );

    let mut offset = 0;
    let mut send_failures: u32 = 0;
    while offset + interleaved_per_frame <= pcm.len() {
        let _ = tick.tick().await;

        let chunk = &pcm[offset..offset + interleaved_per_frame];
        offset += interleaved_per_frame;

        let frame = AudioFrame {
            data: f32_to_ne_bytes(chunk),
            format: AudioFormat::STEREO_48KHZ_F32,
            sequence: frame_number,
            is_silent: false,
        };
        let packet = encoder.encode(&frame).map_err(|e| format!("encode: {e}"))?;

        let audio = mumble_udp::Audio {
            header: Some(mumble_udp::audio::Header::Target(0)),
            sender_session: 0,
            frame_number,
            opus_data: packet.data,
            positional_data: Vec::new(),
            volume_adjustment: 0.0,
            is_terminator: false,
        };
        // Don't abort the whole test on a single transient send failure (the
        // event-loop channel briefly full). Only give up after several in a row.
        if handle.send_audio(UdpMessage::Audio(audio)).is_err() {
            send_failures += 1;
            if send_failures >= 10 {
                warn!("stereo test: aborting after 10 consecutive send failures");
                break;
            }
        } else {
            send_failures = 0;
        }
        frame_number += SEQ_UNITS_PER_FRAME;
    }

    // Terminator so the far end tears down the stream promptly.
    let term = mumble_udp::Audio {
        header: Some(mumble_udp::audio::Header::Target(0)),
        sender_session: 0,
        frame_number,
        opus_data: Vec::new(),
        positional_data: Vec::new(),
        volume_adjustment: 0.0,
        is_terminator: true,
    };
    let _ = handle.send_audio(UdpMessage::Audio(term));
    Ok(())
}

/// Decode the embedded 48 kHz stereo 16-bit WAV into interleaved f32 L R L R.
fn decode_wav_stereo_f32() -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::new(Cursor::new(STEREO_TEST_WAV))
        .map_err(|e| format!("wav open: {e}"))?;
    let spec = reader.spec();
    if spec.channels != 2 {
        return Err(format!("stereo test asset is not stereo ({}ch)", spec.channels));
    }
    // Asset is pcm_s16le at 48 kHz; convert to normalised f32.
    let samples: Result<Vec<f32>, _> = reader
        .samples::<i16>()
        .map(|s| s.map(|v| v as f32 / 32768.0))
        .collect();
    samples.map_err(|e| format!("wav decode: {e}"))
}

/// Interleaved f32 slice -> native-endian bytes (what `AudioFrame` expects).
fn f32_to_ne_bytes(samples: &[f32]) -> Vec<u8> {
    let mut bytes = vec![0u8; samples.len() * 4];
    let (chunks, _) = bytes.as_chunks_mut::<4>();
    for (chunk, s) in chunks.iter_mut().zip(samples) {
        *chunk = s.to_ne_bytes();
    }
    bytes
}
