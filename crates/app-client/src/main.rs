//! `RemoteRadioClient` desktop application entry point.
//!
//! Thin binary: it supplies its own Tauri context (built from this crate's
//! `tauri.conf.json`, giving the Client its distinct product identity) and
//! delegates all behaviour to the shared `mumble_tauri_lib`, selecting
//! [`Mode::Client`].
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// These Tauri plugins are direct dependencies of this binary only so Tauri can
// resolve their ACL/permission schemas for `app-client`'s capabilities (without
// a direct dep the schema does not resolve and every plugin IPC is denied,
// hanging the app on the loading splash). They are not called from Rust here -
// the shared `mumble_tauri_lib` registers and uses them - so bind each as `_`
// to acknowledge the dependency and silence `unused_crate_dependencies`.
use tauri_plugin_deep_link as _;
use tauri_plugin_dialog as _;
use tauri_plugin_global_shortcut as _;
use tauri_plugin_notification as _;
use tauri_plugin_opener as _;
use tauri_plugin_process as _;
use tauri_plugin_store as _;
use tauri_plugin_updater as _;
use tauri_plugin_window_state as _;

use mumble_tauri_lib::Mode;

fn main() {
    mumble_tauri_lib::platform::pre_init();
    mumble_tauri_lib::run_with(tauri::generate_context!(), Mode::Client);
}
