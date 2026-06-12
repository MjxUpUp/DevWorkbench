//! SSE → Tauri event bridge — subscribes to OwnAgent SSE and forwards as Tauri events.

use crate::error::AppError;

/// Subscribe to the OwnAgent SSE event stream and forward events via Tauri.
///
/// This is a skeleton — full implementation requires async runtime integration.
pub fn subscribe_sse(_port: u16, _app_handle: tauri::AppHandle) -> Result<(), AppError> {
    // TODO: Use reqwest::stream to subscribe to SSE endpoint
    // TODO: Parse SSE events and emit as Tauri events via app_handle.emit()
    log::info!("SSE subscription skeleton — not yet implemented");
    Ok(())
}
