//! MCP client — wraps McpTransport with high-level protocol methods.

use serde_json::Value;

use crate::error::AppError;
use crate::mcp::transport::McpTransport;

/// High-level MCP client that speaks the Model Context Protocol.
pub struct McpClient {
    transport: McpTransport,
}

impl McpClient {
    /// Create a new client by spawning the MCP server process.
    pub fn connect(command: &str, args: &[String], env: &[(String, String)]) -> Result<Self, AppError> {
        let transport = McpTransport::spawn(command, args, env)?;
        Ok(Self { transport })
    }

    /// Send the `initialize` handshake.
    pub fn initialize(&mut self) -> Result<Value, AppError> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "dev-workbench",
                "version": env!("CARGO_PKG_VERSION")
            }
        });
        self.transport.send_request("initialize", Some(params))
    }

    /// List available tools from the server.
    pub fn list_tools(&mut self) -> Result<Value, AppError> {
        self.transport.send_request("tools/list", None)
    }

    /// Call a specific tool on the server.
    pub fn call_tool(&mut self, tool_name: &str, arguments: Value) -> Result<Value, AppError> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments
        });
        self.transport.send_request("tools/call", Some(params))
    }

    /// Shut down the connection.
    pub fn disconnect(&mut self) -> Result<(), AppError> {
        self.transport.shutdown()
    }
}
