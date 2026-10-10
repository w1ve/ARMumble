//! Audio / voice pipeline management: enable, disable, mute, deafen,
//! and the background outbound audio loop.
//!
//! Platform-specific audio I/O is abstracted behind factory functions
//! (`create_capture` / `create_playback`) that return trait objects.
//! On desktop, cpal is used; on Android, oboe.

use tauri::Emitter;

use super::types::VoiceState;
use super::AppState;

// -- Shared methods (all platforms) --------------------------------

impl AppState {
    /// Resolve the per-session [`SharedState`] a settings command should
    /// target.  `Some(id)` selects that specific server from the registry;
    /// `None` (or an unknown id) falls back to the currently-active session.
    ///
    /// When no server is connected, the active snapshot is `default_inner`,
    /// which is exactly the "global default" audio settings that seed every
    /// new session via [`AppState::fresh_session_state`] - so editing audio
    /// settings while disconnected edits those defaults, which is intended.
    fn resolve_audio_target(
        &self,
        server_id: Option<super::ServerId>,
    ) -> std::sync::Arc<std::sync::Mutex<super::SharedState>> {
        server_id
            .and_then(|id| self.registry.session(id))
            .unwrap_or_else(|| self.inner.snapshot())
    }

    /// Build a fresh playback [`StereoControl`] initialised from `settings`.
    fn make_stereo_control(
        settings: &super::types::AudioSettings,
    ) -> std::sync::Arc<mumble_protocol::audio::mixer::StereoControl> {
        let ctrl = mumble_protocol::audio::mixer::StereoControl::default();
        ctrl.set(
            Self::stereo_out_mode(settings.playback_stereo_mode),
            settings.left_volume,
            settings.right_volume,
        );
        std::sync::Arc::new(ctrl)
    }

    /// Map the settings-layer [`StereoMode`](super::types::StereoMode) onto the
    /// mixer's [`StereoOutMode`](mumble_protocol::audio::mixer::StereoOutMode).
    fn stereo_out_mode(
        mode: super::types::StereoMode,
    ) -> mumble_protocol::audio::mixer::StereoOutMode {
        use mumble_protocol::audio::mixer::StereoOutMode;
        match mode {
            super::types::StereoMode::Stereo => StereoOutMode::Stereo,
            super::types::StereoMode::Mono => StereoOutMode::Mono,
            super::types::StereoMode::SwapLr => StereoOutMode::SwapLr,
        }
    }

    /// Get the audio settings for a specific server (or the active session
    /// when `server_id` is `None`).
    pub fn audio_settings_for(
        &self,
        server_id: Option<super::ServerId>,
    ) -> super::types::AudioSettings {
        self.resolve_audio_target(server_id)
            .lock()
            .map(|s| s.audio.settings.clone())
            .unwrap_or_default()
    }

    /// Update audio settings for the active session.
    ///
    /// If any pipeline-relevant setting changed while voice is active,
    /// the outbound pipeline is automatically restarted.
    /// Volume changes are applied live via atomic handles (no restart).
    pub fn set_audio_settings(
        &self,
        settings: super::types::AudioSettings,
    ) -> Option<(bool, bool, bool, bool)> {
        self.set_audio_settings_for(None, settings)
    }

    /// Update the audio settings for a specific server (or the active
    /// session when `server_id` is `None`).  Returns which pipelines need
    /// restarting; the caller restarts only the active session's pipeline,
    /// so a pipeline restart is reported only when the target *is* the
    /// active session.
    pub fn set_audio_settings_for(
        &self,
        server_id: Option<super::ServerId>,
        settings: super::types::AudioSettings,
    ) -> Option<(bool, bool, bool, bool)> {
        let target = self.resolve_audio_target(server_id);
        // A pipeline restart only makes sense for the session the live
        // audio loop is bound to - the active one.  Writing settings to a
        // background (non-active) session must never restart audio.
        let is_active = std::sync::Arc::ptr_eq(&target, &self.inner.snapshot());

        let (old_settings, voice_active) = {
            let state = target.lock().ok()?;
            (
                state.audio.settings.clone(),
                state.audio.voice_state == VoiceState::Active,
            )
        };

        // A transmit-mode change needs the voice state re-reconciled to the new
        // mode (PTT -> mic closed/Muted; VOX/Continuous -> mic open/Active),
        // which the caller does asynchronously via `reconcile_voice_for_mode`.
        // When the mode changed we skip the ordinary outbound restart here and
        // let the reconciler rebuild the pipeline in the correct state, so the
        // two paths never fight (the old flag-based path left the mic stuck
        // open on VOX->PTT and stuck muted on PTT->VOX).
        let mode_changed =
            is_active && old_settings.transmit_mode() != settings.transmit_mode();

        let restart_outbound = is_active
            && voice_active
            && !mode_changed
            && old_settings.needs_pipeline_restart(&settings);
        let restart_inbound =
            is_active && voice_active && old_settings.needs_inbound_restart(&settings);
        let force_tcp_changed =
            is_active && old_settings.force_tcp_audio != settings.force_tcp_audio;

        // Update live volume + stereo handles (no pipeline restart needed).
        if let Ok(state) = target.lock() {
            use std::sync::atomic::Ordering;
            if let Some(ref h) = state.audio.input_volume_handle {
                h.store(settings.input_volume.to_bits(), Ordering::Relaxed);
            }
            if let Some(ref h) = state.audio.output_volume_handle {
                h.store(settings.output_volume.to_bits(), Ordering::Relaxed);
            }
            if let Some(ref sc) = state.audio.stereo_control {
                sc.set(
                    Self::stereo_out_mode(settings.playback_stereo_mode),
                    settings.left_volume,
                    settings.right_volume,
                );
            }
            // Live-switch the dual-radio right-channel source (no restart).
            if let Some(ref rs) = state.audio.right_source_control {
                let v = match settings.right_channel_source {
                    crate::state::types::RightChannelSource::Radio2 => {
                        crate::audio::rodio_desktop::RIGHT_SOURCE_RADIO2
                    }
                    crate::state::types::RightChannelSource::Radio1 => {
                        crate::audio::rodio_desktop::RIGHT_SOURCE_RADIO1
                    }
                };
                rs.store(v, Ordering::Relaxed);
            }
        }

        if let Ok(mut state) = target.lock() {
            state.audio.settings = settings;
        }

        Some((restart_outbound, restart_inbound, force_tcp_changed, mode_changed))
    }

    /// React to a change in the set of available audio devices (hotplug).
    ///
    /// If the active session's currently-selected input or output device is a
    /// real device name that is no longer present, fall back to "(No device)"
    /// and reset the affected chain, so a yanked USB sound card doesn't leave
    /// the pipeline pointed at a dead endpoint. A fresh selection by the user
    /// rebuilds the chain through the normal `set_audio_settings` path.
    ///
    /// `inputs` / `outputs` are the current device-name sets.
    pub fn handle_device_set_changed(&self, inputs: &[String], outputs: &[String]) {
        let target = self.inner.snapshot();

        // Read the active selection + whether voice is live.
        let (selected_in, selected_out, voice_active) = {
            let Ok(state) = target.lock() else { return };
            (
                state.audio.settings.selected_device.clone(),
                state.audio.settings.selected_output_device.clone(),
                state.audio.voice_state == VoiceState::Active,
            )
        };

        // A selection is "dead" when it names a real device (not None = system
        // default, not the (No device) sentinel) that is absent from the set.
        let input_dead = selected_in.as_deref().is_some_and(|name| {
            !crate::commands::audio::is_no_device(Some(name))
                && !inputs.iter().any(|d| d == name)
        });
        let output_dead = selected_out.as_deref().is_some_and(|name| {
            !crate::commands::audio::is_no_device(Some(name))
                && !outputs.iter().any(|d| d == name)
        });

        if !input_dead && !output_dead {
            return;
        }

        // Flip the dead selection(s) to (No device) and persist.
        {
            let Ok(mut state) = target.lock() else { return };
            if input_dead {
                tracing::warn!(
                    "input device '{}' disappeared; falling back to (No device)",
                    selected_in.as_deref().unwrap_or_default()
                );
                state.audio.settings.selected_device =
                    Some(crate::commands::audio::NO_DEVICE.to_string());
            }
            if output_dead {
                tracing::warn!(
                    "output device '{}' disappeared; falling back to (No device)",
                    selected_out.as_deref().unwrap_or_default()
                );
                state.audio.settings.selected_output_device =
                    Some(crate::commands::audio::NO_DEVICE.to_string());
            }
        }

        // Reset the affected chain(s) if voice is live.
        if voice_active {
            if input_dead {
                let _ = self.restart_outbound();
            }
            if output_dead {
                let _ = self.restart_inbound();
            }
        }
    }

    /// Get the rig-PTT config for a specific server (or the active session when
    /// `server_id` is `None`).
    pub fn ptt_config_for(
        &self,
        server_id: Option<super::ServerId>,
    ) -> super::types::PttConfig {
        self.resolve_audio_target(server_id)
            .lock()
            .map(|s| s.audio.ptt_config.clone())
            .unwrap_or_default()
    }

    /// Set the rig-PTT config for a specific server (or the active session when
    /// `server_id` is `None`). Returns whether the target is the active session
    /// (the caller restarts the live PTT controller only for the active one).
    pub fn set_ptt_config_for(
        &self,
        server_id: Option<super::ServerId>,
        config: super::types::PttConfig,
    ) -> bool {
        let target = self.resolve_audio_target(server_id);
        let is_active = std::sync::Arc::ptr_eq(&target, &self.inner.snapshot());
        if let Ok(mut state) = target.lock() {
            state.audio.ptt_config = config;
        }
        is_active
    }

    /// Get current voice state.
    pub fn voice_state(&self) -> VoiceState {
        self.inner.snapshot()
            .lock()
            .map(|s| s.audio.voice_state)
            .unwrap_or_default()
    }

    /// Emit voice-state-changed event to the frontend.
    pub(super) fn emit_voice_state(&self) {
        if let Some(app) = self.app_handle() {
            let vs = self.voice_state();
            let _ = app.emit("voice-state-changed", vs);
        }
    }

    /// Emit `whisper-active` so the frontend can turn the target channel's
    /// megaphone icon green while the operator is whispering, and clear it on
    /// release. Payload: `(active, target_channel_or_null)`.
    pub(crate) fn emit_whisper_active(&self, active: bool, channel: Option<u32>) {
        if let Some(app) = self.app_handle() {
            let _ = app.emit("whisper-active", (active, channel));
        }
    }

    /// Set the local playback volume for a specific remote user.
    ///
    /// `volume` is a multiplier (0.0 = muted, 1.0 = normal, 2.0 = 200%).
    pub fn set_user_volume(&self, session: u32, volume: f32) {
        // Guard against NaN: `f32::clamp` propagates NaN, which would poison the
        // mixer (every `sample * NaN` becomes NaN) and silently break this
        // user's audio. Drop the update instead.
        if volume.is_nan() {
            return;
        }
        if let Ok(state) = self.inner.snapshot().lock() {
            if let Ok(mut sv) = state.audio.speaker_volumes.lock() {
                if (volume - 1.0).abs() < f32::EPSILON {
                    let _ = sv.remove(&session);
                } else {
                    let _ = sv.insert(session, volume.clamp(0.0, 2.0));
                }
            }
        }
    }
}

// -- Voice pipeline (all platforms) ---------------------------------

mod voice_pipeline {
    use std::sync::atomic::AtomicU32;
    use std::sync::Arc;
    use std::time::Duration;

    use tauri::Emitter;
    use tracing::info;

    use mumble_protocol::audio::encoder::{ForceChannels, OpusEncoder, OpusEncoderConfig};
    use mumble_protocol::audio::filter::automatic_gain::{AgcConfig, AutomaticGainControl};
    use mumble_protocol::audio::filter::denoiser::{DenoiserConfig, SpectralDenoiser};
    use mumble_protocol::audio::filter::noise_gate::{NoiseGate, NoiseGateConfig};
    use mumble_protocol::audio::filter::FilterChain;
    use mumble_protocol::audio::mixer::{AudioMixer, SpeakerBuffers};
    use mumble_protocol::audio::pipeline::OutboundPipeline;
    use mumble_protocol::audio::sample::AudioFormat;
    use mumble_protocol::client::ClientHandle;
    use mumble_protocol::command;

    use crate::audio::{AudioDeviceFactory, PlatformAudioFactory};

    use crate::state::types::{AudioSettings, TransmitMode, VoiceState};
    use crate::state::AppState;

    impl AppState {
        /// Enable voice calling: unmute + undeaf, start audio pipelines.
        ///
        /// Push-to-Talk is mutually exclusive with an open mic: in PTT mode the
        /// transmitter is keyed only while the key is held, so enabling voice
        /// must land in **Muted** (inbound only, self-mute asserted), exactly
        /// like `enable_voice_muted`. `push_to_talk_start`/`_end` (and the
        /// operator rig hotkey) then open/close the outbound pipeline. Starting
        /// Active here would leave the mic permanently live, since PTT inserts
        /// no voice gate - that is the VOX/PTT bleed-through bug.
        pub async fn enable_voice(&self) -> Result<(), String> {
            let (handle, audio_settings) = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                (state.conn.client_handle.clone(), state.audio.settings.clone())
            };

            if audio_settings.transmit_mode() == TransmitMode::PushToTalk {
                info!("enable_voice: push-to-talk mode - entering muted (mic off until keyed)");
                return self.enable_voice_muted().await;
            }

            info!("enable_voice: starting audio pipelines");

            // Create shared volume handles for live updates.
            let input_vol = Arc::new(AtomicU32::new(audio_settings.input_volume.to_bits()));
            let output_vol = Arc::new(AtomicU32::new(audio_settings.output_volume.to_bits()));

            // Inbound: per-speaker decoders + mixing playback.
            let speaker_buffers: SpeakerBuffers = Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            );
            let speaker_volumes = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                state.audio.speaker_volumes.clone()
            };
            // Inbound is stereo so dual-receiver radio audio stays separated; a
    // stereo decoder upmixes mono senders, so mono traffic is unaffected.
    let mixer = AudioMixer::new(speaker_buffers.clone(), AudioFormat::STEREO_48KHZ_F32);
            let stereo_ctrl = Self::make_stereo_control(&audio_settings);
            let mut mixing_playback = PlatformAudioFactory::create_mixing_playback(
                audio_settings.selected_output_device.as_deref(),
                output_vol.clone(),
                speaker_buffers,
                speaker_volumes,
                stereo_ctrl.clone(),
            )?;
            mixing_playback
                .start()
                .map_err(|e| format!("Playback start: {e}"))?;

            {
                let __session = self.inner.snapshot();
                let mut state = __session.lock().map_err(|e| e.to_string())?;
                state.audio.mixer = Some(mixer);
                state.audio.mixing_playback = Some(mixing_playback);
                state.audio.input_volume_handle = Some(input_vol);
                state.audio.output_volume_handle = Some(output_vol);
                state.audio.stereo_control = Some(stereo_ctrl);
            }

            // Outbound: capture -> filters -> encoder -> network.
            self.start_outbound_pipeline(&audio_settings, &handle)?;

            {
                let __session = self.inner.snapshot();
                let mut state = __session.lock().map_err(|e| e.to_string())?;
                state.audio.voice_state = VoiceState::Active;
            }

            info!("enable_voice: pipelines started, sending unmute");

            // SetSelfMute{false} already carries self_deaf=false (undeafen).
            if let Some(handle) = handle {
                handle
                    .send(command::SetSelfMute { muted: false })
                    .await
                    .map_err(|e| format!("Failed to unmute: {e}"))?;
            }

            self.emit_voice_state();
            Ok(())
        }

        /// Disable voice calling: go back to deaf + muted, stop pipelines.
        pub async fn disable_voice(&self) -> Result<(), String> {
            self.stop_audio();

            let handle = {
                let __session = self.inner.snapshot();
                let mut state = __session.lock().map_err(|e| e.to_string())?;
                state.audio.voice_state = VoiceState::Inactive;
                state.conn.client_handle.clone()
            };

            // SetSelfDeaf{true} already carries self_mute=true (deaf implies mute).
            if let Some(handle) = handle {
                handle
                    .send(command::SetSelfDeaf { deafened: true })
                    .await
                    .map_err(|e| format!("Failed to deafen: {e}"))?;
            }

            self.emit_voice_state();
            Ok(())
        }

        /// Stop all running audio pipelines and tasks.
        pub(in crate::state) fn stop_audio(&self) {
            self.stop_audio_on(&self.inner.snapshot());
        }

        /// Stop all running audio pipelines and tasks for a specific
        /// session arc.  Used during multi-server active-session swaps
        /// so the previous session's pipeline can be torn down even
        /// after `inner` has already been swapped to a new session.
        pub(in crate::state) fn stop_audio_on(
            &self,
            arc: &Arc<std::sync::Mutex<crate::state::SharedState>>,
        ) {
            let stopped_sessions: Vec<(u32, tauri::AppHandle)> = if let Ok(mut state) = arc.lock() {
                if let Some(handle) = state.audio.outbound_task_handle.take() {
                    handle.abort();
                }
                if let Some(handle) = state.audio.mic_test_handle.take() {
                    handle.abort();
                }
                if let Some(handle) = state.audio.latency_test_handle.take() {
                    handle.abort();
                }
                if let Some(mut playback) = state.audio.mixing_playback.take() {
                    let _ = playback.stop();
                }
                state.audio.mixer = None;
                state.audio.input_volume_handle = None;
                state.audio.output_volume_handle = None;
                state.audio.stereo_control = None;

                // Collect sessions to notify OUTSIDE the lock.
                let sessions: Vec<(u32, tauri::AppHandle)> = state
                    .conn.tauri_app_handle
                    .as_ref()
                    .filter(|_| !state.audio.talking_sessions.is_empty())
                    .map(|app| state.audio.talking_sessions.iter().map(|&s| (s, app.clone())).collect())
                    .unwrap_or_default();
                state.audio.talking_sessions.clear();
                sessions
            } else {
                Vec::new()
            };

            // Emit outside the lock to avoid deadlock with Tauri IPC.
            for (session, app) in stopped_sessions {
                let _ = app.emit("user-talking", (session, false));
            }
        }

        /// Stop only the outbound (mic capture) pipeline.
        pub(in crate::state) fn stop_outbound(&self) {
            if let Ok(mut state) = self.inner.snapshot().lock() {
                if let Some(handle) = state.audio.outbound_task_handle.take() {
                    handle.abort();
                }
            }
        }

        /// Restart the outbound pipeline with the current audio settings.
        ///
        /// Called when the input device (or other capture-relevant settings)
        /// change while voice is active.
        pub fn restart_outbound(&self) -> Result<(), String> {
            info!("restart_outbound: restarting capture pipeline with new settings");
            self.stop_outbound();

            let (audio_settings, client_handle) = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                (state.audio.settings.clone(), state.conn.client_handle.clone())
            };

            // "(No device)" selected: leave the capture pipeline torn down so
            // nothing is transmitted. The chain rebuilds when a real device is
            // chosen (which triggers needs_pipeline_restart again).
            if crate::commands::audio::is_no_device(audio_settings.selected_device.as_deref()) {
                info!("restart_outbound: input is (No device); capture left stopped");
                return Ok(());
            }

            self.start_outbound_pipeline(&audio_settings, &client_handle)
        }

        /// Restart the inbound pipeline with the current audio settings.
        ///
        /// Called when the output device changes while voice is active.
        pub fn restart_inbound(&self) -> Result<(), String> {
            info!("restart_inbound: restarting playback pipeline with new settings");

            // Stop the old inbound pipeline.
            if let Ok(mut state) = self.inner.snapshot().lock() {
                if let Some(mut playback) = state.audio.mixing_playback.take() {
                    let _ = playback.stop();
                }
                state.audio.mixer = None;
            }

            let audio_settings = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                state.audio.settings.clone()
            };

            // "(No device)" selected: leave playback torn down so nothing is
            // output. Rebuilds when a real output device is chosen.
            if crate::commands::audio::is_no_device(
                audio_settings.selected_output_device.as_deref(),
            ) {
                info!("restart_inbound: output is (No device); playback left stopped");
                return Ok(());
            }

            let output_vol = Arc::new(AtomicU32::new(audio_settings.output_volume.to_bits()));

            let speaker_buffers: SpeakerBuffers = Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            );
            let speaker_volumes = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                state.audio.speaker_volumes.clone()
            };
            // Inbound is stereo so dual-receiver radio audio stays separated; a
    // stereo decoder upmixes mono senders, so mono traffic is unaffected.
    let mixer = AudioMixer::new(speaker_buffers.clone(), AudioFormat::STEREO_48KHZ_F32);
            let stereo_ctrl = Self::make_stereo_control(&audio_settings);
            let mut mixing_playback = PlatformAudioFactory::create_mixing_playback(
                audio_settings.selected_output_device.as_deref(),
                output_vol.clone(),
                speaker_buffers,
                speaker_volumes,
                stereo_ctrl.clone(),
            )?;
            mixing_playback
                .start()
                .map_err(|e| format!("Playback start: {e}"))?;

            let __session = self.inner.snapshot();

            let mut state = __session.lock().map_err(|e| e.to_string())?;
            state.audio.mixer = Some(mixer);
            state.audio.mixing_playback = Some(mixing_playback);
            state.audio.output_volume_handle = Some(output_vol);
            state.audio.stereo_control = Some(stereo_ctrl);
            Ok(())
        }

        /// Create and start a fresh outbound audio pipeline (mic -> encoder -> network).
        fn start_outbound_pipeline(
            &self,
            audio_settings: &AudioSettings,
            client_handle: &Option<ClientHandle>,
        ) -> Result<(), String> {
            // "(No device)" means transmit nothing: skip opening any capture.
            if crate::commands::audio::is_no_device(audio_settings.selected_device.as_deref()) {
                info!("start_outbound_pipeline: input is (No device); capture not started");
                return Ok(());
            }
            // Re-use existing input volume handle or create a new one.
            let (input_vol, app, own_session, max_bandwidth) = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                (
                    state.audio.input_volume_handle.clone(),
                    state.conn.tauri_app_handle.clone(),
                    state.conn.own_session,
                    state.server.max_bandwidth,
                )
            };
            let input_vol =
                input_vol.unwrap_or_else(|| Arc::new(AtomicU32::new(audio_settings.input_volume.to_bits())));

            let (bitrate_bps, frame_size_ms) = adjust_to_server_bandwidth(
                audio_settings.bitrate_bps,
                audio_settings.frame_size_ms,
                max_bandwidth,
            );
            if (bitrate_bps, frame_size_ms)
                != (audio_settings.bitrate_bps, audio_settings.frame_size_ms)
            {
                info!(
                    "outbound audio clamped to server bandwidth limit {:?} bps: \
                     {} bps @ {} ms (configured: {} bps @ {} ms)",
                    max_bandwidth,
                    bitrate_bps,
                    frame_size_ms,
                    audio_settings.bitrate_bps,
                    audio_settings.frame_size_ms,
                );
            }
            let frame_size_samples = AudioSettings::frame_ms_to_samples(frame_size_ms);

            // A radio Station transmits STEREO so a dual-receiver rig (different
            // receivers on L and R) keeps its channel separation on the air. An
            // operator is a single voice mic and stays mono.
            let station_stereo =
                self.ptt_config_for(None).role == crate::state::types::PttRole::Station;

            // Dual-radio Station: when a 2nd radio input is configured, open
            // both devices and build one stereo stream whose right channel is
            // switchable live (Radio 1 vs Radio 2). Store the shared atomic so
            // set_audio_settings_for can flip it without a pipeline restart.
            let dual_radio = station_stereo
                && audio_settings
                    .selected_device_2
                    .as_deref()
                    .is_some_and(|s| !s.is_empty()
                        && !crate::commands::audio::is_no_device(Some(s)));
            let capture = if dual_radio {
                let right_source = Arc::new(std::sync::atomic::AtomicU8::new(
                    match audio_settings.right_channel_source {
                        crate::state::types::RightChannelSource::Radio2 => {
                            crate::audio::rodio_desktop::RIGHT_SOURCE_RADIO2
                        }
                        crate::state::types::RightChannelSource::Radio1 => {
                            crate::audio::rodio_desktop::RIGHT_SOURCE_RADIO1
                        }
                    },
                ));
                if let Ok(mut state) = self.inner.snapshot().lock() {
                    state.audio.right_source_control = Some(right_source.clone());
                }
                PlatformAudioFactory::create_capture_dual(
                    audio_settings.selected_device.as_deref(),
                    audio_settings.selected_device_2.as_deref(),
                    frame_size_samples,
                    input_vol.clone(),
                    right_source,
                )?
            } else {
                if let Ok(mut state) = self.inner.snapshot().lock() {
                    state.audio.right_source_control = None;
                }
                PlatformAudioFactory::create_capture_channels(
                    audio_settings.selected_device.as_deref(),
                    frame_size_samples,
                    input_vol.clone(),
                    station_stereo,
                )?
            };
            // The capture may have fallen back to mono (e.g. a mono sound card),
            // so build the encoder to match the capture's actual channel count.
            let capture_format = capture.format();
            let is_stereo = capture_format.channels >= 2;
            if station_stereo {
                info!(
                    "outbound: Station stereo requested; capture is {} channel(s)",
                    capture_format.channels
                );
            }

            let encoder_config = OpusEncoderConfig {
                bitrate: bitrate_bps,
                frame_size: frame_size_samples,
                // Pin two coded channels so Opus can't collapse a near-silent
                // receiver into the other and make it vanish; keep phase so the
                // two receivers stay independent.
                force_channels: if is_stereo {
                    ForceChannels::Stereo
                } else {
                    ForceChannels::Auto
                },
                phase_inversion_disabled: is_stereo,
                ..OpusEncoderConfig::default()
            };
            let encoder = OpusEncoder::new(encoder_config, capture_format)
                .map_err(|e| format!("Encoder init: {e}"))?;

            // Build the DSP chain. For a stereo Station, skip filters that assume
            // a mono stream (they would corrupt interleaved L/R); the gate is
            // already absent in Continuous mode which a Station uses.
            let outbound_filters = if is_stereo {
                build_outbound_filters_stereo(audio_settings)
            } else {
                build_outbound_filters(audio_settings)
            };

            info!(
                "start_outbound_pipeline: filters={}, noise_suppression={}, threshold={:.5}",
                outbound_filters.len(),
                audio_settings.noise_suppression,
                audio_settings.vad_threshold,
            );

            let outbound = OutboundPipeline::new(
                capture,
                outbound_filters,
                Box::new(encoder),
            );
            // capture.start() is deferred to outbound_audio_loop so the
            // cpal stream only begins producing samples once the encoding
            // loop is ready to consume them, avoiding buffer overflow on
            // startup.

            let outbound_handle = if let Some(ref client) = client_handle {
                let client = client.clone();
                let app = app.clone();
                // Share the live VoiceTarget id so the Whisper hotkey can
                // redirect this pipeline's packets to another channel.
                let target = self.outbound_target.clone();
                Some(tokio::spawn(async move {
                    outbound_audio_loop(outbound, client, app, own_session, target).await;
                }))
            } else {
                None
            };

            let __session = self.inner.snapshot();

            let mut state = __session.lock().map_err(|e| e.to_string())?;
            state.audio.outbound_task_handle = outbound_handle;
            state.audio.input_volume_handle = Some(input_vol);
            Ok(())
        }

        /// Enable voice in muted state: start inbound pipeline (hearing)
        /// but keep outbound stopped (mic off).
        ///
        /// Used when undeafening from Inactive to land in Muted state.
        pub async fn enable_voice_muted(&self) -> Result<(), String> {
            let (handle, audio_settings) = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                (state.conn.client_handle.clone(), state.audio.settings.clone())
            };

            info!("enable_voice_muted: starting inbound pipeline only");

            let output_vol = Arc::new(AtomicU32::new(audio_settings.output_volume.to_bits()));

            let speaker_buffers: SpeakerBuffers = Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            );
            let speaker_volumes = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                state.audio.speaker_volumes.clone()
            };
            // Inbound is stereo so dual-receiver radio audio stays separated; a
    // stereo decoder upmixes mono senders, so mono traffic is unaffected.
    let mixer = AudioMixer::new(speaker_buffers.clone(), AudioFormat::STEREO_48KHZ_F32);
            let stereo_ctrl = Self::make_stereo_control(&audio_settings);
            let mut mixing_playback = PlatformAudioFactory::create_mixing_playback(
                audio_settings.selected_output_device.as_deref(),
                output_vol.clone(),
                speaker_buffers,
                speaker_volumes,
                stereo_ctrl.clone(),
            )?;
            mixing_playback
                .start()
                .map_err(|e| format!("Playback start: {e}"))?;

            {
                let __session = self.inner.snapshot();
                let mut state = __session.lock().map_err(|e| e.to_string())?;
                state.audio.voice_state = VoiceState::Muted;
                state.audio.mixer = Some(mixer);
                state.audio.mixing_playback = Some(mixing_playback);
                state.audio.output_volume_handle = Some(output_vol);
                state.audio.stereo_control = Some(stereo_ctrl);
            }

            info!("enable_voice_muted: inbound started, sending undeafen");

            if let Some(handle) = handle {
                handle
                    .send(command::SetSelfDeaf { deafened: false })
                    .await
                    .map_err(|e| format!("Failed to undeafen: {e}"))?;
                handle
                    .send(command::SetSelfMute { muted: true })
                    .await
                    .map_err(|e| format!("Failed to mute: {e}"))?;
            }

            self.emit_voice_state();
            Ok(())
        }

        /// Toggle mute.
        ///
        /// | Current   | Result  |
        /// |-----------|---------|
        /// | Inactive  | Active  | (full unmute + undeaf)
        /// | Active    | Muted   |
        /// | Muted     | Active  |
        pub async fn toggle_mute(&self) -> Result<(), String> {
            let (voice_state, handle, audio_settings) = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                (
                    state.audio.voice_state,
                    state.conn.client_handle.clone(),
                    state.audio.settings.clone(),
                )
            };

            match voice_state {
                VoiceState::Active => {
                    info!("toggle_mute: muting (stopping outbound)");
                    self.stop_outbound();
                    {
                        let __session = self.inner.snapshot();
                        let mut state = __session.lock().map_err(|e| e.to_string())?;
                        state.audio.voice_state = VoiceState::Muted;
                    }
                    if let Some(handle) = handle {
                        handle
                            .send(command::SetSelfMute { muted: true })
                            .await
                            .map_err(|e| format!("Failed to mute: {e}"))?;
                    }
                }
                VoiceState::Muted => {
                    info!("toggle_mute: unmuting (restarting outbound)");
                    self.start_outbound_pipeline(&audio_settings, &handle)?;
                    {
                        let __session = self.inner.snapshot();
                        let mut state = __session.lock().map_err(|e| e.to_string())?;
                        state.audio.voice_state = VoiceState::Active;
                    }
                    if let Some(handle) = handle {
                        handle
                            .send(command::SetSelfMute { muted: false })
                            .await
                            .map_err(|e| format!("Failed to unmute: {e}"))?;
                    }
                }
                VoiceState::Inactive => {
                    info!("toggle_mute: enabling voice from inactive");
                    self.enable_voice().await?;
                    return Ok(());
                }
            }

            self.emit_voice_state();
            Ok(())
        }

        /// Toggle deafen.
        ///
        /// | Current   | Result   |
        /// |-----------|----------|
        /// | Inactive  | Muted    | (undeaf, stay muted)
        /// | Active    | Inactive | (deaf + muted)
        /// | Muted     | Inactive | (deaf + muted)
        pub async fn toggle_deafen(&self) -> Result<(), String> {
            let voice_state = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                state.audio.voice_state
            };

            match voice_state {
                VoiceState::Active | VoiceState::Muted => {
                    self.disable_voice().await?;
                }
                VoiceState::Inactive => {
                    self.enable_voice_muted().await?;
                }
            }
            Ok(())
        }

        /// Activate the mic for push-to-talk (key pressed).
        ///
        /// Transitions Muted/Inactive -> Active and sends unmute to the server.
        /// If already Active, this is a no-op so rapid key repeats are harmless.
        pub async fn push_to_talk_start(&self) -> Result<(), String> {
            let (voice_state, handle, audio_settings) = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                (
                    state.audio.voice_state,
                    state.conn.client_handle.clone(),
                    state.audio.settings.clone(),
                )
            };

            if voice_state == VoiceState::Active {
                return Ok(());
            }

            if voice_state == VoiceState::Inactive {
                self.enable_voice_muted().await?;
            }

            self.start_outbound_pipeline(&audio_settings, &handle)?;
            {
                let __session = self.inner.snapshot();
                let mut state = __session.lock().map_err(|e| e.to_string())?;
                state.audio.voice_state = VoiceState::Active;
            }
            if let Some(handle) = handle {
                handle
                    .send(command::SetSelfMute { muted: false })
                    .await
                    .map_err(|e| format!("Failed to unmute for PTT: {e}"))?;
            }
            self.emit_voice_state();
            Ok(())
        }

        /// Deactivate the mic for push-to-talk (key released).
        ///
        /// Transitions Active -> Muted and sends mute to the server.
        /// If not Active, this is a no-op.
        pub async fn push_to_talk_end(&self) -> Result<(), String> {
            let (voice_state, handle) = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                (state.audio.voice_state, state.conn.client_handle.clone())
            };

            if voice_state != VoiceState::Active {
                return Ok(());
            }

            self.stop_outbound();
            {
                let __session = self.inner.snapshot();
                let mut state = __session.lock().map_err(|e| e.to_string())?;
                state.audio.voice_state = VoiceState::Muted;
            }
            if let Some(handle) = handle {
                handle
                    .send(command::SetSelfMute { muted: true })
                    .await
                    .map_err(|e| format!("Failed to mute after PTT: {e}"))?;
            }
            self.emit_voice_state();
            Ok(())
        }

        /// Reconcile the live voice state to the current transmit mode after a
        /// mode change while connected. This is the single authority for the
        /// VOX/PTT/Continuous transition and replaces the old ad-hoc flag:
        ///
        /// - **Push-to-Talk** -> the mic must be closed until the key is held,
        ///   so drop to `Muted` (stops outbound, asserts self-mute). From there
        ///   `push_to_talk_start` opens it.
        /// - **VoiceActivity / Continuous** -> the mic is open (gated or not),
        ///   so come up to `Active`: rebuild the outbound chain for the new mode
        ///   and clear self-mute. This is what un-sticks a client that was left
        ///   Muted by a previous PTT selection.
        ///
        /// A no-op when voice is Inactive (not connected / not enabled) - the
        /// next `enable_voice` already honours the mode.
        pub async fn reconcile_voice_for_mode(&self) -> Result<(), String> {
            let (voice_state, mode) = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                (state.audio.voice_state, state.audio.settings.transmit_mode())
            };

            if voice_state == VoiceState::Inactive {
                return Ok(());
            }

            match mode {
                TransmitMode::PushToTalk => {
                    // Close the mic (idempotent if already Muted).
                    info!("reconcile_voice_for_mode: -> push-to-talk, closing mic");
                    self.push_to_talk_end().await
                }
                TransmitMode::VoiceActivity | TransmitMode::Continuous => {
                    // Open the mic in the new mode. push_to_talk_start rebuilds
                    // the outbound pipeline (via start_outbound_pipeline) with
                    // the freshly-stored settings, so the gate is correctly
                    // present (VOX) or absent (Continuous), and clears
                    // self-mute. Idempotent-ish: from Active it is a no-op, so
                    // force a rebuild by muting first only when needed.
                    info!("reconcile_voice_for_mode: -> {mode:?}, opening mic");
                    if voice_state == VoiceState::Active {
                        // Already open: rebuild the chain for the new mode.
                        self.restart_outbound()
                    } else {
                        // Was Muted (e.g. coming from PTT): open it.
                        self.push_to_talk_start().await
                    }
                }
            }
        }


        /// `mic-amplitude` events to the frontend at ~30 Hz.
        ///
        /// When `auto_input_sensitivity` is enabled, the measured
        /// noise floor (post-AGC) is used to auto-adjust `vad_threshold`.
        pub fn start_mic_test(&self) -> Result<(), String> {
            // Stop any already-running mic test.
            self.stop_mic_test();

            let audio_settings = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                state.audio.settings.clone()
            };

            let input_vol = Arc::new(AtomicU32::new(audio_settings.input_volume.to_bits()));

            let mut capture = PlatformAudioFactory::create_capture(
                audio_settings.selected_device.as_deref(),
                960, // 20ms @ 48kHz
                input_vol,
            )?;

            capture.start().map_err(|e| format!("Mic test capture start: {e}"))?;

            // Same AGC config as the voice pipeline so the calibrator
            // measures the post-gain signal the noise gate will see.
            let agc_filter = build_agc_filter(&audio_settings);

            let app = self.app_handle().ok_or("No app handle")?;
            let auto_sensitivity = audio_settings.auto_input_sensitivity;
            let inner = self.inner.clone_arc();

            let handle = tauri::async_runtime::spawn(async move {
                mic_test_loop(capture, app, auto_sensitivity, inner, agc_filter).await;
            });

            let __session = self.inner.snapshot();

            let mut state = __session.lock().map_err(|e| e.to_string())?;
            state.audio.mic_test_handle = Some(handle);
            Ok(())
        }

        /// Stop the mic test.
        pub fn stop_mic_test(&self) {
            if let Ok(mut state) = self.inner.snapshot().lock() {
                if let Some(handle) = state.audio.mic_test_handle.take() {
                    handle.abort();
                }
            }
        }

        /// Start a voice replay session.
        ///
        /// Records the local microphone through the *exact* filter chain
        /// used for live transmission (AGC + denoiser + noise gate), then
        /// plays the captured buffer back through the output device.
        ///
        /// Either press stop again (which advances or ends the cycle) or
        /// let the loop auto-stop when the ~20 s buffer fills.
        pub fn start_voice_replay(&self) -> Result<(), String> {
            self.stop_voice_replay();

            let (audio_settings, speaker_volumes) = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                (
                    state.audio.settings.clone(),
                    state.audio.speaker_volumes.clone(),
                )
            };

            let input_vol = Arc::new(AtomicU32::new(audio_settings.input_volume.to_bits()));
            let output_vol = Arc::new(AtomicU32::new(audio_settings.output_volume.to_bits()));

            let capture = PlatformAudioFactory::create_capture(
                audio_settings.selected_device.as_deref(),
                960,
                input_vol,
            )?;
            let filters = build_outbound_filters(&audio_settings);
            let (playback, speaker_buffers) = super::super::voice_replay::make_voice_replay_playback(
                &audio_settings,
                output_vol,
                speaker_volumes,
            )?;

            let app = self.app_handle().ok_or("No app handle")?;
            let (stop_tx, stop_rx) = tokio::sync::watch::channel(true);

            let context = super::super::voice_replay::VoiceReplayContext {
                capture,
                filters,
                playback,
                speaker_buffers,
            };

            let handle = tauri::async_runtime::spawn(async move {
                super::super::voice_replay::voice_replay_loop(context, app, stop_rx).await;
            });

            let __session = self.inner.snapshot();
            let mut state = __session.lock().map_err(|e| e.to_string())?;
            state.audio.voice_replay_handle = Some(handle);
            state.audio.voice_replay_stop = Some(stop_tx);
            Ok(())
        }

        /// Signal the replay loop to advance: from Recording it stops
        /// capture and starts playback; from Playing it stops playback;
        /// from Idle it is a no-op.  The task itself emits the matching
        /// `voice-replay-state` event so the UI stays in sync.
        pub fn stop_voice_replay(&self) {
            let session = self.inner.snapshot();
            let Ok(state) = session.lock() else {
                return;
            };
            if let Some(tx) = &state.audio.voice_replay_stop {
                // Flip the watch value to `false` so the loop's
                // `has_changed()` polling sees the request.  The sender
                // is intentionally left in state so a second stop call
                // (e.g. stopping playback after stopping recording) still
                // has a live sender to signal through.
                let _ = tx.send(false);
            }
        }

        /// Start sending TCP pings at high frequency to measure round-trip latency.
        ///
        /// RTT is computed in the event handler when the server echoes the ping
        /// back, and emitted as a `"ping-latency"` Tauri event.
        pub fn start_latency_test(&self) -> Result<(), String> {
            self.stop_latency_test();

            let client_handle = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                state
                    .conn.client_handle
                    .clone()
                    .ok_or_else(|| "Not connected".to_string())?
            };

            let handle = tauri::async_runtime::spawn(async move {
                latency_ping_loop(client_handle).await;
            });

            let __session = self.inner.snapshot();

            let mut state = __session.lock().map_err(|e| e.to_string())?;
            state.audio.latency_test_handle = Some(handle);
            Ok(())
        }

        /// Stop the latency test.
        pub fn stop_latency_test(&self) {
            if let Ok(mut state) = self.inner.snapshot().lock() {
                if let Some(handle) = state.audio.latency_test_handle.take() {
                    handle.abort();
                }
            }
        }

        /// One-shot voice activation calibration.
        ///
        /// Opens the microphone, applies the same AGC filter chain as
        /// the voice pipeline, feeds ~3 seconds of audio into the
        /// shared [`Calibrator`], and writes the resulting open
        /// threshold, close ratio, and hold time back into
        /// [`AudioSettings`].  Returns the new open threshold.
        ///
        /// The estimator rejects digital silence (see
        /// [`super::super::calibration`]) so a user who pauses during
        /// calibration no longer collapses the noise floor.
        pub async fn calibrate_voice_threshold(&self) -> Result<f32, String> {
            use super::super::calibration::{frame_rms, Calibrator, AUTO_CALIBRATION_WINDOW};

            let audio_settings = {
                let __session = self.inner.snapshot();
                let state = __session.lock().map_err(|e| e.to_string())?;
                state.audio.settings.clone()
            };

            let input_vol = Arc::new(AtomicU32::new(audio_settings.input_volume.to_bits()));

            let mut capture = PlatformAudioFactory::create_capture(
                audio_settings.selected_device.as_deref(),
                960,
                input_vol,
            )?;
            capture.start().map_err(|e| format!("Calibration capture start: {e}"))?;

            let mut agc_filter = build_agc_filter(&audio_settings);

            let mut calibrator = Calibrator::new(AUTO_CALIBRATION_WINDOW);
            let mut interval = tokio::time::interval(Duration::from_millis(33));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            // Sample for ~3 s (90 frames at 30 Hz) so the percentile
            // estimator has enough non-silent frames to be meaningful.
            for _ in 0..90 {
                let _ = interval.tick().await;

                let mut latest = None;
                while let Ok(frame) = capture.read_frame() {
                    latest = Some(frame);
                }
                let Some(mut frame) = latest else {
                    continue;
                };

                if let Some(ref mut agc) = agc_filter {
                    use mumble_protocol::audio::filter::AudioFilter as _;
                    let pre_agc_rms = frame_rms(&frame);
                    let _ = agc.process(&mut frame);
                    calibrator.push(frame_rms(&frame), pre_agc_rms);
                } else {
                    let rms = frame_rms(&frame);
                    calibrator.push(rms, rms);
                }
            }

            let _ = capture.stop();

            let Some(calibration) = calibrator.compute() else {
                return Err(
                    "Calibration failed: not enough audio captured.  Speak normally for a few seconds and try again.".into(),
                );
            };

            info!(
                "calibrate_voice_threshold: open={:.5}, close_ratio={:.3}, hold_frames={}, max_gain_db={:.1}",
                calibration.vad_threshold, calibration.noise_gate_close_ratio, calibration.hold_frames, calibration.max_gain_db,
            );

            let payload = if let Ok(mut state) = self.inner.snapshot().lock() {
                state.audio.settings.vad_threshold = calibration.vad_threshold;
                state.audio.settings.noise_gate_close_ratio = calibration.noise_gate_close_ratio;
                state.audio.settings.hold_frames = calibration.hold_frames;
                state.audio.settings.max_gain_db = calibration.max_gain_db;
                Some(super::super::types::VoiceActivationCalibrationPayload {
                    vad_threshold: calibration.vad_threshold,
                    noise_gate_close_ratio: calibration.noise_gate_close_ratio,
                    hold_frames: calibration.hold_frames,
                    max_gain_db: calibration.max_gain_db,
                })
            } else {
                None
            };

            if let (Some(app), Some(payload)) = (self.app_handle(), payload) {
                use tauri::Emitter as _;
                let _ = app.emit("voice-activation-calibrated", payload);
            }

            Ok(calibration.vad_threshold)
        }
    }

    /// Clamp the configured encoder settings to the server's per-user
    /// bandwidth budget (`ServerSync.max_bandwidth`), mirroring the
    /// official client's `AudioInput::adjustBandwidth`.
    ///
    /// The budget covers the full network cost: Opus payload plus
    /// per-packet IP/UDP/OCB2/header overhead.  Servers *silently drop*
    /// voice packets from clients that exceed it (legacy servers default
    /// to 72 kbit/s), so sending more only causes bursts of lost audio.
    /// Keeps the configured packet duration when a reduced bitrate still
    /// fits; otherwise falls back to longer packets to shrink the
    /// per-packet overhead before giving up quality entirely.
    fn adjust_to_server_bandwidth(
        bitrate_bps: i32,
        frame_size_ms: u32,
        max_bandwidth: Option<u32>,
    ) -> (i32, u32) {
        /// Quality floor below which longer packets beat lower bitrate.
        const MIN_BITRATE: i32 = 8_000;
        /// Valid Opus packet durations in ms, shortest first.
        const FRAME_STEPS: [u32; 4] = [10, 20, 40, 60];
        /// Per-packet overhead: IP (20) + UDP (8) + OCB2 tag (4) +
        /// audio header byte and sequence/length varints (~6).
        const OVERHEAD_BYTES: i32 = 20 + 8 + 4 + 6;

        let Some(max_bw) = max_bandwidth else {
            return (bitrate_bps, frame_size_ms);
        };
        let max_bw = max_bw as i32;

        let mut frame_ms = if FRAME_STEPS.contains(&frame_size_ms) {
            frame_size_ms
        } else {
            20
        };
        loop {
            let packets_per_sec = 1000_u32.div_ceil(frame_ms) as i32;
            let allowed_bitrate = max_bw - packets_per_sec * OVERHEAD_BYTES * 8;
            if bitrate_bps <= allowed_bitrate {
                return (bitrate_bps, frame_ms);
            }
            if allowed_bitrate >= MIN_BITRATE {
                return (allowed_bitrate, frame_ms);
            }
            match FRAME_STEPS.iter().find(|&&f| f > frame_ms) {
                Some(&next) => frame_ms = next,
                None => return (MIN_BITRATE, frame_ms),
            }
        }
    }

    /// Build the full outbound filter chain used by the live voice
    /// pipeline.  Shared by `start_outbound_pipeline` and the voice
    /// replay task so users hear *exactly* what is sent over the wire,
    /// gating, denoising, AGC and all.
    pub(super) fn build_outbound_filters(settings: &AudioSettings) -> FilterChain {
        let mut filters = FilterChain::new();
        if let Some(agc) = build_agc_filter(settings) {
            filters.push(Box::new(agc));
        }
        // Noise suppression cleans the signal that is sent; it says nothing
        // about *whether* to send. So it is applied on its own toggle,
        // independent of the transmit mode.
        if settings.noise_suppression {
            let denoiser_config = DenoiserConfig {
                algorithm: settings.denoiser_algorithm,
                params: settings.denoiser_params.clone(),
                ..DenoiserConfig::default()
            };
            filters.push(Box::new(SpectralDenoiser::new(denoiser_config)));
        }
        // The voice gate is what makes transmission conditional, so it belongs
        // only to VoiceActivity mode. Continuous keeps the mic permanently open
        // (the radio Station case); PushToTalk gates by self-mute at the
        // protocol level, not by a filter. In both of those the gate must be
        // absent, or the far end would lose audio the operator expects to send.
        //
        // The gate runs AFTER the AGC so its threshold matches what the
        // calibrator measured (also post-AGC) - keep the two in the same
        // signal domain or a calibrated threshold is meaningless.
        if settings.transmit_mode() == TransmitMode::VoiceActivity {
            // Floor the open threshold so a stale/over-eager auto-calibration -
            // which can pin `vad_threshold` near its ~-50 dBFS minimum - cannot
            // leave the gate effectively always-open on a quiet room's
            // AGC-amplified noise floor. -40 dBFS (~0.01 linear) sits well below
            // a normal speaking level but above residual ambient. This is the
            // main guard against "VOX stuck open in a quiet room".
            const MIN_OPEN_THRESHOLD: f32 = 0.01;
            let open = settings.vad_threshold.max(MIN_OPEN_THRESHOLD);
            filters.push(Box::new(NoiseGate::new(NoiseGateConfig {
                open_threshold: open,
                close_threshold: open * settings.noise_gate_close_ratio,
                hold_frames: settings.hold_frames,
                ..NoiseGateConfig::default()
            })));
        }
        filters
    }

    /// Outbound filter chain for a STEREO Station. Only channel-safe filters are
    /// included: the AGC applies one linked gain per interleaved sample (safe for
    /// stereo), while the spectral denoiser, low-pass, and noise gate all assume a
    /// mono stream and would corrupt interleaved L/R — so they are omitted. A
    /// Station is Continuous (no gate) and its audio is receiver output, not a
    /// voice mic, so denoising/gating are undesirable here anyway.
    pub(super) fn build_outbound_filters_stereo(settings: &AudioSettings) -> FilterChain {
        let mut filters = FilterChain::new();
        if let Some(agc) = build_agc_filter(settings) {
            filters.push(Box::new(agc));
        }
        filters
    }

    /// Build the AGC filter used by the calibration paths.  Mirrors
    /// the configuration applied by `start_outbound_pipeline` so the
    /// calibrator sees the same post-gain signal the noise gate will.
    fn build_agc_filter(settings: &AudioSettings) -> Option<AutomaticGainControl> {
        if !settings.auto_gain {
            return None;
        }
        let max_gain_linear = 10.0_f32.powf(settings.max_gain_db / 20.0);
        #[cfg(target_os = "android")]
        let agc_config = AgcConfig {
            max_gain: max_gain_linear.min(2.0),
            ..AgcConfig::default()
        };
        #[cfg(not(target_os = "android"))]
        let agc_config = AgcConfig {
            max_gain: max_gain_linear,
            ..AgcConfig::default()
        };
        Some(AutomaticGainControl::new(agc_config))
    }

    // Background task functions are in the `audio_tasks` sibling module
    // to keep this file's size manageable.
    use super::super::audio_tasks::{latency_ping_loop, mic_test_loop, outbound_audio_loop};

    #[cfg(test)]
    mod tests {
        use super::*;
        use mumble_protocol::audio::sample::AudioFormat;

        #[test]
        fn bandwidth_unknown_keeps_settings() {
            assert_eq!(
                adjust_to_server_bandwidth(128_000, 10, None),
                (128_000, 10)
            );
        }

        #[test]
        fn bandwidth_generous_budget_keeps_settings() {
            // Mumble 1.5+ default budget of 558 kbit/s fits everything.
            assert_eq!(
                adjust_to_server_bandwidth(128_000, 10, Some(558_000)),
                (128_000, 10)
            );
        }

        #[test]
        fn bandwidth_legacy_default_clamps_bitrate() {
            // Legacy 72 kbit/s budget at 10 ms packets: 100 packets/s
            // of 38-byte overhead leaves 41.6 kbit/s for Opus.
            let (bitrate, frame_ms) = adjust_to_server_bandwidth(128_000, 10, Some(72_000));
            assert_eq!(frame_ms, 10, "frame size kept when clamped bitrate fits");
            assert!((8_000..=41_600).contains(&bitrate), "got {bitrate}");
            // The result must actually fit the budget.
            assert!(bitrate + 100 * 38 * 8 <= 72_000);
        }

        #[test]
        fn bandwidth_tight_budget_grows_frame_size() {
            // 16 kbit/s leaves no room at 10/20 ms, so packets get longer.
            let (bitrate, frame_ms) = adjust_to_server_bandwidth(72_000, 10, Some(16_000));
            assert!(frame_ms > 20, "expected longer packets, got {frame_ms} ms");
            assert!(bitrate >= 8_000);
        }

        #[test]
        fn bandwidth_absurd_budget_floors_at_min_bitrate() {
            let (bitrate, frame_ms) = adjust_to_server_bandwidth(72_000, 20, Some(1_000));
            assert_eq!(bitrate, 8_000);
            assert_eq!(frame_ms, 60);
        }

        #[test]
        #[ignore = "requires audio hardware - run with --ignored"]
        fn create_capture_returns_correct_format() {
            let vol = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
            let Ok(capture) = PlatformAudioFactory::create_capture(None, 960, vol) else {
                eprintln!("Skipping: no audio input device available");
                return;
            };
            assert_eq!(capture.format(), AudioFormat::MONO_48KHZ_F32);
        }

        #[test]
        #[ignore = "requires audio hardware - run with --ignored"]
        fn factory_capture_is_compatible_with_pipeline() {
            use mumble_protocol::audio::filter::FilterChain;
            use mumble_protocol::audio::pipeline::OutboundPipeline;

            let vol = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
            let Ok(capture) = PlatformAudioFactory::create_capture(None, 960, vol) else { return };

            let Ok(encoder) = OpusEncoder::new(
                OpusEncoderConfig::default(),
                AudioFormat::MONO_48KHZ_F32,
            ) else {
                return;
            };

            let _pipeline = OutboundPipeline::new(
                capture,
                FilterChain::new(),
                Box::new(encoder),
            );
        }

        // --- Transmit mode / VAD decoupling (§14c) ------------------------

        use crate::state::types::TransmitMode;
        use mumble_protocol::audio::sample::AudioFrame;

        /// A mono/48k frame of constant amplitude, for driving the gate.
        fn frame_at(amplitude: f32, samples: usize) -> AudioFrame {
            let mut data = Vec::with_capacity(samples * 4);
            for _ in 0..samples {
                data.extend_from_slice(&amplitude.to_le_bytes());
            }
            AudioFrame {
                data,
                format: AudioFormat::MONO_48KHZ_F32,
                sequence: 0,
                is_silent: false,
            }
        }

        /// Settings with every processing stage off, so a test turns on only
        /// the one thing it means to exercise.
        fn bare_settings() -> AudioSettings {
            AudioSettings {
                auto_gain: false,
                noise_suppression: false,
                transmit_mode: TransmitMode::VoiceActivity,
                push_to_talk: false,
                ..AudioSettings::default()
            }
        }

        #[test]
        fn voice_activity_mode_inserts_a_gate_that_silences_quiet_input() {
            let settings = AudioSettings {
                transmit_mode: TransmitMode::VoiceActivity,
                ..bare_settings()
            };
            let mut chain = build_outbound_filters(&settings);
            assert_eq!(chain.len(), 1, "only the voice gate should be present");

            // Below the 0.01 open threshold: the gate must mark it silent.
            let mut quiet = frame_at(0.001, 480);
            assert!(chain.process(&mut quiet).is_ok(), "filter chain runs");
            assert!(
                quiet.is_silent,
                "VoiceActivity must gate sub-threshold input"
            );
        }

        #[test]
        fn continuous_mode_has_no_gate_so_quiet_input_still_transmits() {
            let settings = AudioSettings {
                transmit_mode: TransmitMode::Continuous,
                ..bare_settings()
            };
            let mut chain = build_outbound_filters(&settings);
            assert!(
                chain.is_empty(),
                "Continuous with no AGC/suppression is a pass-through"
            );

            // The same quiet frame must NOT be silenced: the mic is always live.
            let mut quiet = frame_at(0.001, 480);
            assert!(chain.process(&mut quiet).is_ok(), "filter chain runs");
            assert!(
                !quiet.is_silent,
                "Continuous must never gate — the radio stays keyed up"
            );
        }

        #[test]
        fn push_to_talk_mode_has_no_voice_gate() {
            // PTT gates by self-mute at the protocol level, not by a filter.
            let settings = AudioSettings {
                transmit_mode: TransmitMode::PushToTalk,
                ..bare_settings()
            };
            let chain = build_outbound_filters(&settings);
            assert!(chain.is_empty(), "PushToTalk must not insert a voice gate");
        }

        #[test]
        fn noise_suppression_is_independent_of_the_voice_gate() {
            // The historical bug: the gate hung off `noise_suppression`. Now a
            // Continuous Station can clean its audio without re-introducing a
            // gate, and VoiceActivity can gate with suppression off.
            let suppressed_continuous = AudioSettings {
                transmit_mode: TransmitMode::Continuous,
                noise_suppression: true,
                ..bare_settings()
            };
            let chain = build_outbound_filters(&suppressed_continuous);
            // Denoiser present, but still no gate: a quiet frame stays live.
            assert_eq!(chain.len(), 1, "denoiser only, no gate");

            let gated_unsuppressed = AudioSettings {
                transmit_mode: TransmitMode::VoiceActivity,
                noise_suppression: false,
                ..bare_settings()
            };
            assert_eq!(
                build_outbound_filters(&gated_unsuppressed).len(),
                1,
                "gate present even with suppression off"
            );
        }

        #[test]
        fn legacy_push_to_talk_flag_is_honoured_when_mode_is_unset() {
            // A config saved before `transmit_mode` existed: the field defaults
            // to VoiceActivity but the legacy boolean says PTT, which must win.
            let legacy = AudioSettings {
                transmit_mode: TransmitMode::VoiceActivity,
                push_to_talk: true,
                ..bare_settings()
            };
            assert_eq!(legacy.transmit_mode(), TransmitMode::PushToTalk);
            // And so no voice gate is inserted for that legacy PTT user.
            assert!(build_outbound_filters(&legacy).is_empty());

            // An explicit Continuous choice must not be overridden by a stale
            // boolean left set in an old config.
            let explicit = AudioSettings {
                transmit_mode: TransmitMode::Continuous,
                push_to_talk: true,
                ..bare_settings()
            };
            assert_eq!(explicit.transmit_mode(), TransmitMode::Continuous);
        }

        #[test]
        fn changing_transmit_mode_requires_a_pipeline_restart() {
            let voice = bare_settings();
            let continuous = AudioSettings {
                transmit_mode: TransmitMode::Continuous,
                ..bare_settings()
            };
            assert!(
                voice.needs_pipeline_restart(&continuous),
                "swapping the gate in or out must rebuild the pipeline"
            );
        }

        #[test]
        #[ignore = "requires audio hardware - run with --ignored"]
        fn factory_mixing_playback_can_be_started() {
            let vol = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
            let bufs: SpeakerBuffers = Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            );
            let sv: mumble_protocol::audio::mixer::SpeakerVolumes = Default::default();
            let stereo = Arc::new(mumble_protocol::audio::mixer::StereoControl::default());
            let Ok(mut playback) =
                PlatformAudioFactory::create_mixing_playback(None, vol, bufs, sv, stereo)
            else {
                eprintln!("Skipping: no audio output device available");
                return;
            };
            if playback.start().is_err() {
                eprintln!("Skipping: audio output device found but stream could not start");
                return;
            }
            assert!(playback.stop().is_ok());
        }
    }
}
