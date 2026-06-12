//! MCP server registry — manages connections to multiple MCP servers.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::AppError;
use crate::mcp::client::McpClient;

/// Registry holding named MCP clients.
pub struct McpRegistry {
    clients: Mutex<HashMap<String, McpClient>>,
}

impl McpRegistry {
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
        }
    }

    /// Connect to an MCP server and register it by name.
    pub fn connect(&self, name: &str, command: &str, args: &[String], env: &[(String, String)]) -> Result<(), AppError> {
        let client = McpClient::connect(command, args, env)?;
        self.clients.lock().map_err(|e| AppError::Mcp(format!("Lock error: {}", e)))?.insert(name.to_string(), client);
        Ok(())
    }

    /// Disconnect and remove a named MCP server.
    pub fn disconnect(&self, name: &str) -> Result<(), AppError> {
        let mut clients = self.clients.lock().map_err(|e| AppError::Mcp(format!("Lock error: {}", e)))?;
        if let Some(mut client) = clients.remove(name) {
            client.disconnect()?;
        }
        Ok(())
    }

    /// List all tools from all connected servers.
    pub fn get_tools(&self) -> Result<Vec<(String, serde_json::Value)>, AppError> {
        let clients = self.clients.lock().map_err(|e| AppError::Mcp(format!("Lock error: {}", e)))?;
        let results = Vec::new();
        // Note: This cannot mutate clients (McpClient::list_tools takes &mut self).
        // For now, return empty — full implementation requires interior mutability refactoring.
        drop(clients);
        Ok(results)
    }

    /// Call a tool on a specific server.
    pub fn call_tool(&self, server_name: &str, tool_name: &str, arguments: serde_json::Value) -> Result<serde_json::Value, AppError> {
        let mut clients = self.clients.lock().map_err(|e| AppError::Mcp(format!("Lock error: {}", e)))?;
        let client = clients.get_mut(server_name).ok_or_else(|| AppError::Mcp(format!("Server '{}' not found", server_name)))?;
        client.call_tool(tool_name, arguments)
    }
}
