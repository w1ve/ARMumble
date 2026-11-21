//! Remote.Radio Mumble application shell.
//!
//! A thin Tauri layer over `rr-client`: it owns the sound-card streams, the
//! rig hardware and the hotkeys, and pushes a status snapshot to the UI ten
//! times a second.

#![allow(unreachable_pub, reason = "internal modules of an application crate")]
#![allow(clippy::needless_pass_by_value, reason = "Tauri command arguments are extracted by value")]

#[cfg(desktop)]
mod cat;
mod commands;
mod devices;
mod engine;
mod keyer_udp;
#[cfg(desktop)]
#[path = "platform_desktop.rs"]
mod platform;
#[cfg(mobile)]
#[path = "platform_mobile.rs"]
mod platform;
mod rigs;
mod routing;
mod servers;
mod settings;
mod status;
mod stereo_test;

use tauri::Manager;

use engine::{lock, AppState};
use settings::Settings;

/// Start the application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let builder = tauri::Builder::default();
    #[cfg(desktop)]
    let builder = builder.plugin(platform::plugin());
    let result = builder
        .manage(AppState::default())
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            let path = commands::path_in(&dir);
            let state = app.state::<AppState>();
            let loaded = Settings::load(&path);
            platform::apply_hotkeys(app.handle(), &loaded.hotkeys);
            *lock(&state.settings) = loaded;
            *lock(&state.settings_path) = path;
            engine::restart_monitor(&state);
            engine::restart_otrsp(app.handle());
            let _levels = tauri::async_runtime::spawn(engine::levels_ticker(app.handle().clone()));
            let _keyer = tauri::async_runtime::spawn(engine::keyer_ticker(app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::load_settings,
            commands::save_settings,
            commands::list_devices,
            commands::device_format,
            commands::list_serial_ports,
            commands::connect,
            commands::disconnect,
            commands::join_channel,
            commands::set_ptt,
            commands::stereo_test,
            commands::listen_in,
            commands::whisper,
            commands::set_user_gain,
            commands::settings_location,
            commands::get_route,
            commands::list_servers,
            commands::save_servers,
            commands::load_rigs,
            commands::save_rigs,
            commands::test_rig,
            commands::test_rig_key,
        ])
        .run(tauri::generate_context!());
    if let Err(e) = result {
        tracing::error!("application error: {e}");
    }
}
