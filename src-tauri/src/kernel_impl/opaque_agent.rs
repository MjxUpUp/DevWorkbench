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
    Agent, AgentCaps, AgentEvent, AgentInput, AgentKind, AgentOutcome, AgentRunStatus, Error,
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
            self.app.unlisten(id);
        }
    }
}

/// RAII guard that kills the spawned opaque-CLI child when the agent stream is
/// dropped before it yields `Done` (i.e. cancellation). `ListenerGuard` only
/// unregisters the Tauri listeners — it leaves the subprocess running, so before
/// this guard a cancelled opaque agent orphaned its CLI (the reader/wait threads
/// kept the session alive until the process exited on its own). This guard calls
/// `pty::stop_agent` on drop to actually kill it.
///
/// `finalize()` disarms the kill once the run completes naturally (the CLI
/// already exited by the time we yield `Done`), so a post-completion drop does
/// not signal an already-dead process. Deliberately free of any Tauri handle so
/// the cancel-vs-finalize logic is unit-testable in isolation.
struct ChildKillGuard {
    processes: Option<Arc<AgentProcesses>>,
    session_id: String,
    finalized: bool,
}

impl ChildKillGuard {
    fn new(processes: Arc<AgentProcesses>, session_id: String) -> Self {
        Self {
            processes: Some(processes),
            session_id,
            finalized: false,
        }
    }

    /// Mark the run as naturally complete — disarms the kill-on-drop.
    fn finalize(&mut self) {
        self.finalized = true;
    }
}

impl Drop for ChildKillGuard {
    fn drop(&mut self) {
        if !self.finalized {
            if let Some(procs) = self.processes.take() {
                let _ = pty::stop_agent(&procs, &self.session_id);
            }
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
    ) -> Result<BoxStream<'static, Result<AgentEvent, kernel_core::Error>>, kernel_core::Error>
    {
        let app = self.app.clone();
        let processes = self.processes.clone();
        let db = self.db.clone();
        let agent_type = self.agent_type.clone();
        let is_structured = matches!(
            agent_type,
            AgentType::ClaudeCode | AgentType::GeminiCli | AgentType::QwenCode
        );
        let working_dir = input.working_dir.clone().unwrap_or_else(|| ".".to_string());
        let prompt = input.prompt.clone();
        let model = input.model.clone();
        let resume_from = input.resume_from.clone();

        let s = async_stream::try_stream! {
            // Keep a processes handle that survives the spawn closure's move, so
            // the kill-on-drop guard (created after spawn, once we have the
            // session id) can still reach the process table.
            let processes_for_kill = processes.clone();
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

            // Arm kill-on-drop as early as possible (right after spawn) so that
            // if the stream is dropped at ANY point before Done — including
            // during listener setup below — the child is killed instead of
            // orphaned. `finalize()` disarms it on natural completion.
            let mut kill_guard = ChildKillGuard::new(processes_for_kill, session_id.clone());

            // 2. Wire up Tauri event listeners that feed an mpsc channel. We
            //    listen for pty:output (Token chunks) and agent:completed (the
            //    CLI exited). Listeners are sync Tauri callbacks, so they only
            //    send a lightweight AgentMsg signal; the async main loop below
            //    does the heavy work (honesty audit + building the outcome).
            // F6: cap 512 (was 64). A long claude stream-json / codex stdout
            // burst can fill a small buffer; the drain task runs the post-hoc
            // audit AFTER Completed (blocking recv), so a full channel could
            // drop the terminal Completed → the agent appears to never finish.
            // 512 + the error logs on the try_send sites make that vanishingly
            // unlikely; a dedicated oneshot for Completed is the full fix (TODO).
            let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentMsg>(512);

            // Dual-channel dispatch by agent_type. Structured CLIs
            // (claude/gemini/qwen) run in OutputMode::StructuredJson
            // (pty.rs:262), whose reader thread emits STRUCTURED ChatStreamEvent
            // blocks on `agent:event` (plus rendered ANSI text on `pty:output`).
            // We listen the structured channel and reverse-map it back to
            // kernel-core AgentEvent so the workflow path's tool_use/tool_result
            // cards render — listening `pty:output` here would give ANSI text
            // only (the G2 gap). The remaining opaque CLIs (codex/cursor/pi)
            // run in Raw mode and emit ONLY bytes on `pty:output`; for them the
            // structured path does not exist, so they keep the text Token path.
            // `pending` pairs ToolUse↔ToolResult positionally (no tool_call_id
            // on the wire).
            let pending: Arc<std::sync::Mutex<VecDeque<(String, String)>>> =
                Arc::new(std::sync::Mutex::new(VecDeque::new()));
            let mut listener_ids: Vec<tauri::EventId> = Vec::new();

            if is_structured {
                // Structured CLIs (claude/gemini/qwen): agent:event → reverse-mapped AgentEvent.
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
                    for ae in decode_agent_event_payload(&v, &sid_ev, &mut guard) {
                        if tx_ev.try_send(AgentMsg::Structured(ae)).is_err() {
                            log::warn!("[opaque-agent] event channel full, dropping structured event");
                        }
                    }
                });
                listener_ids.push(ev_id);
            } else {
                // Raw agent (codex/cursor/copilot/pi): pty:output → Token.
                // The sessionId filter + `data` byte-array decode + lossy UTF-8
                // live in the pure `decode_pty_output_payload` helper so they're
                // unit-testable without an AppHandle (symmetric to the claude
                // channel's decode_agent_event_payload). from_str (I/O) and
                // try_send (backpressure) stay here. ANSI escapes are preserved
                // through the lossy UTF-8 decode.
                let tx_out = tx.clone();
                let sid_for_output = session_id.clone();
                let output_id = app.listen("pty:output", move |event| {
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(event.payload()) else { return };
                    if let Some(text) = decode_pty_output_payload(&v, &sid_for_output) {
                        if tx_out.try_send(AgentMsg::Token(text)).is_err() {
                            log::warn!("[opaque-agent] event channel full, dropping pty token");
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
                        if tx_done
                            .try_send(AgentMsg::Completed {
                                status: run_status,
                                exit_code,
                            })
                            .is_err()
                        {
                            // F6: dropping Completed is catastrophic — the drain
                            // loop awaits it to terminate the stream, so a drop
                            // makes the agent appear to run forever. Log at error
                            // so it's observable (the 512 cap makes it rare).
                            log::error!(
                                "[opaque-agent] event channel full on Completed — stream may stall forever"
                            );
                        }
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

                        // Natural completion — the CLI already exited, so disarm
                        // the kill-on-drop guard before yielding Done (otherwise
                        // the guard's drop would signal an already-dead process).
                        kill_guard.finalize();
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
    let Ok(wire) = serde_json::from_value::<crate::agents::pty::ChatStreamEvent>(event_val.clone())
    else {
        return Vec::new();
    };
    crate::agents::react_chat::chat_event_to_agent_events(&wire, pending)
}

/// Pure mirror of `decode_agent_event_payload` for the Raw (non-claude) channel:
/// parse + sessionId-filter + extract the `data` byte array from a `pty:output`
/// payload and lossy-decode it to a Token string. Returns None for: a
/// non-matching sessionId (incl. PREFIX sessionIds — `==` is exact, not
/// starts_with), missing/non-array `data`, or any non-object payload — mirroring
/// the Raw listener's silent-skip contract. The raw-payload `from_str` (I/O) and
/// `try_send` (channel backpressure) stay in the listener.
///
/// SEMANTIC PIN — Some("") vs None is load-bearing and asymmetric with the
/// claude channel: an EMPTY data array `[]` is a legal array → yields Some(""),
/// which the listener forwards as Token(""). Missing/non-array `data`, a
/// non-matching sid, or a non-object payload yields None (listener skips). Do
/// NOT "symmetrize" []→None — it changes behavior. Truth source: the listener's
/// `if let Some(data) = v.get("data").and_then(|d| d.as_array())`, which enters
/// for `[]`.
///
/// Extracted pure so the byte-decode (u64→u8 filter + from_utf8_lossy) — a
/// Raw-channel-specific invariant with no claude counterpart — is unit-testable
/// without an AppHandle. NO `pending: &mut VecDeque` param (unlike the claude
/// helper): the Raw path has no FIFO ToolUse↔ToolResult pairing to maintain.
fn decode_pty_output_payload(payload: &serde_json::Value, session_id: &str) -> Option<String> {
    if payload.get("sessionId").and_then(|s| s.as_str()) != Some(session_id) {
        return None;
    }
    let data = payload.get("data")?.as_array()?;
    let bytes: Vec<u8> = data
        .iter()
        .filter_map(|b| b.as_u64().and_then(|n| u8::try_from(n).ok()))
        .collect();
    Some(String::from_utf8_lossy(&bytes).into_owned())
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
        use crate::kernel_impl::anthropic_chat_model::AnthropicChatModel;
        use crate::kernel_impl::react_agent::{ReactAgent, ToolRegistry};
        let transparent: Box<dyn Agent> = Box::new(ReactAgent::new(
            AnthropicChatModel::bigmodel("k", "glm-4.6"),
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
        assert!(
            !caps.injectable_tools,
            "opaque agents reject injected tools"
        );
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
        assert_eq!(
            pending.len(),
            1,
            "pending must be untouched for filtered events"
        );
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
            "s1",
            &mut pending,
        );
        let succeeded = decode_agent_event_payload(
            &json!({
                "sessionId": "s1",
                "event": { "kind": "tool_result", "content": "file body", "is_error": false },
            }),
            "s1",
            &mut pending,
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
            "s1",
            &mut pending,
        );
        let tool_use = decode_agent_event_payload(
            &json!({ "sessionId": "s1", "event": { "kind": "tool_use", "name": "Read", "input": {} } }),
            "s1",
            &mut pending,
        );
        let tool_res = decode_agent_event_payload(
            &json!({ "sessionId": "s1", "event": { "kind": "tool_result", "content": "ok", "is_error": false } }),
            "s1",
            &mut pending,
        );
        let result = decode_agent_event_payload(
            &json!({ "sessionId": "s1", "event": { "kind": "result", "is_error": false, "secs": 3 } }),
            "s1",
            &mut pending,
        );
        let all: Vec<AgentEvent> = [text, tool_use, tool_res].into_iter().flatten().collect();
        assert_eq!(all.len(), 3, "text + tool_use + tool_result → 3 events");
        assert!(
            result.is_empty(),
            "Result block must NOT emit (Done owned by agent:completed)"
        );
        match &all[0] {
            AgentEvent::Token(s) => assert_eq!(s, "reading"),
            other => panic!("expected Token, got {:?}", other),
        }
        assert_tool_event(&all[1], "Read", ToolCallStatus::Started);
        assert_tool_event(&all[2], "Read", ToolCallStatus::Succeeded);
    }

    // ---- decode_pty_output_payload (Raw channel, symmetric to the claude
    // decode_* tests above) ----

    #[test]
    fn decode_pty_output_matching_session_emits_decoded_text() {
        // Happy path: matching session + ASCII byte array → decoded string.
        let out =
            decode_pty_output_payload(&json!({ "sessionId": "s1", "data": [104, 105] }), "s1");
        assert_eq!(out.as_deref(), Some("hi"));
    }

    #[test]
    fn decode_pty_output_filters_other_session_id() {
        // Concurrency-critical: a workflow runs multiple raw nodes in parallel,
        // each listening the SAME global pty:output channel filtered by its own
        // sid. A payload for a different session must yield None (listener
        // skips) — symmetric to the claude channel's cross-session guard.
        let out = decode_pty_output_payload(&json!({ "sessionId": "other", "data": [104] }), "s1");
        assert!(out.is_none());
    }

    #[test]
    fn decode_pty_output_filters_prefix_session_id() {
        // `==` is EXACT, not starts_with. Two UUID-like sessions sharing a
        // prefix ("s1" vs "s1prefix") must NOT cross-trigger. (The claude
        // channel's test set is missing this boundary — added here on raw.)
        let out =
            decode_pty_output_payload(&json!({ "sessionId": "s1prefix", "data": [104] }), "s1");
        assert!(out.is_none());
    }

    #[test]
    fn decode_pty_output_missing_data_field() {
        let out = decode_pty_output_payload(&json!({ "sessionId": "s1" }), "s1");
        assert!(out.is_none());
    }

    #[test]
    fn decode_pty_output_data_not_array() {
        // data as a string or number is not a byte array → None.
        assert!(
            decode_pty_output_payload(&json!({ "sessionId": "s1", "data": "hi" }), "s1",).is_none()
        );
        assert!(
            decode_pty_output_payload(&json!({ "sessionId": "s1", "data": 42 }), "s1",).is_none()
        );
    }

    #[test]
    fn decode_pty_output_empty_data_array_is_some_empty_string() {
        // SEMANTIC PIN: an empty array is a LEGAL array → Some(""), NOT None.
        // Asymmetric with missing/non-array data (which yield None) and with the
        // claude channel. Guards against a future "symmetrization" that would
        // change behavior. Truth source: the listener's
        // `if let Some(data) = ...as_array()` enters for `[]`.
        let out = decode_pty_output_payload(&json!({ "sessionId": "s1", "data": [] }), "s1");
        assert_eq!(out.as_deref(), Some(""));
    }

    #[test]
    fn decode_pty_output_non_u8_elements_dropped() {
        // A mixed array: only values where as_u64→Some(n) AND
        // u8::try_from(n)→Ok survive the filter_map. Out-of-range (256),
        // negative (-1), string, null, bool, and non-integer float (1.5) all
        // drop; 104/105 survive → "hi".
        let out = decode_pty_output_payload(
            &json!({ "sessionId": "s1", "data": [104, 256, -1, "x", null, true, 1.5, 105] }),
            "s1",
        );
        assert_eq!(out.as_deref(), Some("hi"));

        // Integer-float boundary: json!(256.0) is an f64 that as_u64() ACCEPTS
        // (Some(256)), but u8::try_from(256) rejects it — so it drops via the
        // SAME u8::try_from branch as the integer 256 above, NOT via the
        // as_u64-None branch that 1.5 takes. Pinning the distinction.
        let float_out =
            decode_pty_output_payload(&json!({ "sessionId": "s1", "data": [256.0] }), "s1");
        assert_eq!(float_out.as_deref(), Some(""));
    }

    #[test]
    fn decode_pty_output_invalid_utf8_lossy() {
        // 0xFF is not a valid UTF-8 lead byte → from_utf8_lossy replaces it with
        // U+FFFD, 0x41 ('A') survives. Raw-channel-specific invariant (the
        // claude channel has no byte decode). Pins: no panic, replacement
        // semantics — a CLI emitting non-UTF-8 bytes (Windows paths, broken
        // emoji) still surfaces, just with U+FFFD.
        let out = decode_pty_output_payload(&json!({ "sessionId": "s1", "data": [255, 65] }), "s1");
        assert_eq!(out.as_deref(), Some("\u{FFFD}A"));
    }

    #[test]
    fn decode_pty_output_session_id_non_string() {
        // sessionId as a number must not match the string target (and must not
        // panic) — number.as_str() is None.
        let out = decode_pty_output_payload(&json!({ "sessionId": 42, "data": [] }), "42");
        assert!(out.is_none());
    }

    #[test]
    fn decode_pty_output_non_object_payload() {
        // A non-object payload (string / array / null) must yield None, no panic.
        assert!(decode_pty_output_payload(&json!("str"), "s1").is_none());
        assert!(decode_pty_output_payload(&json!([1, 2]), "s1").is_none());
        assert!(decode_pty_output_payload(&json!(null), "s1").is_none());
    }

    #[test]
    fn child_kill_guard_kills_on_early_drop() {
        // Cancellation path: the agent stream is dropped before Done. The spawned
        // CLI child is still alive and must be killed (previously it orphaned —
        // ListenerGuard only unlistened). A bogus PID is harmless: stop_agent's
        // kill is best-effort; we assert the table entry (the kill target) is
        // removed, which is the observable effect of the drop calling stop_agent.
        let procs = Arc::new(AgentProcesses::new());
        pty::track_test_pipe(&procs, "s1", 999_999);
        assert!(pty::is_tracked(&procs, "s1"));
        {
            let _g = ChildKillGuard::new(procs.clone(), "s1".into());
        }
        assert!(
            !pty::is_tracked(&procs, "s1"),
            "dropping a non-finalized guard must kill the child"
        );
    }

    #[test]
    fn child_kill_guard_finalize_disarms_kill() {
        // Natural-completion path: Done was yielded, so finalize() was called.
        // Dropping the guard must NOT call stop_agent — the CLI already exited,
        // and killing would be a redundant signal to a dead process.
        let procs = Arc::new(AgentProcesses::new());
        pty::track_test_pipe(&procs, "s2", 999_998);
        {
            let mut g = ChildKillGuard::new(procs.clone(), "s2".into());
            g.finalize();
        }
        assert!(
            pty::is_tracked(&procs, "s2"),
            "a finalized guard must not kill an already-completed child"
        );
    }
}
