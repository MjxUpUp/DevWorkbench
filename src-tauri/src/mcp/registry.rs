//! MCP server registry — manages connections to multiple MCP servers.
//!
//! Stores each connection as `Arc<Mutex<McpClient>>` so individual MCP tools can
//! hold a cheap cloneable handle and call `&mut self` methods (list_tools /
//! call_tool require sequential stdin/stdout I/O).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::error::AppError;
use crate::mcp::client::McpClient;

/// A shareable handle to one MCP server connection.
pub type SharedMcpClient = Arc<Mutex<McpClient>>;

/// Registry holding named MCP clients.
pub struct McpRegistry {
    clients: Mutex<HashMap<String, SharedMcpClient>>,
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl McpRegistry {
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
        }
    }

    /// Connect to an MCP server and register it by name.
    pub fn connect(
        &self,
        name: &str,
        command: &str,
        args: &[String],
        env: &[(String, String)],
    ) -> Result<(), AppError> {
        let client = McpClient::connect(command, args, env)?;
        self.clients
            .lock()
            .map_err(|e| AppError::Mcp(format!("Lock error: {}", e)))?
            .insert(name.to_string(), Arc::new(Mutex::new(client)));
        Ok(())
    }

    /// Disconnect and remove a named MCP server.
    pub fn disconnect(&self, name: &str) -> Result<(), AppError> {
        let mut clients = self
            .clients
            .lock()
            .map_err(|e| AppError::Mcp(format!("Lock error: {}", e)))?;
        if let Some(client) = clients.remove(name) {
            // If this is the last strong ref, shut the process down.
            if let Ok(mut c) = client.lock() {
                let _ = c.disconnect();
            }
        }
        Ok(())
    }

    /// Borrow a shared handle to a named client (for tool invocation).
    pub fn get_client(&self, name: &str) -> Option<SharedMcpClient> {
        self.clients
            .lock()
            .ok()?
            .get(name)
            .map(Arc::clone)
    }

    /// List connected server names.
    pub fn server_names(&self) -> Vec<String> {
        self.clients
            .lock()
            .map(|c| c.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// List all tools from all connected servers.
    pub fn get_tools(&self) -> Result<Vec<(String, serde_json::Value)>, AppError> {
        let clients = self
            .clients
            .lock()
            .map_err(|e| AppError::Mcp(format!("Lock error: {}", e)))?;
        let mut results = Vec::new();
        for (name, shared) in clients.iter() {
            match shared.lock() {
                Ok(mut client) => match client.list_tools() {
                    Ok(tools) => results.push((name.clone(), tools)),
                    Err(e) => log::warn!("MCP server '{}' list_tools failed: {}", name, e),
                },
                Err(e) => log::warn!("MCP server '{}' lock failed: {}", name, e),
            }
        }
        Ok(results)
    }

    /// Call a tool on a specific server.
    pub fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, AppError> {
        let clients = self
            .clients
            .lock()
            .map_err(|e| AppError::Mcp(format!("Lock error: {}", e)))?;
        let shared = clients
            .get(server_name)
            .ok_or_else(|| AppError::Mcp(format!("Server '{}' not found", server_name)))?
            .clone();
        drop(clients);
        // Release the registry lock before locking the client, so a slow tool
        // call doesn't block other registry operations.
        let mut client = shared
            .lock()
            .map_err(|e| AppError::Mcp(format!("client lock: {}", e)))?;
        client.call_tool(tool_name, arguments)
    }

    /// Connect every enabled server from a parsed `mcp-servers.toml` config
    /// that isn't already connected. Used at project-open to reconnect servers
    /// the user previously installed (`mcp_install_preset` persists them), so
    /// the catalog survives a restart. Returns the count newly connected.
    /// Already-connected names are skipped (no duplicate handshake); a
    /// per-server failure is logged and skipped — one broken/unreachable
    /// server must never block the rest of the catalog.
    pub fn connect_from_config(
        &self,
        config: &crate::models::McpConfigFile,
    ) -> Result<usize, AppError> {
        let existing: std::collections::HashSet<String> =
            self.server_names().into_iter().collect();
        let mut connected = 0;
        for server in &config.servers {
            if !server.enabled {
                continue;
            }
            if existing.contains(&server.name) {
                continue;
            }
            let env: Vec<(String, String)> =
                server.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            match self.connect(&server.name, &server.command, &server.args, &env) {
                Ok(()) => connected += 1,
                Err(e) => log::warn!("MCP server '{}' auto-connect failed: {}", server.name, e),
            }
        }
        Ok(connected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{McpConfigFile, McpServerConfig};
    use std::collections::HashMap;

    fn server(name: &str, command: &str, enabled: bool) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            command: command.into(),
            args: vec![],
            env: HashMap::new(),
            enabled,
            target_agents: vec![],
        }
    }

    #[test]
    fn connect_from_config_skips_disabled_servers() {
        // A disabled server is never handed to connect() — so no process spawn
        // is attempted and the registry stays empty.
        let reg = McpRegistry::new();
        let config = McpConfigFile { servers: vec![server("off", "echo", false)] };
        let n = reg.connect_from_config(&config).unwrap();
        assert_eq!(n, 0);
        assert!(reg.server_names().is_empty());
    }

    #[test]
    fn connect_from_config_logs_and_skips_failing_server_without_aborting() {
        // An enabled server whose command can't start: connect() fails, the
        // error is logged + skipped (NOT propagated), and the count stays 0 —
        // one bad server doesn't poison the whole reconnect.
        let reg = McpRegistry::new();
        let config =
            McpConfigFile { servers: vec![server("bad", "no-such-cmd-xyz-12345", true)] };
        let n = reg.connect_from_config(&config).unwrap();
        assert_eq!(n, 0, "failing server skipped, not counted");
        assert!(reg.server_names().is_empty(), "failed connect left nothing registered");
    }
}
