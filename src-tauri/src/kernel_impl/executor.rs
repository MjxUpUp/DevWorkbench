//! `Executor` implementation routing graph nodes to DevWorkbench subsystems.
//!
//! - Agent nodes → `spawn_pty_agent` (opaque CLI), then await completion by
//!   polling the session row in SQLite (the existing wait-thread updates it).
//! - Gate nodes → `quality::forge::run_forge_gate` (or HonestyVerifier for the
//!   "honesty" gate).

use std::sync::Arc;
use tauri::Manager;

use async_trait::async_trait;
use futures::stream::BoxStream;
use kernel_compose::graph::{AgentChunk, AgentNodeSpec, Executor, GateNode};
use kernel_core::{AgentInput, ToolContext};
use serde_json::{json, Value};

use crate::agents::pty::AgentProcesses;
use crate::db::DbState;
use crate::error::AppError;
use crate::kernel_impl::mcp_tool::McpTool;
use crate::kernel_impl::opaque_agent::OpaqueAgent;
use crate::kernel_impl::react_agent::{GlmChatModel, ReactAgent, ToolRegistry};
use crate::kernel_impl::skill_tool::SkillTool;
use crate::mcp::registry::McpRegistry;
use crate::models::AgentType;
use crate::quality;

/// The host Executor — bridges kernel-compose graph nodes to real subsystems.
pub struct KernelExecutor {
    app: tauri::AppHandle,
    processes: Arc<AgentProcesses>,
    db: DbState,
}

impl KernelExecutor {
    pub fn new(app: tauri::AppHandle, processes: Arc<AgentProcesses>, db: DbState) -> Self {
        Self { app, processes, db }
    }
}

#[async_trait]
impl Executor for KernelExecutor {
    fn run_agent(
        &self,
        spec: &AgentNodeSpec,
        input: Value,
        working_dir: Option<String>,
    ) -> Result<BoxStream<'static, Result<AgentChunk, String>>, String> {
        let prompt = spec
            .prompt
            .clone()
            .or_else(|| input.as_str().map(String::from))
            .ok_or_else(|| "agent node has no prompt".to_string())?;

        let project_path = working_dir.clone().unwrap_or_else(|| ".".into());
        let agent_input = AgentInput {
            prompt,
            working_dir: Some(project_path.clone()),
            model: spec.model.clone(),
            resume_from: spec.resume_from.clone(),
        };

        // Dual-mode dispatch: a known CLI spec resolves to an opaque agent
        // (claude/codex/gemini/…); anything else is a transparent ReactAgent
        // (self-built, kernel-controlled LLM + tool loop). Both implement
        // kernel_core::Agent and produce the same AgentEvent stream, which we
        // map onto AgentChunk for the graph runner. This closes the gap where
        // the executor bypassed OpaqueAgent/ReactAgent entirely and spawned
        // the CLI directly + polled the DB.
        let agent: Box<dyn kernel_core::Agent> = match AgentType::from_spec(&spec.agent) {
            Some(at) => Box::new(OpaqueAgent::new(
                self.app.clone(),
                self.processes.clone(),
                self.db.clone(),
                at,
            )),
            None => {
                let mcp = self.app.try_state::<McpRegistry>();
                Box::new(build_react_agent(
                    spec.model.as_deref(),
                    mcp.as_deref(),
                    &project_path,
                    None,
                )?)
            }
        };

        let event_stream = agent.run(agent_input).map_err(|e| e.to_string())?;
        Ok(Box::pin(map_agent_to_chunks(event_stream)))
    }

    async fn run_gate(
        &self,
        gate: &GateNode,
        input: Value,
        working_dir: Option<String>,
    ) -> Result<Value, String> {
        let project = working_dir.unwrap_or_else(|| ".".into());
        let path = std::path::Path::new(&project);

        match gate.gate.as_str() {
            "forge" => {
                let path_clone = path.to_path_buf();
                let result = tokio::task::spawn_blocking(move || {
                    quality::forge::run_forge_gate(&path_clone)
                })
                .await
                .map_err(|e| format!("forge join: {e}"))?;
                let json_val = match result {
                    Ok(report) => serde_json::to_value(&report)
                        .unwrap_or_else(|_| json!({"status": "unknown"})),
                    Err(AppError::ForgeNotInstalled) => {
                        // Graceful skip — forge missing is not a graph failure.
                        json!({"gate": "forge", "status": "skipped", "note": "forge not installed"})
                    }
                    Err(e) => return Err(e.to_string()),
                };
                Ok(json_val)
            }
            "honesty" => {
                // Post-hoc honesty audit — shared with OpaqueAgent's Done path
                // (one function so the gate node and the opaque agent cannot
                // drift apart). Scans the uncommitted diff for assertion
                // weakening, sanity-checks the compile env (Rust), claim-vs-proof.
                let claim = input.as_str().unwrap_or("").to_string();
                let project_path = path.to_path_buf();
                let result = tokio::task::spawn_blocking(move || {
                    crate::kernel_impl::honesty::audit_project(&project_path, &claim)
                })
                .await
                .map_err(|e| format!("honesty join: {e}"))?;
                Ok(result)
            }
            other => Err(format!("unknown gate '{other}'")),
        }
    }
}

// ---------------------------------------------------------------------------
// Dual-mode dispatch helpers
// ---------------------------------------------------------------------------

/// Build a transparent ReactAgent, resolving credentials from the user's
/// `providers.toml` (gap-②). The default model is `glm-4.6` — the strongest
/// tool-calling GLM on the Anthropic-compatible endpoint — overridable via
/// `spec.model`. Flagship models stay on the opaque path (claude/codex/gemini)
/// where the user selects them.
///
/// If no enabled+keyed provider serves the requested model (e.g. the user
/// hasn't filled an API key yet), we fall back to an empty-key default Z.AI
/// model: the agent still CONSTRUCTS (so the graph run doesn't crash), but GLM
/// calls fail at request time with 401 — the honest signal that Settings →
/// Providers needs a key.
pub(crate) fn build_react_agent(
    model: Option<&str>,
    mcp: Option<&McpRegistry>,
    working_dir: &str,
    conversation_id: Option<&str>,
) -> Result<ReactAgent, String> {
    let model_id = model.unwrap_or("glm-4.6").to_string();
    let data_dir = crate::commands::projects::dirs_home().join(".dev-workbench");
    let (endpoint, api_key, resolved_model) =
        match crate::config::providers::load_providers_config(&data_dir)
            .ok()
            .and_then(|c| crate::config::providers::resolve_provider(&c, &model_id))
        {
            Some(r) => (r.endpoint, r.api_key, r.model),
            None => (
                "https://open.bigmodel.cn/api/anthropic".to_string(),
                String::new(),
                model_id,
            ),
        };
    let chat = GlmChatModel::new(endpoint, api_key, resolved_model);

    // Build the tool registry: skills (always, from ~/.dev-workbench/skills) +
    // MCP tools (when a registry is available). An empty registry leaves the
    // agent chat-only; a populated one activates the tool loop + ToolCall
    // events end-to-end (the tool loop + react_chat mapping are already wired
    // — only the registry was empty before).
    let mut registry = ToolRegistry::new();
    let skills_dir = data_dir.join("skills");
    for skill in SkillTool::load_dir(&skills_dir) {
        registry.push(skill);
    }
    if let Some(reg) = mcp {
        // get_tools() is synchronous stdio I/O — acceptable as a one-shot cost
        // at agent construction. A slow/unreachable server is logged + skipped
        // inside the registry, never fatal.
        if let Ok(all) = reg.get_tools() {
            for (server, list_json) in all {
                if let Some(client) = reg.get_client(&server) {
                    for tool in McpTool::from_list_result(&server, &list_json, client) {
                        registry.push(tool);
                    }
                }
            }
        }
    }

    let ctx = ToolContext {
        working_dir: Some(working_dir.to_string()),
        conversation_id: conversation_id.map(|s| s.to_string()),
    };
    Ok(ReactAgent::new(
        chat,
        registry,
        "You are a Dev Workbench kernel agent. Complete the task concisely.",
    )
    .with_context(ctx))
}

/// Map a kernel-core `AgentEvent` stream onto graph `AgentChunk`s:
/// - `Token` → `Delta` (forwarded to the frontend as `NodeOutput`)
/// - `Done`  → `Final` (becomes the node's output value, propagated to successors)
/// - `ToolCall`/`FileChanged`/`TurnBoundary` → dropped (the runner only
///   forwards textual deltas; these are agent-internal observations)
/// - `Err`   → stream error (fails the node)
fn map_agent_to_chunks(
    events: BoxStream<'static, Result<kernel_core::AgentEvent, kernel_core::Error>>,
) -> impl futures::Stream<Item = Result<AgentChunk, String>> {
    use futures::StreamExt;
    events.filter_map(|ev_res| async move {
        match ev_res {
            Ok(kernel_core::AgentEvent::Token(t)) => {
                Some(Ok(AgentChunk::Delta(Value::String(t))))
            }
            Ok(kernel_core::AgentEvent::Done(outcome)) => {
                Some(Ok(AgentChunk::Final(outcome_to_value(outcome))))
            }
            Ok(_) => None,
            Err(e) => Some(Err(e.to_string())),
        }
    })
}

/// Serialize an `AgentOutcome` as the node's final JSON value.
fn outcome_to_value(o: kernel_core::AgentOutcome) -> Value {
    json!({
        "status": format!("{:?}", o.status).to_lowercase(),
        "output": o.output_summary,
        "files_changed": o.files_changed,
        "exit_code": o.exit_code,
        "honesty": o.honesty,
    })
}
