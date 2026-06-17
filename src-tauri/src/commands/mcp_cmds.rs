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

/// Toggle a server's `enabled` flag in `mcp-servers.toml` AND sync the live
/// registry: enabling connects (handshake), disabling disconnects. Lets the
/// user mute a server without losing its config. Errors if the config file or
/// the named server is absent.
#[tauri::command]
pub async fn mcp_set_enabled(
    registry: State<'_, McpRegistry>,
    project_path: String,
    name: String,
    enabled: bool,
) -> Result<(), AppError> {
    let config_path = std::path::Path::new(&project_path).join("mcp-servers.toml");
    let mut config = if config_path.is_file() {
        crate::config::mcp::load_mcp_config(&config_path)?
    } else {
        return Err(AppError::Config("mcp-servers.toml not found".into()));
    };
    if !crate::config::mcp::set_server_enabled(&mut config, &name, enabled) {
        return Err(AppError::Config(format!("server '{}' not in config", name)));
    }
    // Snapshot the fields before save_mcp_config borrows `config` again.
    let (command, args, env_map) = {
        let s = config.servers.iter().find(|s| s.name == name).expect("just set enabled");
        (s.command.clone(), s.args.clone(), s.env.clone())
    };
    crate::config::mcp::save_mcp_config(&config, &config_path)?;
    if enabled {
        let env: Vec<(String, String)> = env_map.into_iter().collect();
        let _ = registry.disconnect(&name); // fresh connect if it was live
        registry.connect(&name, &command, &args, &env)?;
    } else {
        registry.disconnect(&name)?;
    }
    Ok(())
}

/// Replace a server's command/args/env in `mcp-servers.toml` and reconnect the
/// live registry when it's enabled. Errors if the config file or server absent.
#[tauri::command]
pub async fn mcp_update_server(
    registry: State<'_, McpRegistry>,
    project_path: String,
    name: String,
    command: String,
    args: Vec<String>,
    env: Option<Vec<(String, String)>>,
) -> Result<(), AppError> {
    let config_path = std::path::Path::new(&project_path).join("mcp-servers.toml");
    let mut config = if config_path.is_file() {
        crate::config::mcp::load_mcp_config(&config_path)?
    } else {
        return Err(AppError::Config("mcp-servers.toml not found".into()));
    };
    let env_map: std::collections::HashMap<String, String> =
        env.unwrap_or_default().into_iter().collect();
    if !crate::config::mcp::update_server(&mut config, &name, command, args, env_map) {
        return Err(AppError::Config(format!("server '{}' not in config", name)));
    }
    // Read back the (now-updated) fields + whether to reconnect.
    let (reconnect, command, args, env_map) = {
        let s = config.servers.iter().find(|s| s.name == name).expect("just updated");
        (s.enabled, s.command.clone(), s.args.clone(), s.env.clone())
    };
    crate::config::mcp::save_mcp_config(&config, &config_path)?;
    if reconnect {
        let env: Vec<(String, String)> = env_map.into_iter().collect();
        let _ = registry.disconnect(&name);
        registry.connect(&name, &command, &args, &env)?;
    }
    Ok(())
}

/// Delete a server from `mcp-servers.toml` and disconnect it from the live
/// registry. Idempotent — a missing name (or a project with no config file) is
/// a no-op, not an error.
#[tauri::command]
pub async fn mcp_delete_server(
    registry: State<'_, McpRegistry>,
    project_path: String,
    name: String,
) -> Result<(), AppError> {
    let config_path = std::path::Path::new(&project_path).join("mcp-servers.toml");
    if !config_path.is_file() {
        return Ok(()); // nothing to delete from
    }
    let mut config = crate::config::mcp::load_mcp_config(&config_path)?;
    if !crate::config::mcp::remove_server(&mut config, &name) {
        return Ok(()); // not present — idempotent
    }
    crate::config::mcp::save_mcp_config(&config, &config_path)?;
    registry.disconnect(&name)?;
    Ok(())
}

/// Reconnect every enabled server from the project's `mcp-servers.toml`. Call
/// this when a project opens so servers the user previously installed
/// (`mcp_install_preset`) are live again without re-adding them. Returns the
/// count newly connected. Per-server failures are logged, never fatal.
#[tauri::command]
pub async fn mcp_load_enabled(
    registry: State<'_, McpRegistry>,
    project_path: String,
) -> Result<usize, AppError> {
    let config_path = std::path::Path::new(&project_path).join("mcp-servers.toml");
    if !config_path.is_file() {
        return Ok(0);
    }
    let config = crate::config::mcp::load_mcp_config(&config_path)?;
    registry.connect_from_config(&config)
}
