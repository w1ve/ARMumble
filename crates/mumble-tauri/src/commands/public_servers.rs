//! File-server capability probe.
//!
//! Remote.Radio Mumble is an amateur-radio-only client and deliberately does
//! **not** browse the public Mumble directory, so the former
//! `fetch_public_servers` command (and its XML directory parser) was removed.
//! What remains is the unrelated file-server capability probe, kept here.

/// Probe the file-server plugin's `GET /capabilities` endpoint.
///
/// Performed in Rust (rather than via browser `fetch`) so it works
/// regardless of the file-server's CORS allow-list configuration and
/// avoids preflight overhead.  Returns the raw JSON body on success;
/// the frontend deserialises into [`FileServerCapabilities`].
#[tauri::command]
pub(crate) async fn fetch_file_server_capabilities(base_url: String) -> Result<String, String> {
    let trimmed = base_url.trim_end_matches('/');
    let url = format!("{trimmed}/capabilities");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| format!("HTTP client init failed: {e}"))?;
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    response
        .text()
        .await
        .map_err(|e| format!("read body failed: {e}"))
}
