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
                    // poisoned lock → drop this event, never panic the listener. std Mutex
                    // platform semantics; not unit-tested (synthesizing a poison needs a
                    // panicking lock holder — cost > value for this branch).
                    let mut guard = match pending_ev.lock() {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                    // sessionId filter + `event` decode + FIFO pairing live in the pure
                    // `decode_agent_event_payload` helper so they're unit-testable without
                    // an AppHandle. The lock spans the decode (pending IS the FIFO queue);
                    // no contention in practice — each session owns its own pending and the
                    // reader thread emits events serially.
                    for ae in decode_agent_event_payload(&v, &sid_ev, &mut *guard) {
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

/// Parse + sessionId-filter + reverse-map one `agent:event` payload into
/// `AgentEvent`s for the OpaqueAgent stream. PURE (no I/O, no locking): the
/// raw-payload `from_str` and `pending.lock()` stay in the listener — the
/// caller passes an already-parsed `Value` + an already-locked `VecDeque`
/// (lock scope preserved by passing `&mut`, NOT `Arc<Mutex>`, so no per-event
/// re-lock). Returns empty (never panics) for: non-matching sessionId, missing
/// `event` field, or a non-deserializable `ChatStreamEvent` — mirroring the
/// listener's silent-skip contract.
///
/// Extracted from the `app.listen("agent:event", …)` closure so the
/// sessionId-routing + decode logic is unit-testable without an `AppHandle`
/// (the closure itself needs Tauri runtime + a real spawned CLI, which is why
/// this layer had zero coverage after G4 — only the downstream pure
/// `chat_event_to_agent_events` was tested). The sessionId filter is
/// concurrency-critical: a workflow runs multiple claude nodes in parallel,
/// each listening the SAME global `agent:event` channel filtered by its own sid.
fn decode_agent_event_payload(
    payload: &serde_json::Value,
    session_id: &str,
    pending: &mut VecDeque<(String, String)>,
) -> Vec<AgentEvent> {
    if payload.get("sessionId").and_then(|s| s.as_str()) != Some(session_id) {
        return Vec::new();
    }
    let Some(event_val) = payload.get("event") else {
        return Vec::new();
    };
    let Ok(wire) =
        serde_json::from_value::<crate::agents::pty::ChatStreamEvent>(event_val.clone())
    else {
        return Vec::new();
    };
    crate::agents::react_chat::chat_event_to_agent_events(&wire, pending)
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

    // ---- decode_agent_event_payload (agent:event listener decode layer) ----
    //
    // These cover the sessionId-routing + event-decode glue that was inlined in
    // the listener closure and had zero coverage after G4. The closure itself
    // (from_str, pending.lock, try_send) stays untested — those are I/O / mutex
    // boundaries; only the pure decode is tested here.

    use kernel_core::ToolCallStatus;
    use serde_json::json;

    /// Assert an AgentEvent is a ToolCall with the given name + status.
    fn assert_tool_event(ev: &AgentEvent, expected_name: &str, expected_status: ToolCallStatus) {
        match ev {
            AgentEvent::ToolCall(tc) => {
                assert_eq!(tc.tool, expected_name, "tool name mismatch");
                assert_eq!(tc.status, expected_status, "status mismatch");
            }
            other => panic!("expected ToolCall({expected_name}), got {:?}", other),
        }
    }

    #[test]
    fn decode_text_payload_matching_session_emits_token() {
        let payload = json!({
            "sessionId": "s1",
            "event": { "kind": "text", "content": "hi" },
        });
        let mut pending = VecDeque::new();
        let out = decode_agent_event_payload(&payload, "s1", &mut pending);
        assert_eq!(out.len(), 1);
        match &out[0] {
            AgentEvent::Token(s) => assert_eq!(s, "hi"),
            other => panic!("expected Token, got {:?}", other),
        }
        assert!(pending.is_empty());
    }

    #[test]
    fn decode_filters_other_session_id_and_leaves_pending_intact() {
        // Concurrency guard: a workflow runs multiple claude nodes in parallel,
        // each filtering the SAME global agent:event channel by its own sid. An
        // event for ANOTHER session must drop WITHOUT touching our pending FIFO
        // queue — otherwise a future refactor that moves pop_front above the sid
        // check silently steals ToolResult pairing across sessions.
        let mut pending = VecDeque::new();
        pending.push_back(("A".to_string(), "{}".to_string()));
        let payload = json!({
            "sessionId": "other-session",
            "event": { "kind": "tool_result", "content": "not ours", "is_error": false },
        });
        let out = decode_agent_event_payload(&payload, "s1", &mut pending);
        assert!(out.is_empty(), "other-session event must be filtered");
        assert_eq!(pending.len(), 1, "pending must be untouched for filtered events");
        assert_eq!(pending.front().unwrap().0, "A");
    }

    #[test]
    fn decode_missing_event_field_returns_empty() {
        let payload = json!({ "sessionId": "s1" });
        let mut pending = VecDeque::new();
        let out = decode_agent_event_payload(&payload, "s1", &mut pending);
        assert!(out.is_empty());
        assert!(pending.is_empty());
    }

    #[test]
    fn decode_malformed_event_returns_empty() {
        // `event` present but not a deserializable ChatStreamEvent (a stray
        // string). Must skip silently, never panic the listener.
        let payload = json!({ "sessionId": "s1", "event": "not-an-object" });
        let mut pending = VecDeque::new();
        let out = decode_agent_event_payload(&payload, "s1", &mut pending);
        assert!(out.is_empty());
    }

    #[test]
    fn decode_null_event_returns_empty() {
        // Defensive: pty.rs's reader never emits event:null today, but a null
        // must degrade gracefully rather than panic.
        let payload = json!({ "sessionId": "s1", "event": null });
        let mut pending = VecDeque::new();
        assert!(decode_agent_event_payload(&payload, "s1", &mut pending).is_empty());
    }

    #[test]
    fn decode_session_id_non_string_returns_empty() {
        // sessionId as number/bool/null — .as_str() returns None → filtered.
        let cases = [
            json!({ "sessionId": 123, "event": { "kind": "text", "content": "x" } }),
            json!({ "sessionId": null, "event": { "kind": "text", "content": "x" } }),
            json!({ "sessionId": true, "event": { "kind": "text", "content": "x" } }),
        ];
        for p in cases {
            let mut pending = VecDeque::new();
            assert!(
                decode_agent_event_payload(&p, "s1", &mut pending).is_empty(),
                "non-string sessionId must filter: {p}"
            );
        }
    }

    #[test]
    fn decode_non_object_payload_returns_empty() {
        // Whole payload isn't an object — .get() returns None on non-objects,
        // every branch falls through to empty. No panic.
        let cases: Vec<serde_json::Value> = vec![
            json!([1, 2, 3]),
            json!("just a string"),
            serde_json::Value::Null,
            json!(42),
        ];
        for p in cases {
            let mut pending = VecDeque::new();
            assert!(
                decode_agent_event_payload(&p, "s1", &mut pending).is_empty(),
                "non-object payload must not panic: {p}"
            );
        }
    }

    #[test]
    fn decode_tool_use_then_result_pairs_fifo() {
        // End-to-end through the decode layer: tool_use then tool_result pair
        // positionally into Started + Succeeded ToolCalls.
        let mut pending = VecDeque::new();
        let started = decode_agent_event_payload(
            &json!({
                "sessionId": "s1",
                "event": { "kind": "tool_use", "name": "Read", "input": { "file_path": "/x" } },
            }),
            "s1", &mut pending,
        );
        let succeeded = decode_agent_event_payload(
            &json!({
                "sessionId": "s1",
                "event": { "kind": "tool_result", "content": "file body", "is_error": false },
            }),
            "s1", &mut pending,
        );
        assert_eq!(started.len(), 1);
        assert_eq!(succeeded.len(), 1);
        assert_tool_event(&started[0], "Read", ToolCallStatus::Started);
        assert_tool_event(&succeeded[0], "Read", ToolCallStatus::Succeeded);
        assert!(pending.is_empty(), "FIFO queue drained after pairing");
    }

    #[test]
    fn decode_full_claude_turn_sequence() {
        // A realistic claude turn: text + tool_use + tool_result + result.
        // Decode yields [Token, ToolCall(Started), ToolCall(Succeeded)] — the
        // Result block emits NOTHING (Done is owned by agent:completed, not the
        // event stream; emitting here would double-end the OpaqueAgent stream).
        let mut pending = VecDeque::new();
        let text = decode_agent_event_payload(
            &json!({ "sessionId": "s1", "event": { "kind": "text", "content": "reading" } }),
            "s1", &mut pending,
        );
        let tool_use = decode_agent_event_payload(
            &json!({ "sessionId": "s1", "event": { "kind": "tool_use", "name": "Read", "input": {} } }),
            "s1", &mut pending,
        );
        let tool_res = decode_agent_event_payload(
            &json!({ "sessionId": "s1", "event": { "kind": "tool_result", "content": "ok", "is_error": false } }),
            "s1", &mut pending,
        );
        let result = decode_agent_event_payload(
            &json!({ "sessionId": "s1", "event": { "kind": "result", "is_error": false, "secs": 3 } }),
            "s1", &mut pending,
        );
        let all: Vec<AgentEvent> = [text, tool_use, tool_res].into_iter().flatten().collect();
        assert_eq!(all.len(), 3, "text + tool_use + tool_result → 3 events");
        assert!(result.is_empty(), "Result block must NOT emit (Done owned by agent:completed)");
        match &all[0] {
            AgentEvent::Token(s) => assert_eq!(s, "reading"),
            other => panic!("expected Token, got {:?}", other),
        }
        assert_tool_event(&all[1], "Read", ToolCallStatus::Started);
        assert_tool_event(&all[2], "Read", ToolCallStatus::Succeeded);
    }
}
