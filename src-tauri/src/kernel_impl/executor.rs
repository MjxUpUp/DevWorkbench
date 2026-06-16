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
                    Vec::new(),
                    Some(self.db.clone()),
                    crate::kernel_impl::hooks::PermissionMode::Default,
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
    history: Vec<kernel_core::Message>,
    db: Option<DbState>,
    mode: crate::kernel_impl::hooks::PermissionMode,
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
    // T7 experience flywheel: prepend prior quality-failure lessons so the
    // agent avoids repeating them. Computed before `db` moves into the cost
    // sink. Best-effort — a DB read failure just yields the bare prompt.
    let mut sys_prompt = String::from(
        "You are a Dev Workbench kernel agent. Complete the task concisely.",
    );
    if let Some(dbs) = db.as_ref() {
        if let Ok(conn) = dbs.get() {
            let hash = crate::activity::hash_project_path(working_dir);
            sys_prompt.push_str(&experience_prompt_suffix(&conn, &hash));
        }
    }
    let chat = GlmChatModel::new(endpoint, api_key, resolved_model)
        // P0 model orchestration: a process-wide breaker so a down GLM endpoint
        // fails fast instead of every session retrying into it, plus a cost sink
        // that records token usage + derived cost per request (conversation_id
        // acts as the session_id for cost attribution).
        .with_circuit(crate::kernel_impl::react_agent::shared_glm_circuit())
        .with_cost_sink(crate::cost::sink::optional_shared(
            db,
            "react_kernel",
            conversation_id.map(|s| s.to_string()),
        ));

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
    // Mount the kernel hook pipeline (the Forge task-guard / bash-guard /
    // assertion-check analogs). CommandGuard vetoes destructive shell commands
    // before they run; AssertionGuard watches file-write diffs for assertion
    // weakening after the fact. TaskGuard is deliberately deferred to v1.2: it
    // blocks every WriteFile unless an active Forge task is set, and the chat
    // path carries no task state yet — mounting it now would brick the agent's
    // own file-writing tools.
    let mut hooks = crate::kernel_impl::hooks::HookManager::new().with_mode(mode);
    hooks.register(Box::new(crate::kernel_impl::hooks::CommandGuardHook::default()));
    hooks.register(Box::new(crate::kernel_impl::hooks::AssertionGuardHook));
    Ok(ReactAgent::new(chat, registry, sys_prompt)
        .with_context(ctx)
        .with_history(history)
        .with_thinking(2048)
        .with_max_verify(1)
        // T9 per-step routing: rule-based glm-4-flash for low-stakes turns
        // (tool-result echoes, confirmations), glm-4.6 for planning/reasoning.
        // Same Z.AI provider → endpoint/key constant; route_step is a no-op for
        // non-GLM base models.
        .with_model_router(Arc::new(
            crate::kernel_impl::model_router::route_step,
        ))
        .with_hooks(Arc::new(hooks)))
}

/// Build the experience-flywheel suffix for the system prompt (v1.2 T7): up
/// to 3 prior `quality_failure` lessons from this project, so the agent avoids
/// repeating them. Empty when there are none (or the DB read fails) → no prompt
/// bloat.
fn experience_prompt_suffix(conn: &rusqlite::Connection, project_hash: &str) -> String {
    let entries = crate::knowledge::store::get_entries_for_project(conn, project_hash)
        .unwrap_or_default();
    let failures: Vec<_> = entries
        .iter()
        .filter(|e| e.category == "quality_failure")
        .take(3)
        .collect();
    if failures.is_empty() {
        return String::new();
    }
    let body = failures
        .iter()
        .map(|e| format!("- {}", e.title))
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n\n历史质量经验（避免重蹈覆辙）：\n{body}")
}

/// Map a kernel-core `AgentEvent` stream onto graph `AgentChunk`s.
///
/// - `Done` → `Final` (becomes the node's output value, propagated to graph
///   successors). NOT emitted as a `NodeOutput` — the terminal status surfaces
///   via `node_end` instead of a Result block.
/// - every other event → structured `ChatStreamEvent` wire blocks (via
///   `react_chat::map_agent_event`, the SAME mapping single-agent chat uses),
///   each serialized into `Delta(Value)`. This lets the workflow canvas render
///   text / tool_use / tool_result block cards — identical to chat — instead of
///   the old flat text-tail. `secs=0` is safe: only Result blocks consume secs,
///   and Done (the sole Result source) is handled above.
/// - `Err` → stream error (fails the node).
fn map_agent_to_chunks(
    events: BoxStream<'static, Result<kernel_core::AgentEvent, kernel_core::Error>>,
) -> impl futures::Stream<Item = Result<AgentChunk, String>> {
    use futures::StreamExt;
    events.flat_map(|ev_res| {
        let chunks: Vec<Result<AgentChunk, String>> = match ev_res {
            Ok(kernel_core::AgentEvent::Done(outcome)) => {
                vec![Ok(AgentChunk::Final(outcome_to_value(outcome)))]
            }
            Ok(other) => crate::agents::react_chat::map_agent_event(other, 0)
                .into_iter()
                .map(|w| {
                    Ok(AgentChunk::Delta(
                        serde_json::to_value(&w).unwrap_or(Value::Null),
                    ))
                })
                .collect(),
            Err(e) => vec![Err(e.to_string())],
        };
        futures::stream::iter(chunks)
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use kernel_core::{AgentEvent, AgentOutcome, AgentRunStatus, ToolCallEvent, ToolCallStatus};
    use std::path::PathBuf;

    /// Drive `map_agent_to_chunks` with a scripted event list and collect the
    /// emitted chunks (errors dropped — tests only feed Ok events).
    async fn collect_chunks(events: Vec<Result<AgentEvent, kernel_core::Error>>) -> Vec<AgentChunk> {
        let mut s = map_agent_to_chunks(Box::pin(futures::stream::iter(events)));
        let mut out = Vec::new();
        while let Some(c) = s.next().await {
            if let Ok(chunk) = c {
                out.push(chunk);
            }
        }
        out
    }

    fn kind_of(v: &Value) -> Option<&str> {
        v.get("kind").and_then(|k| k.as_str())
    }

    #[test]
    fn experience_prompt_suffix_lists_quality_failures_only() {
        use crate::db;
        use crate::knowledge::store::add_entry;
        use crate::models::{AgentType, KnowledgeEntry};
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = db::init_db(&tmp.path().join("t.db")).unwrap();
        let mk = |id: &str, cat: &str, title: &str| KnowledgeEntry {
            id: id.into(),
            project_hash: "h".into(),
            category: cat.into(),
            title: title.into(),
            content: "c".into(),
            source_agent: AgentType::ClaudeCode,
            source_session_id: None,
            source_type: "self_verify".into(),
            confidence: 0.8,
            created_at: "t".into(),
            updated_at: "t".into(),
            access_count: 0,
        };
        add_entry(&conn, &mk("k1", "quality_failure", "t.Fatal 被降级为 t.Log")).unwrap();
        add_entry(&conn, &mk("k2", "insight", "用 thiserror")).unwrap();
        let suffix = experience_prompt_suffix(&conn, "h");
        assert!(suffix.contains("t.Fatal"), "quality_failure must surface: {suffix}");
        assert!(
            !suffix.contains("thiserror"),
            "non-failure category excluded: {suffix}"
        );
    }

    #[tokio::test]
    async fn token_maps_to_text_delta() {
        let chunks = collect_chunks(vec![Ok(AgentEvent::Token("hi".into()))]).await;
        assert_eq!(chunks.len(), 1);
        match &chunks[0] {
            AgentChunk::Delta(v) => {
                assert_eq!(kind_of(v), Some("text"));
                assert_eq!(v["content"], "hi");
            }
            other => panic!("expected Delta, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn tool_call_started_and_succeeded_map_to_tool_use_and_result_deltas() {
        let chunks = collect_chunks(vec![
            Ok(AgentEvent::ToolCall(ToolCallEvent {
                tool: "Read".into(),
                arguments: r#"{"file_path":"/x"}"#.into(),
                status: ToolCallStatus::Started,
                result: None,
            })),
            Ok(AgentEvent::ToolCall(ToolCallEvent {
                tool: "Read".into(),
                arguments: "{}".into(),
                status: ToolCallStatus::Succeeded,
                result: None,
            })),
        ])
        .await;
        assert_eq!(chunks.len(), 2);
        match (&chunks[0], &chunks[1]) {
            (AgentChunk::Delta(use_v), AgentChunk::Delta(res_v)) => {
                assert_eq!(kind_of(use_v), Some("tool_use"));
                assert_eq!(use_v["name"], "Read");
                assert_eq!(kind_of(res_v), Some("tool_result"));
                assert_eq!(res_v["is_error"], false);
            }
            other => panic!("expected [Delta(tool_use), Delta(tool_result)], got {:?}", other),
        }
    }

    #[tokio::test]
    async fn done_maps_to_final_not_delta() {
        // Done must become Final (graph data-flow propagation), NOT a
        // Delta/Result block — the node terminal status surfaces via node_end.
        let chunks = collect_chunks(vec![Ok(AgentEvent::Done(AgentOutcome {
            status: AgentRunStatus::Completed,
            ..Default::default()
        }))])
        .await;
        assert_eq!(chunks.len(), 1, "got {chunks:?}");
        match &chunks[0] {
            AgentChunk::Final(v) => {
                assert_eq!(v["status"], "completed");
            }
            other => panic!("expected Final, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn file_changed_and_turn_boundary_emit_nothing() {
        // map_agent_event returns empty Vec for these (G3b deferred). The
        // workflow path inherits that — no spurious chunks, no panic.
        let chunks = collect_chunks(vec![
            Ok(AgentEvent::FileChanged(PathBuf::from("/x.rs"))),
            Ok(AgentEvent::TurnBoundary),
        ])
        .await;
        assert!(chunks.is_empty(), "got {chunks:?}");
    }

    #[tokio::test]
    async fn full_turn_sequence_produces_deltas_then_final() {
        // Token → text, ToolCall Started → tool_use, Succeeded → tool_result,
        // Done → Final. Order and structure must match chat's BlocksView input.
        let chunks = collect_chunks(vec![
            Ok(AgentEvent::Token("reading".into())),
            Ok(AgentEvent::ToolCall(ToolCallEvent {
                tool: "Read".into(),
                arguments: "{}".into(),
                status: ToolCallStatus::Started,
                result: None,
            })),
            Ok(AgentEvent::ToolCall(ToolCallEvent {
                tool: "Read".into(),
                arguments: "{}".into(),
                status: ToolCallStatus::Succeeded,
                result: None,
            })),
            Ok(AgentEvent::Done(AgentOutcome {
                status: AgentRunStatus::Completed,
                ..Default::default()
            })),
        ])
        .await;
        assert_eq!(chunks.len(), 4, "got {chunks:?}");
        assert!(matches!(chunks[3], AgentChunk::Final(_)));
        // First three are Deltas: text, tool_use, tool_result.
        for c in chunks.iter().take(3) {
            assert!(matches!(c, AgentChunk::Delta(_)));
        }
    }
}
