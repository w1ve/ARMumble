//! Commands callable from the UI.

use rr_client::RadioClient;
use tauri::{AppHandle, Emitter, State};

use crate::engine::{apply_route, lock, restart_monitor, restart_otrsp, route_dto, update_keyer, AppState, Engine};
use crate::routing::RouteDto;
use crate::platform::{self, DeviceDto, PortDto};
use crate::settings::{settings_path, Settings};
use crate::status::Status;

fn client_of(state: &State<'_, AppState>) -> Result<RadioClient, String> {
    lock(&state.engine).as_ref().map(|e| e.client.clone()).ok_or_else(|| "not connected".to_owned())
}

/// Push live-adjustable settings into a running client.
fn apply_live(client: &RadioClient, s: &Settings) {
    let a = &s.audio;
    let cap = client.capture();
    cap.set_gain(a.input_gain);
    cap.set_channel_map(a.map());
    cap.set_tx_mode(a.mode());
    cap.set_vox(a.vox_threshold_db, a.vox_hang_ms);
    client.set_quality(a.bitrate_kbps * 1000, a.stereo, a.max_frame_ms);
    client.set_master_gain(a.master_gain);
}

/// The saved settings.
#[tauri::command]
pub fn load_settings(state: State<'_, AppState>) -> Settings {
    lock(&state.settings).clone()
}

/// Save settings, re-register hotkeys and apply live changes.
#[tauri::command]
pub fn save_settings(app: AppHandle, state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    let path = lock(&state.settings_path).clone();
    settings.save(&path)?;
    platform::apply_hotkeys(&app, &settings.hotkeys);
    let old = std::mem::replace(&mut *lock(&state.settings), settings.clone()).audio;
    let new = &settings.audio;
    if let Some(e) = lock(&state.engine).as_ref() {
        apply_live(&e.client, &settings);
    }
    if old.otrsp_port != new.otrsp_port {
        restart_otrsp(&app);
    }
    // Manual buttons, the OTRSP checkbox, role and mono are all applied here.
    update_keyer(&state);
    let _ = apply_route(&app);
    if old.input_device != new.input_device || old.so2r_role != new.so2r_role {
        restart_monitor(&state);
    } else if let Some(m) = lock(&state.monitor).as_ref() {
        m.set(new.input_gain, new.map());
    }
    Ok(())
}

/// Sound devices.
#[tauri::command]
pub fn list_devices() -> Vec<DeviceDto> {
    platform::devices()
}

/// Serial ports.
#[tauri::command]
pub fn list_serial_ports() -> Vec<PortDto> {
    platform::serial_ports()
}

/// Connect using the saved settings.
#[tauri::command]
pub async fn connect(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let old = lock(&state.engine).take();
    if let Some(e) = old {
        e.stop().await;
    }
    // Release the meter-only stream so the engine can own the input device.
    *lock(&state.monitor) = None;
    let settings = lock(&state.settings).clone();
    match Engine::start(&app, &settings, &state.so2r).await {
        Ok(engine) => {
            *lock(&state.engine) = Some(engine);
            Ok(())
        }
        Err(e) => {
            restart_monitor(&state);
            Err(e)
        }
    }
}

/// Disconnect and release the sound card and the rig.
#[tauri::command]
pub async fn disconnect(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let old = lock(&state.engine).take();
    if let Some(e) = old {
        e.stop().await;
    }
    restart_monitor(&state);
    app.emit("status", Status::idle()).map_err(|e| e.to_string())
}

/// Move to a channel.
#[tauri::command]
pub async fn join_channel(state: State<'_, AppState>, id: u32) -> Result<(), String> {
    let c = client_of(&state)?;
    c.join_channel(id).await.map_err(|e| e.to_string())
}

/// Press or release PTT from the UI.
#[tauri::command]
pub fn set_ptt(state: State<'_, AppState>, down: bool) -> Result<(), String> {
    client_of(&state)?.set_ptt(down);
    Ok(())
}

/// Hold-to-listen on a channel.
#[tauri::command]
pub async fn listen_in(state: State<'_, AppState>, target: u32, down: bool) -> Result<(), String> {
    let mute = lock(&state.settings).hotkeys.mute_home_on_listen;
    let c = client_of(&state)?;
    c.listen_in(target, down, mute).await.map_err(|e| e.to_string())
}

/// Hold-to-whisper to a channel.
#[tauri::command]
pub async fn whisper(state: State<'_, AppState>, target: u32, down: bool) -> Result<(), String> {
    let c = client_of(&state)?;
    c.whisper(target, down).await.map_err(|e| e.to_string())
}

/// Volume of one talker.
#[tauri::command]
pub fn set_user_gain(state: State<'_, AppState>, session: u32, gain: f32) -> Result<(), String> {
    client_of(&state)?.set_user_gain(session, gain);
    Ok(())
}

/// Current two-radio routing.
#[tauri::command]
pub fn get_route(state: State<'_, AppState>) -> RouteDto {
    route_dto(&state)
}

/// Where the settings file lives (for the "about" box).
#[tauri::command]
pub fn settings_location(state: State<'_, AppState>) -> String {
    lock(&state.settings_path).display().to_string()
}

/// Compute the settings path inside `dir` (used at startup).
#[must_use]
pub fn path_in(dir: &std::path::Path) -> std::path::PathBuf {
    settings_path(dir)
}
