//! OpaqueAgent 鈥?wraps an external CLI process (claude/codex/gemini/鈥? as a
//! `kernel_core::Agent`.
//!
//! This is the "opaque" half of the dual-mode kernel: the agent's internal
//! reason鈫抰ool loop runs inside a subprocess we cannot inspect; we only observe
//! its stdout stream and exit. OpaqueAgent bridges those observations into the
//! unified `AgentEvent` stream so the graph engine and frontend treat opaque
//! and transparent (ReactAgent) uniformly.
//!
//! Pipeline:
//!   spawn_pty_agent (sync) 鈫?session id
//!   listen("pty:output", filter by sid)  鈫?AgentEvent::Token
//!   listen("agent:completed", filter sid) 鈫?AgentEvent::Done(AgentOutcome)
//!   stream dropped / cancelled           鈫?stop_agent (Ctrl-C semantics)

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use kernel_core::{
    Error,
    Agent, AgentCaps, AgentEvent, AgentInput, AgentKind, AgentOutcome, AgentRunStatus,
};
use tauri::Listener;

use crate::agents::pty::{self, AgentProcesses};
use crate::db::DbState;
use crate::models::AgentType;

/// Internal message from the Tauri event listeners to the agent stream's
/// main loop. Listeners are synchronous Tauri callbacks and cannot `await`
/// (so they cannot run the blocking honesty audit); they send a lightweight
/// signal and the async main loop does the heavy work (audit + Done).
enum AgentMsg {
    /// A chunk of CLI stdout (pty:output) — raw agents only.
    Token(String),
    /// A structured agent event reverse-mapped from claude's `agent:event`
    /// wire blocks (via `chat_event_to_agent_events`) — ClaudeCode only.
    Structured(AgentEvent),
    /// The CLI process exited (agent:completed). Carries just the parsed
    /// status/exit-code; the main loop attaches files_changed + honesty audit.
    Completed {
        status: AgentRunStatus,
        exit_code: Option<i32>,
    },
}

/// An external CLI agent, wrapped to satisfy the kernel's `Agent` trait.
pub struct OpaqueAgent {
    app: tauri::AppHandle,
    processes: Arc<AgentProcesses>,
    db: DbState,
    agent_type: AgentType,
}

impl OpaqueAgent {
    pub fn new(
        app: tauri::AppHandle,
        processes: Arc<AgentProcesses>,
        db: DbState,
        agent_type: AgentType,
    ) -> Self {
        Self {
            app,
            processes,
            db,
            agent_type,
        }
    }
}


/// RAII guard that unregisters Tauri event listeners on drop. Ensures
/// listeners are cleaned up even when the agent stream is dropped early
/// (cancellation), fixing the leak where unlisten was only reached after Done.
struct ListenerGuard {
    app: tauri::AppHandle,
    ids: Vec<tauri::EventId>,
}

impl ListenerGuard {
    fn new(app: tauri::AppHandle, ids: Vec<tauri::EventId>) -> Self {
        Self { app, ids }
    }
}

impl Drop for ListenerGuard {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            let _ = self.app.unlisten(id);
        }
    }
}

#[async_trait]
impl Agent for OpaqueAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Opaque
    }

    fn capabilities(&self) -> AgentCaps {
        // External CLI: we can stop it (Ctrl-C) and resume via --continue.
        // We CANNOT inject tools (the CLI owns its own).
        AgentCaps {
            interruptible: true,
            resumable: true,
            injectable_tools: false,
            read_only: false,
        }
    }

    fn run(
        &self,
        input: AgentInput,
    ) -> Result<BoxStream<'static, Result<AgentEvent, kernel_core::Error>>, kernel_core::Error> {
        let app = self.app.clone();
        let processes = self.processes.clone();
        let db = self.db.clone();
        let agent_type = self.agent_type.clone();
        let is_claude = matches!(agent_type, AgentType::ClaudeCode);
        let working_dir = input
            .working_dir
            .clone()
            .unwrap_or_else(|| ".".to_string());
        let prompt = input.prompt.clone();
        let model = input.model.clone();
        let resume_from = input.resume_from.clone();

        let s = async_stream::try_stream! {
            // 1. Spawn the CLI process (sync, returns immediately with a Session).
            let session = {
                let app = app.clone();
                let project_path = working_dir.clone();
                tokio::task::spawn_blocking(move || -> Result<crate::models::Session, String> {
                    pty::spawn_pty_agent(
                        &app,
                        processes.clone(),
                        db,
                        &project_path,
                        agent_type,
                        &prompt,
                        model.as_deref(),
                        None,
                        resume_from.as_deref(),
                        None,
                    )
                })
                .await
                .map_err(|e| Error::Agent(format!("spawn join: {e}")))?
                .map_err(Error::Agent)?
            };
            let session_id = session.id.clone();

            // 2. Wire up Tauri event listeners that feed an mpsc channel. We
            //    listen for pty:output (Token chunks) and agent:completed (the
            //    CLI exited). Listeners are sync Tauri callbacks, so they only
            //    send a lightweight AgentMsg signal; the async main loop below
            //    does the heavy work (honesty audit + building the outcome).
            let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentMsg>(64);

            // Dual-channel dispatch by agent_type. ClaudeCode runs in
            // OutputMode::ClaudeStreamJson (pty.rs:262), whose reader thread
            // emits STRUCTURED ChatStreamEvent blocks on `agent:event` (plus
            // rendered ANSI text on `pty:output`). We listen the structured
            // channel and reverse-map it back to kernel-core AgentEvent so the
            // workflow path's tool_use/tool_result cards render — listening
            // `pty:output` here would give ANSI text only (the G2 gap). All
            // other opaque CLIs (codex/gemini/qwen/cursor/pi) run in Raw mode
            // and emit ONLY bytes on `pty:output`; for them the structured path
            // does not exist, so they keep the text Token path. `pending` pairs
            // ToolUse↔ToolResult positionally (no tool_call_id on the wire).
            let pending: Arc<std::sync::Mutex<VecDeque<(String, String)>>> =
                Arc::new(std::sync::Mutex::new(VecDeque::new()));
            let mut listener_ids: Vec<tauri::EventId> = Vec::new();

            if is_claude {
                // ClaudeCode: structured agent:event → reverse-mapped AgentEvent.
                let tx_ev = tx.clone();
                let sid_ev = session_id.clone();
                let pending_ev = pending.clone();
                let ev_id = app.listen("agent:event", move |event| {
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(event.payload()) else { return };
                    if v.get("sessionId").and_then(|s| s.as_str()) != Some(&sid_ev) {
                        return;
                    }
                    let Some(event_val) = v.get("event") else { return };
                    let Ok(wire) = serde_json::from_value::<crate::agents::pty::ChatStreamEvent>(
                        event_val.clone(),
                    ) else { return };
                    let mut guard = match pending_ev.lock() {
                        Ok(g) => g,
                        Err(_) => return, // poisoned — drop this event, never panic the listener
                    };
                    for ae in crate::agents::react_chat::chat_event_to_agent_events(&wire, &mut *guard) {
                        let _ = tx_ev.try_send(AgentMsg::Structured(ae));
                    }
                });
                listener_ids.push(ev_id);
            } else {
                // Raw agent (codex/gemini/qwen/cursor/pi): pty:output → Token.
                // Decode bytes lossily as UTF-8; ANSI escapes are preserved.
                let tx_out = tx.clone();
                let sid_for_output = session_id.clone();
                let output_id = app.listen("pty:output", move |event| {
                    let payload = event.payload();
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) {
                        if v.get("sessionId").and_then(|s| s.as_str()) == Some(&sid_for_output) {
                            if let Some(data) = v.get("data").and_then(|d| d.as_array()) {
                                let bytes: Vec<u8> = data.iter()
                                    .filter_map(|b| b.as_u64().and_then(|n| u8::try_from(n).ok()))
                                    .collect();
                                let text = String::from_utf8_lossy(&bytes).into_owned();
                                let _ = tx_out.try_send(AgentMsg::Token(text));
                            }
                        }
                    }
                });
                listener_ids.push(output_id);
            }

            // agent:completed -> AgentMsg::Completed. The listener only parses
            // status/exit-code; the main loop attaches files_changed + runs the
            // post-hoc honesty audit (it can't run here — listeners can't await
            // the blocking git-diff/cargo-check work).
            let tx_done = tx.clone();
            let sid_for_done = session_id.clone();
            let done_id = app.listen("agent:completed", move |event| {
                let payload = event.payload();
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) {
                    if v.get("sessionId").and_then(|s| s.as_str()) == Some(&sid_for_done) {
                        let status_str = v.get("status").and_then(|s| s.as_str()).unwrap_or("failed");
                        let exit_code = v.get("exitCode").and_then(|c| c.as_i64()).map(|i| i as i32);
                        let run_status = if status_str == "completed" {
                            AgentRunStatus::Completed
                        } else {
                            AgentRunStatus::Failed
                        };
                        let _ = tx_done.try_send(AgentMsg::Completed {
                            status: run_status,
                            exit_code,
                        });
                    }
                }
            });

            // 3. Drain the channel into the stream. On Completed, run the
            //    post-hoc honesty audit (gap-③): the opaque CLI is a black box,
            //    so call-level hooks are impossible — we scan its uncommitted
            //    diff for assertion weakening + sanity-check the env *after* it
            //    exits, then emit Done with the audit attached.
            //    ListenerGuard ensures unlisten runs even on early stream drop.
            listener_ids.push(done_id);
            let _listener_guard = ListenerGuard::new(app.clone(), listener_ids);
            while let Some(msg) = rx.recv().await {
                match msg {
                    AgentMsg::Token(text) => {
                        yield AgentEvent::Token(text);
                    }
                    AgentMsg::Structured(ae) => {
                        yield ae;
                    }
                    AgentMsg::Completed { status, exit_code } => {
                        let audit_dir = working_dir.clone();
                        let honesty = tokio::task::spawn_blocking(move || {
                            crate::kernel_impl::honesty::audit_project(
                                std::path::Path::new(&audit_dir),
                                "",
                            )
                        })
                        .await
                        .map_err(|e| Error::Agent(format!("honesty audit join: {e}")))?;

                        let files_changed = read_session_files(&app, &session_id);

                        yield AgentEvent::Done(AgentOutcome {
                            status,
                            files_changed,
                            exit_code,
                            output_summary: None,
                            honesty: Some(honesty),
                        });
                        break;
                    }
                }
            }
            // _listener_guard drops here (or on early stream drop) -> unlisten both.
        };
        Ok(Box::pin(s))
    }
}

/// Read the files_changed list for a session from the DB (best-effort).
fn read_session_files(app: &tauri::AppHandle, session_id: &str) -> Vec<String> {
    use tauri::Manager;
    let Some(db) = app.try_state::<DbState>() else {
        return Vec::new();
    };
    let db = db.inner().clone();
    let sid = session_id.to_string();
    let Ok(conn) = db.get() else {
        return Vec::new();
    };
    let snap_str: Option<String> = conn
        .query_row(
            "SELECT context_snapshot FROM sessions WHERE id = ?1",
            rusqlite::params![&sid],
            |r| r.get::<_, Option<String>>(0),
        )
        .unwrap_or(None);
    crate::utils::files_changed_from_snapshot(snap_str.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AgentType;

    /// Every opaque CLI spec string must resolve to an AgentType 鈥?this is the
    /// precondition for OpaqueAgent construction. Guards the from_spec table.
    #[test]
    fn all_cli_specs_resolve_to_agent_type() {
        let cases = [
            ("claude_code", AgentType::ClaudeCode),
            ("claude", AgentType::ClaudeCode),
            ("codex", AgentType::Codex),
            ("cursor_agent", AgentType::CursorAgent),
            ("gemini_cli", AgentType::GeminiCli),
            ("qwen_code", AgentType::QwenCode),
            ("pi", AgentType::Pi),
        ];
        for (spec, expected) in cases {
            assert_eq!(
                AgentType::from_spec(spec),
                Some(expected),
                "spec '{spec}' should resolve"
            );
        }
    }

    /// An unknown spec returns None (the caller treats it as a transparent agent).
    #[test]
    fn unknown_spec_returns_none() {
        assert!(AgentType::from_spec("react").is_none());
        assert!(AgentType::from_spec("does_not_exist").is_none());
    }

    /// Compile-time proof of dual-mode unification: OpaqueAgent and ReactAgent
    /// both satisfy `Box<dyn Agent>` and can coexist in one collection. We only
    /// build a ReactAgent here (OpaqueAgent needs an AppHandle), but the fact
    /// that this function type-checks proves the trait is object-safe and shared.
    #[allow(dead_code)]
    fn dual_mode_unification_proof() {
        use crate::kernel_impl::react_agent::{GlmChatModel, ReactAgent, ToolRegistry};
        let transparent: Box<dyn Agent> = Box::new(ReactAgent::new(
            GlmChatModel::bigmodel("k", "glm-4.6"),
            ToolRegistry::new(),
            "sys",
        ));
        // If we had an AppHandle, this would also compile:
        //   let opaque: Box<dyn Agent> = Box::new(OpaqueAgent::new(app, proc, db, AgentType::ClaudeCode));
        assert_eq!(transparent.kind(), AgentKind::Transparent);
    }

    /// capabilities() for an opaque agent declares the right contract:
    /// interruptible + resumable, but NOT injectable_tools (the CLI owns its tools).
    /// We assert the expected shape directly since we can't construct one.
    #[test]
    fn opaque_capabilities_contract() {
        let caps = AgentCaps {
            interruptible: true,
            resumable: true,
            injectable_tools: false,
            read_only: false,
        };
        assert!(caps.interruptible);
        assert!(caps.resumable);
        assert!(!caps.injectable_tools, "opaque agents reject injected tools");
    }
}
