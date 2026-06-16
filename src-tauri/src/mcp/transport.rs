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

        // On Windows, suppress the console window for MCP server processes
        // (same flag as the agent PTY path in agents/pty.rs).
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the JSON-RPC 2.0 request envelope serializes with the fields
    /// and shape the MCP spec requires: `jsonrpc`, `id`, `method`, and `params`
    /// present only when supplied. Guards against silent drift in field naming
    /// (e.g. camelCase) that would make the server reject every request.
    #[test]
    fn jsonrpc_request_envelope_shape_with_params() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 7,
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name": "grep", "arguments": {"q": "foo"}})),
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 7);
        assert_eq!(v["method"], "tools/call");
        assert_eq!(v["params"]["name"], "grep");
        assert_eq!(v["params"]["arguments"]["q"], "foo");
    }

    /// When `params` is None the field must be omitted entirely (spec servers
    /// reject unknown fields). Guards the `skip_serializing_if` attribute.
    #[test]
    fn jsonrpc_request_envelope_omits_params_when_none() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "tools/list".to_string(),
            params: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["method"], "tools/list");
        assert!(
            v.get("params").is_none(),
            "params must be absent when None; got: {s}"
        );
    }

    /// A server response carrying an error object must parse into `JsonRpcError`
    /// so `send_request` can surface the message. Guards the deserialize path.
    #[test]
    fn jsonrpc_response_parses_error_object() {
        let raw = r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"Method not found"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(raw).unwrap();
        let err = resp.error.expect("error object present");
        assert_eq!(err.message, "Method not found");
        assert!(resp.result.is_none(), "result must be absent when error is set");
    }
}
