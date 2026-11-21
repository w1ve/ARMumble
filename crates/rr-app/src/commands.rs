//! Commands callable from the UI.

use rr_client::RadioClient;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::engine::{apply_route, lock, restart_monitor, restart_otrsp, route_dto, update_keyer, AppState, Engine};
use crate::routing::RouteDto;
use crate::devices::Devices;
use crate::platform::{self, DeviceDto, PortDto};
use crate::rigs::{Rig, Rigs};
use crate::servers::ServerEntry;
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
    client.set_vox_keys_station(a.vox_keys_station);
    client.set_swap_lr(a.swap_lr);
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
    let devices = Devices::from_settings(new);
    if Devices::from_settings(&old) != devices && lock(&state.engine).is_some() {
        let _task = tauri::async_runtime::spawn(restart_for_devices(app.clone(), devices));
    }
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
    start_engine(&app, &state).await
}

/// (Re)start the engine from the saved settings, replacing a running one.
async fn start_engine(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let settings = lock(&state.settings).clone();
    let rigs = match crate::rigs::load(&rigs_file(state)) {
        Ok(r) => r,
        Err(e) if uses_cat(&settings) => return Err(format!("radio CAT settings: {e}")),
        Err(_) => Rigs::default(),
    };
    let old = lock(&state.engine).take();
    if let Some(e) = old {
        e.stop().await;
    }
    // Release the meter-only stream so the engine can own the input device.
    *lock(&state.monitor) = None;
    match Engine::start(app, &settings, &rigs, &state.so2r).await {
        Ok(engine) => {
            *lock(&state.engine) = Some(engine);
            Ok(())
        }
        Err(e) => {
            restart_monitor(state);
            Err(e)
        }
    }
}

/// The sound devices changed while connected: move the streams to the new
/// devices, keeping the connection, channel and holds. Only adding or removing
/// a second output device needs a reconnect, which then returns to the channel.
async fn restart_for_devices(app: AppHandle, d: Devices) {
    let switched = tauri::async_runtime::spawn_blocking({
        let app = app.clone();
        move || {
            let state = app.state::<AppState>();
            let mut slot = lock(&state.engine);
            slot.as_mut().map(|e| e.switch_devices(d))
        }
    })
    .await;
    match switched {
        Ok(None | Some(Ok(true) | Err(_))) => {}
        Err(e) => tracing::warn!("device switch task failed: {e}"),
        Ok(Some(Ok(false))) => reconnect_in_place(&app).await,
    }
}

/// Reconnect, then go back to the channel we were in.
async fn reconnect_in_place(app: &AppHandle) {
    let state = app.state::<AppState>();
    let channel = lock(&state.engine).as_ref().and_then(|e| e.client.own_channel());
    if let Err(e) = start_engine(app, &state).await {
        tracing::warn!("reconnect on new sound devices failed: {e}");
        let _ = app.emit("status", Status { error: Some(e), ..Status::idle() });
        return;
    }
    let (Some(channel), Some(client)) = (channel, lock(&state.engine).as_ref().map(|e| e.client.clone())) else {
        return;
    };
    for _ in 0..100 {
        if client.own_channel().is_some() && !client.channels().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    if client.own_channel() != Some(channel) {
        if let Err(e) = client.join_channel(channel).await {
            tracing::warn!("could not return to channel {channel}: {e}");
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

/// Start (`play` true) or stop sending the stereo test clip.
#[tauri::command]
pub fn stereo_test(state: State<'_, AppState>, play: bool) -> Result<(), String> {
    let c = client_of(&state)?;
    if play {
        c.capture().play_test(crate::stereo_test::clip()?);
    } else {
        c.capture().stop_test();
    }
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

fn rigs_file(state: &AppState) -> std::path::PathBuf {
    let settings = lock(&state.settings_path).clone();
    let dir = settings.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    crate::rigs::rigs_path(&dir)
}

fn servers_file(state: &State<'_, AppState>) -> std::path::PathBuf {
    let settings = lock(&state.settings_path).clone();
    let dir = settings.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    crate::servers::servers_path(&dir)
}

/// Station PTT keys a radio by CAT command.
fn uses_cat(s: &Settings) -> bool {
    let p = &s.ptt;
    p.station.enabled && (p.station.method == rr_radio::KeyMethod::Cat || p.station2.method == rr_radio::KeyMethod::Cat)
}

/// The CAT settings of the station's radios (`rigs.json`).
#[tauri::command]
pub fn load_rigs(state: State<'_, AppState>) -> Result<Rigs, String> {
    crate::rigs::load(&rigs_file(&state))
}

/// Save the CAT settings of the station's radios. They are used from the next connect.
#[tauri::command]
pub fn save_rigs(state: State<'_, AppState>, rigs: Rigs) -> Result<(), String> {
    crate::rigs::save(&rigs_file(&state), &rigs)
}

/// Key a radio for two seconds over CAT and report whether it transmitted.
/// Refused while connected with Station PTT running (the port is in use).
#[tauri::command]
pub async fn test_rig_key(state: State<'_, AppState>, rig: Rig) -> Result<crate::rigs::KeyTest, String> {
    if lock(&state.engine).is_some() && lock(&state.settings).ptt.station.enabled {
        return Err("disconnect first: Station PTT is using the radio".into());
    }
    tauri::async_runtime::spawn_blocking(move || platform::test_rig_key(&rig)).await.map_err(|e| e.to_string())?
}

/// Connect to a radio and read its identity, without keying it.
#[tauri::command]
pub async fn test_rig(rig: Rig) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || platform::test_rig(&rig)).await.map_err(|e| e.to_string())?
}

/// The user's server list (seeded with the shipped list on first run).
#[tauri::command]
pub fn list_servers(state: State<'_, AppState>) -> Result<Vec<ServerEntry>, String> {
    crate::servers::load(&servers_file(&state))
}

/// Replace the server list. Returns the tidied list that was saved.
#[tauri::command]
pub fn save_servers(state: State<'_, AppState>, servers: Vec<ServerEntry>) -> Result<Vec<ServerEntry>, String> {
    let list = crate::servers::validate(&servers)?;
    crate::servers::save(&servers_file(&state), &list)?;
    Ok(list)
}

/// Compute the settings path inside `dir` (used at startup).
#[must_use]
pub fn path_in(dir: &std::path::Path) -> std::path::PathBuf {
    settings_path(dir)
}
