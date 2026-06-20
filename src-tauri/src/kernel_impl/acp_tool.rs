//! C1 — `dispatch_acp_agent` tool: the kernel agent delegates a self-contained
//! sub-task to an EXTERNAL ACP-speaking coding agent (`npx
//! @zed-industries/codex-acp`, Claude Code via ACP, …) over stdio JSON-RPC.
//! Mirrors deer-flow's `tools/builtins/invoke_acp_agent_tool.py` and is the
//! sibling of [`crate::kernel_impl::react_agent::SubAgentTool`] — but where
//! `dispatch_subagent` runs an IN-PROCESS kernel child, this drives a SEPARATE
//! coding agent the kernel cannot itself become. The protocol driving lives in
//! [`crate::acp::client`]; this file is the thin [`Tool`] adapter (arg parsing
//! + result framing).

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use kernel_core::{Error, Tool, ToolContext, ToolInfo};

use crate::acp::client::run_acp_agent;

/// Default wall-clock cap (seconds) for one external ACP agent run. An ACP turn
/// is bounded — the agent resolves `session/prompt` with a `PromptResponse` —
/// but a wedged external agent must not hang the parent kernel turn. Generous:
/// real coding turns run minutes. See [`crate::acp::client::AcpError::Timeout`]
/// for the timed-out-child leak caveat (accepted v1 limitation).
const DEFAULT_ACP_TIMEOUT_SECS: u64 = 600;

/// The `dispatch_acp_agent` tool. Stateless beyond its timeout — no model handle
/// (the external agent brings its own), no registry. Construct via [`Default`]
/// from `build_react_agent`.
pub struct AcpAgentTool {
    timeout: Duration,
}

impl Default for AcpAgentTool {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_ACP_TIMEOUT_SECS),
        }
    }
}

impl AcpAgentTool {
    /// Build with a non-default timeout (tests inject a short cap).
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            timeout: Duration::from_secs(timeout_secs.max(1)),
        }
    }
}

#[async_trait]
impl Tool for AcpAgentTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "dispatch_acp_agent".into(),
            description: "把一个自包含子任务派给一个外部的、说 ACP 协议的编码 agent（如 \
                `npx @zed-industries/codex-acp`、Claude Code 经 ACP）执行，返回其文字结论。\
                用于让本 agent 委托给它自身无法成为的另一种编码 agent。该外部 agent 在当前\
                工作目录运行，其权限请求会被自动批准（本 agent 是授权方）。".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "启动外部 ACP agent 的命令行，如 `npx @zed-industries/codex-acp`"
                    },
                    "task": {
                        "type": "string",
                        "description": "派给外部 agent 的自包含子任务"
                    }
                },
                "required": ["command", "task"]
            }),
        }
    }

    async fn invoke(&self, args: &str, ctx: &ToolContext) -> Result<String, Error> {
        let parsed = serde_json::from_str::<serde_json::Value>(args).ok();
        let command = parsed
            .as_ref()
            .and_then(|v| v.get("command").and_then(|s| s.as_str()).map(str::to_owned))
            .ok_or_else(|| Error::Agent("dispatch_acp_agent 需要参数 {command: string}".into()))?;
        let task = parsed
            .as_ref()
            .and_then(|v| v.get("task").and_then(|s| s.as_str()).map(str::to_owned))
            .ok_or_else(|| Error::Agent("dispatch_acp_agent 需要参数 {task: string}".into()))?;
        if command.trim().is_empty() {
            return Err(Error::Agent("dispatch_acp_agent 的 command 不能为空".into()));
        }
        if task.trim().is_empty() {
            return Err(Error::Agent("dispatch_acp_agent 的 task 不能为空".into()));
        }

        // The external agent runs in THIS turn's working dir — same cwd the
        // kernel agent itself operates in, so the delegate sees the same
        // project. Fall back to the process cwd only when the turn has no
        // working_dir (shouldn't happen for a kernel-bound session).
        let cwd: PathBuf = ctx
            .working_dir
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        match run_acp_agent(&command, &cwd, &task, self.timeout.as_secs()).await {
            Ok(r) => {
                let body = if r.text.trim().is_empty() {
                    // No streamed answer text — the agent may have done tool work
                    // then ended with an empty message. Surface stop_reason so
                    // the parent isn't blind to what happened.
                    format!("(外部 agent 无文字输出; 停止原因: {})", r.stop_reason)
                } else {
                    r.text
                };
                Ok(format!("[ACP agent 结论] {body}"))
            }
            Err(e) => {
                // Surface a run failure as a tool RESULT (not an error) so the
                // parent can adapt — retry, fall back to doing it inline, or
                // pick a different command. Bad args already returned Err above.
                // This is the same contract as dispatch_subagent.
                log::warn!("[acp] dispatch failed for command '{command}': {e}");
                Ok(format!("[ACP agent 失败: {e}]"))
            }
        }
    }

    /// The external coding agent can mutate files in the workspace, so this is
    /// NOT a read-only tool — the kernel surfaces it alongside BashTool/Write,
    /// not with the read-only search tools.
    fn is_read_only(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tool's `info` is the contract the model sees: name + required-arg
    /// schema. Lock both so a rename or a dropped `required` entry fails here
    /// rather than silently breaking the model's tool calls.
    #[test]
    fn info_declares_command_and_task_required() {
        let info = AcpAgentTool::default().info();
        assert_eq!(info.name, "dispatch_acp_agent");
        let required = info
            .parameters_schema
            .get("required")
            .and_then(|r| r.as_array())
            .expect("required array present");
        let req: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(req.contains(&"command"), "command must be required: {req:?}");
        assert!(req.contains(&"task"), "task must be required: {req:?}");
    }

    /// Missing args → `Err` (the parent's tool loop treats Err as a hard tool
    /// failure, appropriate for a malformed call — distinct from a run failure,
    /// which is surfaced as an Ok result string below). The arg-validation path
    /// returns before any subprocess work, but `invoke` is async, so this drives
    /// the future on a tokio runtime.
    #[tokio::test]
    async fn invoke_errors_on_missing_args() {
        let tool = AcpAgentTool::default();
        let ctx = ToolContext::default();
        // Empty command.
        let err = tool
            .invoke(r#"{"command":"","task":"x"}"#, &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Agent(_)));
        // Missing task entirely.
        let err = tool
            .invoke(r#"{"command":"npx codex-acp"}"#, &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Agent(_)));
        // Malformed JSON.
        let err = tool.invoke("not json", &ctx).await.unwrap_err();
        assert!(matches!(err, Error::Agent(_)));
    }

    /// Default timeout is the documented cap; `new` honors an explicit override
    /// (clamped to ≥1s so a 0 never produces a sub-second instant timeout).
    #[test]
    fn default_and_custom_timeout() {
        assert_eq!(AcpAgentTool::default().timeout.as_secs(), DEFAULT_ACP_TIMEOUT_SECS);
        assert_eq!(AcpAgentTool::new(30).timeout.as_secs(), 30);
        assert_eq!(AcpAgentTool::new(0).timeout.as_secs(), 1, "0 clamps to 1s");
    }
}
