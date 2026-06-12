//! MCP stdio transport — JSON-RPC 2.0 over stdin/stdout of a child process.

use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::AppError;

/// JSON-RPC 2.0 request envelope.
#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

/// JSON-RPC 2.0 response envelope.
#[derive(Deserialize, Debug)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    id: u64,
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

#[derive(Deserialize, Debug)]
struct JsonRpcError {
    #[allow(dead_code)]
    code: i64,
    message: String,
}

/// Manages a child process communicating via JSON-RPC 2.0 over stdin/stdout.
pub struct McpTransport {
    _child: Child,
    stdin: ChildStdin,
    stdout: std::io::BufReader<ChildStdout>,
    next_id: u64,
}

impl McpTransport {
    /// Spawn a child process and wire up stdin/stdout pipes.
    pub fn spawn(command: &str, args: &[String], env: &[(String, String)]) -> Result<Self, AppError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (k, v) in env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().map_err(|e| AppError::Mcp(format!("Failed to spawn MCP server: {}", e)))?;
        let stdin = child.stdin.take().ok_or_else(|| AppError::Mcp("Failed to open stdin".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| AppError::Mcp("Failed to open stdout".into()))?;

        Ok(Self {
            _child: child,
            stdin,
            stdout: std::io::BufReader::new(stdout),
            next_id: 1,
        })
    }

    /// Send a JSON-RPC request and read the response.
    pub fn send_request(&mut self, method: &str, params: Option<Value>) -> Result<Value, AppError> {
        let id = self.next_id;
        self.next_id += 1;

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        let mut line = serde_json::to_string(&request).map_err(|e| AppError::Mcp(format!("Serialize error: {}", e)))?;
        line.push('\n');

        use std::io::Write;
        self.stdin.write_all(line.as_bytes()).map_err(|e| AppError::Mcp(format!("Write error: {}", e)))?;
        self.stdin.flush().map_err(|e| AppError::Mcp(format!("Flush error: {}", e)))?;

        let mut response_line = String::new();
        use std::io::BufRead;
        self.stdout.read_line(&mut response_line).map_err(|e| AppError::Mcp(format!("Read error: {}", e)))?;

        let response: JsonRpcResponse =
            serde_json::from_str(&response_line).map_err(|e| AppError::Mcp(format!("Parse error: {}", e)))?;

        if let Some(err) = response.error {
            return Err(AppError::Mcp(format!("JSON-RPC error: {}", err.message)));
        }

        response.result.ok_or_else(|| AppError::Mcp("Empty result".into()))
    }

    /// Kill the child process gracefully.
    pub fn shutdown(&mut self) -> Result<(), AppError> {
        self._child.kill().map_err(|e| AppError::Mcp(format!("Kill error: {}", e)))
    }
}
