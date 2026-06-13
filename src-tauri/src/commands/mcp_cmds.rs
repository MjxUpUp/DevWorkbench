//! MCP management commands.

use tauri::State;

use crate::error::AppError;
use crate::mcp::registry::McpRegistry;

/// Connect to an MCP server by spawning its process and performing the
/// `initialize` handshake. The optional `env` is passed to the child process.
#[tauri::command]
pub async fn mcp_connect(
    registry: State<'_, McpRegistry>,
    name: String,
    command: String,
    args: Vec<String>,
    env: Option<Vec<(String, String)>>,
) -> Result<(), AppError> {
    let env = env.unwrap_or_default();
    registry.connect(&name, &command, &args, &env)?;
    Ok(())
}

/// Disconnect and remove a named MCP server.
#[tauri::command]
pub async fn mcp_disconnect(registry: State<'_, McpRegistry>, name: String) -> Result<(), AppError> {
    registry.disconnect(&name)?;
    Ok(())
}

/// List all tools from all connected servers.
/// Returns `(server_name, tools_json)` pairs.
#[tauri::command]
pub async fn mcp_list_tools(
    registry: State<'_, McpRegistry>,
) -> Result<Vec<(String, serde_json::Value)>, AppError> {
    registry.get_tools()
}

/// Invoke a tool on a specific connected MCP server.
///
/// `arguments` is the JSON object passed as the tool's `arguments` field.
/// Returns the raw JSON `result` from the server's `tools/call` response.
#[tauri::command]
pub async fn mcp_call_tool(
    registry: State<'_, McpRegistry>,
    server_name: String,
    tool_name: String,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    registry.call_tool(&server_name, &tool_name, arguments)
}
