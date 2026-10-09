//! Rig-PTT commands: serial-port enumeration and per-server PTT configuration.
//!
//! The live PTT controller (serial keying / footswitch sensing) is driven by
//! the backend state machine; these commands expose the serial-port list to
//! the settings UI and persist the per-server [`PttConfig`].

use crate::state::types::PttConfig;
use crate::state::AppState;

/// A serial port the user can pick for PTT: OS name plus a friendly label.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SerialPortInfo {
    /// OS port name (e.g. `"COM5"`, `"/dev/ttyUSB0"`).
    pub name: String,
    /// Human-friendly description (USB product string, or the name again).
    pub label: String,
}

/// Enumerate the serial ports available for PTT, sorted by name.
///
/// Desktop only - returns an empty list on Android (no rig keying on a phone).
#[cfg(not(target_os = "android"))]
#[tauri::command]
pub(crate) fn list_serial_ports() -> Vec<SerialPortInfo> {
    match rr_ptt::available_ports() {
        Ok(ports) => ports
            .into_iter()
            .map(|(name, label)| SerialPortInfo { name, label })
            .collect(),
        Err(e) => {
            tracing::warn!("serial port enumeration failed: {e}");
            Vec::new()
        }
    }
}

/// Stub: Android has no serial PTT.
#[cfg(target_os = "android")]
#[tauri::command]
pub(crate) fn list_serial_ports() -> Vec<SerialPortInfo> {
    Vec::new()
}

/// Station: apply a remote operator's PTT intent (received as an inbound
/// `remote-radio/ptt` plugin message and routed here by the frontend) to the
/// live PTT controller, which keys/unkeys the rig through the safety watchdog.
///
/// A no-op unless a Station PTT controller is running. Desktop only.
#[cfg(not(target_os = "android"))]
#[tauri::command]
pub(crate) fn apply_remote_ptt(state: tauri::State<'_, AppState>, down: bool) {
    state.apply_remote_ptt(down);
}

/// Stub: Android has no rig keying.
#[cfg(target_os = "android")]
#[tauri::command]
pub(crate) fn apply_remote_ptt(_state: tauri::State<'_, AppState>, _down: bool) {}

/// Operator software PTT (from the operator PTT hotkey). While held it does two
/// things at once (CONTEXT §14d, mode B): opens this client's Mumble mic (same
/// self-mute path as the global push-to-talk) AND broadcasts an outbound
/// `remote-radio/ptt` so the station keys its rig deterministically.
///
/// `down = true` on key press, `false` on release.
#[cfg(not(target_os = "android"))]
#[tauri::command]
pub(crate) async fn operator_ptt(
    state: tauri::State<'_, AppState>,
    down: bool,
) -> Result<(), String> {
    // Mic first so local monitoring/VU reacts immediately; then the rig intent.
    if down {
        state.push_to_talk_start().await?;
    } else {
        state.push_to_talk_end().await?;
    }
    state.send_operator_ptt(down).await
}

/// Stub: Android has no operator rig control.
#[cfg(target_os = "android")]
#[tauri::command]
pub(crate) async fn operator_ptt(
    _state: tauri::State<'_, AppState>,
    _down: bool,
) -> Result<(), String> {
    Ok(())
}

/// Operator "Whisper" (from the whisper hotkey). While held, the mic is opened
/// and every outbound packet is stamped with VoiceTarget id 1, which is
/// registered to the configured target channel - so the operator's voice goes
/// into that channel without leaving their own. On release the mic closes and
/// the target reverts to 0 (normal own-channel talking).
///
/// `active = true` on key press, `false` on release.
#[cfg(not(target_os = "android"))]
#[tauri::command]
pub(crate) async fn whisper_target(
    state: tauri::State<'_, AppState>,
    active: bool,
) -> Result<(), String> {
    /// VoiceTarget slot the whisper feature owns (1-30 are valid).
    const WHISPER_TARGET_ID: u32 = 1;

    use std::sync::atomic::Ordering;
    if active {
        let channel = state
            .ptt_config_for(None)
            .whisper_target_channel
            .ok_or("No whisper target channel configured")?;
        // Remember whether the mic was already live (VOX/Continuous) so release
        // can leave it live, versus PTT where whisper itself opened it.
        let was_active = state.voice_state() == crate::state::types::VoiceState::Active;
        state.whisper_opened_mic.store(!was_active, Ordering::Relaxed);
        // Register the target (idempotent on the server) then open mic + redirect.
        state.set_voice_target_channel(WHISPER_TARGET_ID, channel).await?;
        if !was_active {
            state.push_to_talk_start().await?;
        }
        state.set_outbound_target(WHISPER_TARGET_ID);
        state.emit_whisper_active(true, Some(channel));
    } else {
        // Revert to normal target first so no trailing packet leaks to the
        // whisper channel.
        state.set_outbound_target(0);
        // Only close the mic if whisper opened it (PTT mode). In VOX/Continuous
        // the mic was already live and must stay live for the own channel.
        if state.whisper_opened_mic.swap(false, Ordering::Relaxed) {
            state.push_to_talk_end().await?;
        }
        state.emit_whisper_active(false, None);
    }
    Ok(())
}

/// Stub: Android has no whisper hotkey.
#[cfg(target_os = "android")]
#[tauri::command]
pub(crate) async fn whisper_target(
    _state: tauri::State<'_, AppState>,
    _active: bool,
) -> Result<(), String> {
    Ok(())
}

/// Get the rig-PTT configuration for a server (or the active session).
#[tauri::command]
pub(crate) fn get_ptt_config(
    state: tauri::State<'_, AppState>,
    server_id: Option<crate::state::ServerId>,
) -> PttConfig {
    state.ptt_config_for(server_id)
}

/// Update the rig-PTT configuration for a server (or the active session).
#[tauri::command]
pub(crate) fn set_ptt_config(
    state: tauri::State<'_, AppState>,
    config: PttConfig,
    server_id: Option<crate::state::ServerId>,
) -> Result<(), String> {
    let is_active = state.set_ptt_config_for(server_id, config);
    // When the active session's config changes, (re)start the live controller
    // so the new port/line/role takes effect.
    #[cfg(not(target_os = "android"))]
    if is_active {
        state.restart_ptt();
    }
    let _ = is_active; // referenced only on desktop
    Ok(())
}
