//! `RemoteRadioStation` desktop application entry point.
//!
//! Thin binary: it supplies its own Tauri context (built from this crate's
//! `tauri.conf.json`, giving the Station its distinct product identity) and
//! delegates all behaviour to the shared `mumble_tauri_lib`, selecting
//! [`Mode::Station`].
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use mumble_tauri_lib::Mode;

fn main() {
    mumble_tauri_lib::platform::pre_init();
    mumble_tauri_lib::run_with(tauri::generate_context!(), Mode::Station);
}
