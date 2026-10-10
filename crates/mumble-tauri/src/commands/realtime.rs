//! Real-time auxiliary commands: push notifications, typing indicators,
//! and link previews.

use crate::state::AppState;

/// Update the per-channel push notification mute preferences on the server.
#[tauri::command]
pub(crate) async fn send_push_update(
    state: tauri::State<'_, AppState>,
    muted_channels: Vec<u32>,
) -> Result<(), String> {
    state.send_push_update(muted_channels).await
}

/// Send a live subscribe-push registration (or update) to the server.
#[tauri::command]
pub(crate) async fn send_subscribe_push(
    state: tauri::State<'_, AppState>,
    muted_channels: Vec<u32>,
) -> Result<(), String> {
    state.send_subscribe_push(muted_channels).await
}
