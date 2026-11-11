//! Desktop entry point.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Dependencies used by the library half of this package.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use rr_audio_cpal as _;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use tauri_plugin_global_shortcut as _;
use rr_audio as _;
use tauri as _;
use rr_client as _;
use rr_radio as _;
use serde as _;
use serde_json as _;
use tokio as _;
use tracing as _;
use tracing_subscriber as _;

fn main() {
    rr_app_lib::run();
}
