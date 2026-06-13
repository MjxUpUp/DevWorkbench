//! MCP management commands — connect/discover/call + market ops.

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
#[tauri::command]
pub async fn mcp_call_tool(
    registry: State<'_, McpRegistry>,
    server_name: String,
    tool_name: String,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    registry.call_tool(&server_name, &tool_name, arguments)
}

// ---------------------------------------------------------------------------
// Market / catalog surface
// ---------------------------------------------------------------------------

/// One tool advertised by one MCP server — the market "browse" row.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolListing {
    pub server: String,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Catalog of all tools across all connected MCP servers. The MCP Market's
/// "what can I use right now" view.
#[tauri::command]
pub async fn mcp_catalog(
    registry: State<'_, McpRegistry>,
) -> Result<Vec<McpToolListing>, AppError> {
    let raw = registry.get_tools()?;
    let mut listings = Vec::new();
    for (server, tools_json) in raw {
        if let Some(arr) = tools_json.get("tools").and_then(|t| t.as_array()) {
            for t in arr {
                listings.push(McpToolListing {
                    server: server.clone(),
                    name: t.get("name").and_then(|n| n.as_str()).unwrap_or("").into(),
                    description: t.get("description").and_then(|d| d.as_str()).unwrap_or("").into(),
                    input_schema: t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({"type":"object","properties":{}})),
                });
            }
        }
    }
    Ok(listings)
}

/// The names of all connected MCP servers.
#[tauri::command]
pub async fn mcp_servers(registry: State<'_, McpRegistry>) -> Result<Vec<String>, AppError> {
    Ok(registry.server_names())
}

/// Install an MCP server preset: connect it now AND persist its config to the
/// project's mcp-servers.toml so it's reconnected on restart. The config layer
/// (config/adapters) then distributes it into each agent's native format.
#[tauri::command]
pub async fn mcp_install_preset(
    registry: State<'_, McpRegistry>,
    project_path: String,
    name: String,
    command: String,
    args: Vec<String>,
    env: Option<Vec<(String, String)>>,
) -> Result<(), AppError> {
    // 1. Connect now (handshake) — fail fast if the server doesn't start.
    let env_vec = env.clone().unwrap_or_default();
    registry.connect(&name, &command, &args, &env_vec)?;

    // 2. Persist to mcp-servers.toml in the project dir.
    let config_path = std::path::Path::new(&project_path).join("mcp-servers.toml");
    let mut config = if config_path.is_file() {
        crate::config::mcp::load_mcp_config(&config_path)?
    } else {
        crate::models::McpConfigFile { servers: Vec::new() }
    };
    // Replace if a server with the same name already exists.
    config.servers.retain(|s| s.name != name);
    config.servers.push(crate::models::McpServerConfig {
        name: name.clone(),
        command,
        args,
        env: env_vec.into_iter().collect(),
        enabled: true,
        target_agents: Vec::new(),
    });
    crate::config::mcp::save_mcp_config(&config, &config_path)?;
    Ok(())
}
