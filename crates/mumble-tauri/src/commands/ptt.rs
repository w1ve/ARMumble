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
