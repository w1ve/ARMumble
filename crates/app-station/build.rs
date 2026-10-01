//! Build script for the `RemoteRadioStation` binary.
//!
//! Runs `tauri-build` so `tauri::generate_context!()` in `main.rs` can embed
//! this crate's `tauri.conf.json` (distinct product identity) and the shared
//! frontend assets. The signal-bridge build and permission codegen are handled
//! by the shared `mumble-tauri` crate's own build script, so this one only
//! needs the standard Tauri build step.
fn main() {
    tauri_build::build();
}
