//! `RemoteRadioClient` desktop application entry point.
//!
//! Thin binary: it supplies its own Tauri context (built from this crate's
//! `tauri.conf.json`, giving the Client its distinct product identity) and
//! delegates all behaviour to the shared `mumble_tauri_lib`, selecting
//! [`Mode::Client`].
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use mumble_tauri_lib::Mode;

fn main() {
    mumble_tauri_lib::platform::pre_init();
    mumble_tauri_lib::run_with(tauri::generate_context!(), Mode::Client);
}
