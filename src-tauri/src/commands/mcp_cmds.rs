//! MCP management commands.

use tauri::State;

use crate::error::AppError;
use crate::mcp::registry::McpRegistry;

#[tauri::command]
pub async fn mcp_connect(registry: State<'_, McpRegistry>, name: String, command: String, args: Vec<String>) -> Result<(), AppError> {
    registry.connect(&name, &command, &args, &[])?;
    Ok(())
}

#[tauri::command]
pub async fn mcp_disconnect(registry: State<'_, McpRegistry>, name: String) -> Result<(), AppError> {
    registry.disconnect(&name)?;
    Ok(())
}

#[tauri::command]
pub async fn mcp_list_tools(registry: State<'_, McpRegistry>) -> Result<Vec<(String, serde_json::Value)>, AppError> {
    registry.get_tools()
}
