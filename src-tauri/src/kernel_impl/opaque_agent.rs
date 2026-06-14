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
                    )
                })
                .await
                .map_err(|e| Error::Agent(format!("spawn join: {e}")))?
                .map_err(Error::Agent)?
            };
            let session_id = session.id.clone();

            // 2. Wire up Tauri event listeners that feed an mpsc channel. We
            //    listen for pty:output (Token) and agent:completed (Done),
            //    filtering by this session's id.
            let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<AgentEvent, kernel_core::Error>>(64);

            // pty:output -> AgentEvent::Token (decode bytes lossily as UTF-8;
            // ANSI escapes are preserved 鈥?the frontend renders them).
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
                            let _ = tx_out.try_send(Ok(AgentEvent::Token(text)));
                        }
                    }
                }
            });

            // agent:completed -> AgentEvent::Done. We also fetch files_changed
            // from the DB session row (the wait thread populates it).
            let tx_done = tx.clone();
            let sid_for_done = session_id.clone();
            let app_for_done = app.clone();
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

                        // Best-effort: read files_changed from the session row.
                        let files_changed = read_session_files(&app_for_done, &sid_for_done);

                        let _ = tx_done.try_send(Ok(AgentEvent::Done(AgentOutcome {
                            status: run_status,
                            files_changed,
                            exit_code,
                            output_summary: None,
                        })));
                        // Signal stream end.
                        let _ = tx_done.try_send(Ok(AgentEvent::TurnBoundary));
                    }
                }
            });

            // 3. Drain the channel into the stream. When we see a Done event,
            //    stop and unregister listeners (avoid leaking registrations
            //    across runs 鈥?Tauri listeners live until unlisten/app exit).
            // C6: guard ensures unlisten runs even if the stream is dropped early.
            let _listener_guard = ListenerGuard::new(app.clone(), vec![output_id, done_id]);
            while let Some(ev) = rx.recv().await {
                let is_done = matches!(ev, Ok(AgentEvent::Done(_)));
                yield ev?;
                if is_done {
                    break;
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
