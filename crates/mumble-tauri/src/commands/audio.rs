//! Audio device enumeration, audio settings, voice toggles, mic test,
//! latency test and recording.

use crate::audio;
use crate::state::{self, AppState, AudioDevice, AudioSettings, VoiceState};

/// Sentinel device name meaning "(No device)" — the user explicitly chose to
/// open nothing. Must match `NO_DEVICE` in the frontend (DeviceSelect.tsx).
/// `None`/empty still means "System default".
pub const NO_DEVICE: &str = "\u{0}none";

/// Returns `true` when a selected-device value is the explicit "(No device)"
/// sentinel (as opposed to `None` = system default, or a real device name).
pub fn is_no_device(name: Option<&str>) -> bool {
    name == Some(NO_DEVICE)
}

/// Sentinel 2nd-radio device meaning "generate a test tone" instead of opening
/// a real capture device. Lets an operator exercise the dual-radio right
/// channel with no second radio present. Must match `TEST_TONE_DEVICE` in the
/// frontend (AudioPanel.tsx).
pub const TEST_TONE_DEVICE: &str = "\u{1}testtone";

/// Returns `true` when the 2nd-radio selection is the test-tone sentinel.
pub fn is_test_tone(name: Option<&str>) -> bool {
    name == Some(TEST_TONE_DEVICE)
}

/// Enumerate input device display names only (no default flag). Shared by the
/// device watcher. Mirrors `get_audio_devices`' name source so the watcher's
/// view matches the dropdown's.
#[cfg(not(target_os = "android"))]
pub(crate) fn enumerate_input_device_names() -> Vec<String> {
    get_audio_devices().into_iter().map(|d| d.name).collect()
}

/// Enumerate output device display names only. Shared by the device watcher.
#[cfg(not(target_os = "android"))]
pub(crate) fn enumerate_output_device_names() -> Vec<String> {
    get_output_devices().into_iter().map(|d| d.name).collect()
}

/// List available audio input devices (microphones).
/// Only available on desktop (cpal is not supported on Android).
#[cfg(not(target_os = "android"))]
#[tauri::command]
pub(crate) fn get_audio_devices() -> Vec<AudioDevice> {
    use cpal::traits::HostTrait;

    // When the rodio backend is active (the default), enumerate inputs via
    // rodio so the names in the list exactly match what the capture open path
    // (`find_input_by_name`) compares against. This prevents the "I picked a
    // device but a different one opened" class of bug.
    if audio::is_rodio_backend() {
        let devices: Vec<AudioDevice> = audio::rodio_desktop::enumerate_input_names()
            .into_iter()
            .map(|(name, is_default)| AudioDevice { name, is_default })
            .collect();
        if !devices.is_empty() {
            return devices;
        }
        // Fall through to the cpal path if rodio enumeration came back empty.
    }

    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| friendly_device_name(&d));

    let list: Vec<AudioDevice> = host
        .input_devices()
        .map(|devices| {
            devices
                .filter_map(|d| {
                    // Prefer the friendly description name; fall back to the
                    // raw cpal name rather than DROPPING the device, so the
                    // list matches the Windows Sound applet more closely.
                    let name = friendly_device_name(&d)?;
                    Some(AudioDevice {
                        name: name.clone(),
                        is_default: default_name.as_deref() == Some(&name),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // On Linux, hide ambiguous virtual/bidirectional nodes so the mic list only
    // offers real capture devices (see `is_real_hw_device`).
    filter_real_hw_devices(list)
}

/// On Linux, drop ambiguous virtual/bidirectional nodes from a device list
/// (keeping real card devices), unless that would empty the list. No-op on
/// other platforms.
#[cfg(not(target_os = "android"))]
fn filter_real_hw_devices(list: Vec<AudioDevice>) -> Vec<AudioDevice> {
    #[cfg(target_os = "linux")]
    {
        let filtered: Vec<AudioDevice> = list
            .iter()
            .filter(|d| audio::rodio_desktop::is_real_hw_device(&d.name))
            .cloned()
            .collect();
        if !filtered.is_empty() {
            return filtered;
        }
    }
    list
}

/// Best-effort friendly name for a cpal device: the endpoint description
/// name when available, otherwise the raw device name. Returns `None` only
/// when both fail (a truly unusable device).
#[cfg(not(target_os = "android"))]
fn friendly_device_name(d: &cpal::Device) -> Option<String> {
    use cpal::traits::DeviceTrait;
    if let Ok(desc) = d.description() {
        return Some(desc.name().to_string());
    }
    // Intentional fallback to the raw (deprecated) name so a device whose
    // endpoint description is unavailable is still listed rather than dropped.
    #[allow(deprecated)]
    d.name().ok()
}

/// Stub: on Android, return an empty device list.
#[cfg(target_os = "android")]
#[tauri::command]
pub(crate) fn get_audio_devices() -> Vec<AudioDevice> {
    Vec::new()
}

/// List available audio output devices (speakers/headphones).
/// Only available on desktop (cpal is not supported on Android).
#[cfg(not(target_os = "android"))]
#[tauri::command]
pub(crate) fn get_output_devices() -> Vec<AudioDevice> {
    use cpal::traits::HostTrait;

    let host = cpal::default_host();
    let default_name = host
        .default_output_device()
        .and_then(|d| friendly_device_name(&d));

    let list: Vec<AudioDevice> = host
        .output_devices()
        .map(|devices| {
            devices
                .filter_map(|d| {
                    // Fall back to the raw cpal name rather than dropping the
                    // device when the endpoint description is unavailable.
                    let name = friendly_device_name(&d)?;
                    Some(AudioDevice {
                        name: name.clone(),
                        is_default: default_name.as_deref() == Some(&name),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // On Linux, hide ambiguous virtual/bidirectional nodes (see
    // `is_real_hw_device`) so the speaker list only offers real output devices,
    // keeping it distinct from the mic list.
    filter_real_hw_devices(list)
}

/// Stub: on Android, return an empty device list.
#[cfg(target_os = "android")]
#[tauri::command]
pub(crate) fn get_output_devices() -> Vec<AudioDevice> {
    Vec::new()
}

/// Get current audio settings.
///
/// When `server_id` is supplied, returns that specific server's settings;
/// otherwise returns the active session's (which is the global default
/// while nothing is connected).
#[tauri::command]
pub(crate) fn get_audio_settings(
    state: tauri::State<'_, AppState>,
    server_id: Option<state::ServerId>,
) -> AudioSettings {
    state.audio_settings_for(server_id)
}

/// List the tunable knobs the given denoiser algorithm exposes.
///
/// The UI uses this to render dynamic sliders in advanced/expert
/// mode without hard-coding the knob list per algorithm.
#[tauri::command]
pub(crate) fn get_denoiser_param_specs(
    algorithm: mumble_protocol::audio::filter::denoiser::NoiseSuppressionAlgorithm,
) -> Vec<mumble_protocol::audio::filter::denoiser::DenoiserParamSpec> {
    mumble_protocol::audio::filter::denoiser::algorithm_param_specs(algorithm).to_vec()
}

/// List the noise-suppression algorithms whose backends are actually
/// compiled into this build.
#[tauri::command]
pub(crate) fn get_available_denoiser_algorithms()
 -> Vec<mumble_protocol::audio::filter::denoiser::NoiseSuppressionAlgorithm>
{
    mumble_protocol::audio::filter::denoiser::NoiseSuppressionAlgorithm::available()
}

/// Update audio settings.
///
/// If any pipeline-relevant setting changes while voice is active, the
/// capture/playback pipelines are automatically restarted as needed.
#[tauri::command]
pub(crate) async fn set_audio_settings(
    state: tauri::State<'_, AppState>,
    settings: AudioSettings,
    server_id: Option<state::ServerId>,
) -> Result<(), String> {
    let force_tcp = settings.force_tcp_audio;
    let (needs_outbound, needs_inbound, force_tcp_changed, mode_changed) = state
        .set_audio_settings_for(server_id, settings)
        .unwrap_or((false, false, false, false));

    // A transmit-mode change reconciles the live voice state to the new mode:
    // PTT closes the mic (muted until keyed); VOX/Continuous opens it. This is
    // the single path for VOX<->PTT<->Continuous and both rebuilds the pipeline
    // and fixes the self-mute, so neither direction leaves the mic stuck.
    if mode_changed {
        state.reconcile_voice_for_mode().await?;
    } else if needs_outbound {
        state.restart_outbound()?;
    }
    if needs_inbound {
        state.restart_inbound()?;
    }
    if force_tcp_changed {
        if let Ok(inner) = state.inner.snapshot().lock() {
            if let Some(ref handle) = inner.conn.client_handle {
                handle.set_force_tcp(force_tcp);
            }
        }
    }

    Ok(())
}

/// Switch the desktop audio backend at runtime.
///
/// `true` selects the rodio backend (default), `false` selects the
/// legacy cpal backend. Takes effect on the next voice toggle.
#[tauri::command]
pub(crate) fn set_audio_backend(use_rodio: bool) {
    audio::set_use_rodio_backend(use_rodio);
}

/// Subscribe to oscilloscope sample frames for a source ("input" or
/// "output"). Ref-counted: the backend only taps + emits `scope-data` while
/// at least one scope is subscribed, so it is free when nothing is shown.
#[tauri::command]
pub(crate) fn scope_subscribe(source: String, id: u64) {
    if let Some(src) = audio::scope::ScopeSource::parse(&source) {
        audio::scope::subscribe(src, id);
    }
}

/// Unsubscribe a previously subscribed scope source.
#[tauri::command]
pub(crate) fn scope_unsubscribe(source: String, id: u64) {
    if let Some(src) = audio::scope::ScopeSource::parse(&source) {
        audio::scope::unsubscribe(src, id);
    }
}

/// Returns `true` if the rodio backend is selected, `false` for legacy cpal.
#[tauri::command]
pub(crate) fn get_audio_backend() -> bool {
    audio::is_rodio_backend()
}

/// Get the current voice state.
#[tauri::command]
pub(crate) fn get_voice_state(state: tauri::State<'_, AppState>) -> VoiceState {
    state.voice_state()
}

/// Transmit the embedded stereo test tone to the server (pauses the mic for
/// the duration). The far operator hears left/all/right separation on the air.
#[tauri::command]
pub(crate) fn play_stereo_test(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.play_stereo_test()
}

/// Stop a running stereo test and resume the mic if it had been paused.
#[tauri::command]
pub(crate) fn stop_stereo_test(state: tauri::State<'_, AppState>) {
    state.stop_stereo_test();
}

/// Enable voice calling for the current channel.
/// Sends unmute/undeaf to the server.
#[tauri::command]
pub(crate) async fn enable_voice(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.enable_voice().await
}

/// Enable voice calling directly in the muted state (undeaf + mute, inbound
/// only). Used by the reconnect voice-restore so the muted state is established
/// in one atomic step instead of `enable_voice` then `toggle_mute`, which can
/// race on a reconnect and leave the user unmuted.
#[tauri::command]
pub(crate) async fn enable_voice_muted(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.enable_voice_muted().await
}

/// Disable voice calling (go back to deaf+muted).
#[tauri::command]
pub(crate) async fn disable_voice(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.disable_voice().await
}

/// Toggle mute (mic on/off, still hearing).
#[tauri::command]
pub(crate) async fn toggle_mute(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.toggle_mute().await
}

/// Toggle deafen (all audio on/off).
#[tauri::command]
pub(crate) async fn toggle_deafen(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.toggle_deafen().await
}

/// Activate mic for push-to-talk (key pressed).
#[tauri::command]
pub(crate) async fn push_to_talk_start(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.push_to_talk_start().await
}

/// Deactivate mic for push-to-talk (key released).
#[tauri::command]
pub(crate) async fn push_to_talk_end(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.push_to_talk_end().await
}

/// Activate mic for voice priority override (key pressed).
///
/// Behaves identically to push-to-talk for now; voice-target routing
/// will be added in a future iteration.
#[tauri::command]
pub(crate) async fn voice_priority_start(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.push_to_talk_start().await
}

/// Deactivate mic for voice priority override (key released).
#[tauri::command]
pub(crate) async fn voice_priority_end(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.push_to_talk_end().await
}

/// Set the local playback volume for a specific remote user.
///
/// `volume` is a multiplier (0.0 = muted, 1.0 = normal, 2.0 = 200%).
#[tauri::command]
pub(crate) fn set_user_volume(state: tauri::State<'_, AppState>, session: u32, volume: f32) {
    state.set_user_volume(session, volume);
}

/// Start monitoring the microphone and emitting amplitude events.
#[tauri::command]
pub(crate) fn start_mic_test(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.start_mic_test()
}

/// Stop monitoring the microphone.
#[tauri::command]
pub(crate) fn stop_mic_test(state: tauri::State<'_, AppState>) {
    state.stop_mic_test();
}

/// Calibrate the voice activation threshold by measuring the ambient
/// noise floor for ~2 seconds (with AGC applied).  Returns the new threshold.
#[tauri::command]
pub(crate) async fn calibrate_voice_threshold(state: tauri::State<'_, AppState>) -> Result<f32, String> {
    state.calibrate_voice_threshold().await
}

/// Begin a voice replay session: record up to ~20 s of microphone
/// audio through the live outbound filter chain, then play it back
/// through the output device.
///
/// Lifecycle events are emitted on `voice-replay-state`.
#[tauri::command]
pub(crate) fn start_voice_replay(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.start_voice_replay()
}

/// Advance the voice replay state machine - stops recording (which
/// auto-transitions into playback), or stops playback entirely.
#[tauri::command]
pub(crate) fn stop_voice_replay(state: tauri::State<'_, AppState>) {
    state.stop_voice_replay();
}

/// Start periodic TCP pings for live latency measurement.
#[tauri::command]
pub(crate) fn start_latency_test(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.start_latency_test()
}

/// Stop the latency test.
#[tauri::command]
pub(crate) fn stop_latency_test(state: tauri::State<'_, AppState>) {
    state.stop_latency_test();
}

/// Start recording inbound audio to a file.
#[tauri::command]
pub(crate) fn start_recording(
    state: tauri::State<'_, AppState>,
    directory: String,
    filename: String,
    format: state::recording::RecordingFormat,
) -> Result<String, String> {
    state.start_recording(directory, filename, format)
}

/// Stop the current recording and finalize the file.
#[tauri::command]
pub(crate) fn stop_recording(state: tauri::State<'_, AppState>) -> Result<String, String> {
    state.stop_recording()
}

/// Get the current recording state.
#[tauri::command]
pub(crate) fn get_recording_state(state: tauri::State<'_, AppState>) -> state::recording::RecordingState {
    state.recording_state()
}
