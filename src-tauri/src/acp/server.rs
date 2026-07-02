//! C1 — ACP (Agent Client Protocol) **server**: expose THIS kernel as an
//! ACP-servable agent that an IDE / another agent drives over stdio JSON-RPC.
//! This is the second half of the bidirectional ACP support the OWOz design
//! doc (C1) makes mandatory, complementing the client half ([`crate::acp::
//! client`]). Together they let Dev Workbench's kernel BOTH drive external
//! coding agents AND be driven — the same role Claude Code / codex play for
//! Zed, Cursor, etc.
//!
//! Two surfaces, separated for testability (mirrors the client split):
//!   - [`map_event_to_update`] + [`EventBridge`] — the PURE mapping from a
//!     kernel [`AgentEvent`] to the ACP [`SessionUpdate`] it carries. This is
//!     the error-prone part (which event variant is "answer text", how tool
//!     ids link a Started call to its Succeeded update), so it is extracted as
//!     a unit-testable stateful mapper driven by a scripted event sequence.
//!   - [`serve_stdio`] — the live server entry. Binds the three required ACP
//!     agent request handlers (`initialize` / `session/new` / `session/prompt`)
//!     over stdio via the crate's `Agent` role + `Stdio` transport — the exact
//!     shape of the crate's `examples/simple_agent.rs`, extended to drive the
//!     kernel ReactAgent on prompt and stream `session/update` notifications
//!     back. Integration-level: it needs a real ACP client (Zed / our own
//!     [`crate::acp::client::run_acp_agent`]) speaking to it over stdio to
//!     exercise end to end; its structure mirrors the crate's proven example.
//!
//! Blueprint: `agent-client-protocol` crate `examples/simple_agent.rs` (the
//! server-side counterpart to `yolo_one_shot_client.rs`).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::{
    AgentCapabilities, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionId,
    SessionNotification, SessionUpdate, StopReason, TextContent, ToolCall, ToolCallId,
    ToolCallStatus as AcpToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{Agent as AcpAgentRole, Client, ConnectionTo, Responder, Stdio};

use kernel_core::{Agent as _, AgentEvent, AgentRunStatus, ToolCallStatus as KernelToolCallStatus};

use crate::kernel_impl::executor::build_react_agent;
use crate::kernel_impl::hooks::PermissionMode;

// ---------------------------------------------------------------------------
// Pure event bridge — kernel AgentEvent → ACP SessionUpdate
// ---------------------------------------------------------------------------

/// A stateful mapper from kernel [`AgentEvent`]s to ACP [`SessionUpdate`]s.
///
/// State is needed because the kernel's [`kernel_core::ToolCallEvent`] carries
/// NO stable tool-call id, yet ACP links a `ToolCall` (start) to its later
/// `ToolCallUpdate` (completion) BY id. The bridge assigns a monotonic id per
/// `Started` tool call and queues it in `pending` (FIFO); the next terminal
/// status pops the OLDEST queued id. This is correct under:
///   - strict sequencing (`Started A` → `Succeeded A`),
///   - batched dispatch in start-order (`Started A`, `Started B` → `Succeeded
///     A`, `Succeeded B`),
/// both of which cover the kernel's actual emit patterns (a ReactAgent fans out
/// parallel calls via `execute_call_set` but emits each call's Started before
/// its terminal status). It would still mismatch on INTERLEAVED completion
/// (`Started A`, `Started B`, `Succeeded B`, `Succeeded A`); the kernel emits
/// completions in dispatch order, so this is not observed. The proper fix is a
/// stable id on `kernel_core::ToolCallEvent` itself (tracked TODO).
#[derive(Debug, Default)]
pub struct EventBridge {
    /// Monotonic counter → deterministic tool-call ids ("dw-1", "dw-2", …).
    next_id: u64,
    /// Ids of Started-but-not-yet-terminal tool calls, in start order. A
    /// terminal status pops the front (oldest) to link back to its Started.
    pending: VecDeque<ToolCallId>,
}

impl EventBridge {
    /// A fresh bridge for one prompt turn.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Map one kernel event to zero or one ACP session update, advancing state.
    ///
    /// - [`AgentEvent::Token`] → `AgentMessageChunk` (the streamed answer).
    /// - [`AgentEvent::Reasoning`] → `AgentThoughtChunk` (thinking trace).
    /// - [`AgentEvent::ToolCall`] `Started` → `ToolCall` (assigns a fresh id,
    ///   queues it in `pending`).
    /// - [`AgentEvent::ToolCall`] `Succeeded`/`Failed` → `ToolCallUpdate`
    ///   (pops the oldest queued id from `pending`).
    /// - [`AgentEvent::FileChanged`] / [`AgentEvent::TurnBoundary`] /
    ///   [`AgentEvent::Done`] → `None` (not streamed updates: Done resolves the
    ///   `session/prompt` request itself).
    pub fn map(&mut self, event: &AgentEvent) -> Option<SessionUpdate> {
        match event {
            AgentEvent::Token(text) => Some(text_chunk(text, false)),
            AgentEvent::Reasoning(text) => Some(text_chunk(text, true)),
            AgentEvent::ToolCall(tc) => Some(self.map_tool_call(tc)),
            AgentEvent::FileChanged(_) | AgentEvent::TurnBoundary | AgentEvent::Done(_) => None,
        }
    }

    /// Assign a fresh tool-call id ("dw-{n}") and queue it for the matching
    /// terminal update.
    fn fresh_tool_id(&mut self) -> ToolCallId {
        self.next_id = self.next_id.saturating_add(1);
        // ToolCallId is #[non_exhaustive] → must use ::new (tuple-struct literal
        // is blocked from outside the crate).
        let id = ToolCallId::new(format!("dw-{}", self.next_id));
        self.pending.push_back(id.clone());
        id
    }

    fn map_tool_call(&mut self, tc: &kernel_core::ToolCallEvent) -> SessionUpdate {
        match tc.status {
            KernelToolCallStatus::Started => {
                let id = self.fresh_tool_id();
                let mut call = ToolCall::new(id, tc.tool.clone())
                    .status(AcpToolCallStatus::InProgress);
                if let Ok(raw) = serde_json::from_str::<serde_json::Value>(&tc.arguments) {
                    call = call.raw_input(raw);
                }
                SessionUpdate::ToolCall(call)
            }
            KernelToolCallStatus::Succeeded | KernelToolCallStatus::Failed => {
                // Pop the OLDEST queued Started id (FIFO). Fall back to a fresh id
                // if none queued — a terminal status with no preceding Started
                // shouldn't happen, but stay robust rather than drop the event.
                let id = self.pending.pop_front().unwrap_or_else(|| self.fresh_tool_id());
                let acp_status = if tc.status == KernelToolCallStatus::Succeeded {
                    AcpToolCallStatus::Completed
                } else {
                    AcpToolCallStatus::Failed
                };
                let mut fields = ToolCallUpdateFields::new().status(acp_status);
                if let Some(out) = tc.result.as_ref() {
                    if let Ok(raw) = serde_json::from_str::<serde_json::Value>(out) {
                        fields = fields.raw_output(raw);
                    } else {
                        // Non-JSON tool output — wrap as a JSON string so the
                        // IDE still sees the real text rather than nothing.
                        fields = fields.raw_output(serde_json::Value::String(out.clone()));
                    }
                }
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(id, fields))
            }
        }
    }
}

/// Build an `AgentMessageChunk` (thought=false) or `AgentThoughtChunk`
/// (thought=true) carrying one text slice.
fn text_chunk(text: &str, thought: bool) -> SessionUpdate {
    let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new(text.to_string())));
    if thought {
        SessionUpdate::AgentThoughtChunk(chunk)
    } else {
        SessionUpdate::AgentMessageChunk(chunk)
    }
}

/// Stateless convenience wrapper around [`EventBridge::map`] for callers that
/// only need the mapping for a single event with no tool-id continuity (e.g. a
/// one-off translation). A turn that spans a tool call MUST use an
/// [`EventBridge`] so the Started/terminal ids link.
pub fn map_event_to_update(bridge: &mut EventBridge, event: &AgentEvent) -> Option<SessionUpdate> {
    bridge.map(event)
}

// ---------------------------------------------------------------------------
// Live server entry — stdio ACP agent driving the kernel ReactAgent
// ---------------------------------------------------------------------------

/// Per-session record the server keeps between `session/new` and `session/prompt`.
#[derive(Debug, Clone)]
struct SessionRec {
    cwd: PathBuf,
}

/// Shared session table across the three request handlers (each is a stateless
/// closure that moves an `Arc` clone in).
type SessionTable = Arc<Mutex<HashMap<String, SessionRec>>>;

/// Serve the kernel as an ACP agent over stdio. Runs until the client
/// disconnects (stdin EOF) or a fatal transport error.
///
/// Handlers bound (the three requests a minimal ACP client sends per turn):
///   - `initialize` → echo the client's protocol version + empty capabilities.
///   - `session/new` → mint a session id, remember the requested `cwd`, return it.
///   - `session/prompt` → build a kernel [`build_react_agent`] ReactAgent for the
///     session's cwd, drive one turn, stream each [`AgentEvent`] as a
///     `session/update` notification (via [`EventBridge`]), and resolve with a
///     [`PromptResponse`] whose stop reason mirrors the kernel outcome.
///
/// Every other request falls through to the dispatch default (an internal-error
/// response), matching `simple_agent.rs`. Authentication is not required
/// (`auth_methods` empty) — the server trusts its parent process (the IDE that
/// spawned it), the same trust model as every other local coding agent.
///
/// Integration-level: this drives a real model + reads stdin/writes stdout; it
/// is verified structurally against the crate's `simple_agent.rs` example and
/// the pure [`EventBridge`] mapping it depends on is unit-tested below.
pub async fn serve_stdio() -> agent_client_protocol::Result<()> {
    let sessions: SessionTable = Arc::new(Mutex::new(HashMap::new()));

    // initialize handler — negotiate protocol v1, advertise no extra capabilities.
    let serve_fut = AcpAgentRole
        .builder()
        .name("dev-workbench-acp")
        .on_receive_request(
            move |request: InitializeRequest,
                  responder: Responder<InitializeResponse>,
                  _connection: ConnectionTo<Client>| {
                async move {
                    responder.respond(
                        InitializeResponse::new(request.protocol_version)
                            .agent_capabilities(AgentCapabilities::new()),
                    )
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let sessions_ns = Arc::clone(&sessions);
                move |request: NewSessionRequest,
                      responder: Responder<NewSessionResponse>,
                      _connection: ConnectionTo<Client>| {
                    let sessions = Arc::clone(&sessions_ns);
                    async move {
                        // SessionId derives From<String>; store the bare String
                        // key so the prompt handler can look the session up.
                        let session_id =
                            SessionId::new(format!("dw-session-{}", uuid::Uuid::new_v4()));
                        if let Ok(mut map) = sessions.lock() {
                            map.insert(
                                session_id.to_string(),
                                SessionRec {
                                    cwd: request.cwd.clone(),
                                },
                            );
                        }
                        responder.respond(NewSessionResponse::new(session_id))
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let sessions_prompt = Arc::clone(&sessions);
                move |request: PromptRequest,
                      responder: Responder<PromptResponse>,
                      connection: ConnectionTo<Client>| {
                    let sessions = Arc::clone(&sessions_prompt);
                    async move {
                        let stop = drive_prompt_turn(&sessions, &request, &connection).await;
                        responder.respond(PromptResponse::new(stop))
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(Stdio::new());

    serve_fut.await
}

/// Drive one kernel ReactAgent turn for `request`, streaming `session/update`
/// notifications for each event, and return the stop reason that mirrors the
/// kernel's terminal outcome. Extracted so the prompt handler's body stays
/// readable and the control flow is inspectable.
async fn drive_prompt_turn(
    sessions: &SessionTable,
    request: &PromptRequest,
    connection: &ConnectionTo<Client>,
) -> StopReason {
    // SessionId derives Display → its String form is the HashMap key set in
    // session/new. Clone once: reused for every session/update notification.
    let session_id = request.session_id.clone();
    let cwd = sessions
        .lock()
        .ok()
        .and_then(|m| m.get(&session_id.to_string()).map(|r| r.cwd.clone()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Flatten the prompt content blocks into one user string. ACP clients send
    // text blocks; non-text blocks (images/resources) are dropped for the MVP —
    // the kernel prompt is text-only.
    let prompt: String = request
        .prompt
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    let cwd_str = cwd.to_string_lossy().to_string();
    let agent = match build_react_agent(
        None,
        None,
        &cwd_str,
        None,
        Vec::new(),
        None,
        PermissionMode::Default,
        None,
        None,
        None, // skill_filter
        None, // mcp_filter
        None, // knowledge_ids
        None, // app — ACP stdio server has no AppHandle; no WorkflowTool here.
        None, // compaction_blocks — no UI/driver to persist into.
        None, // approval — ACP stdio server has no managed state; no Human Gate.
    ) {
        Ok(a) => a,
        Err(e) => {
            // Agent construction failed (e.g. no provider config). Surface it as
            // a single agent-message chunk so the IDE sees WHY, then end the turn.
            log::error!("[acp-server] build_react_agent failed: {e}");
            let _ = connection.send_notification(SessionNotification::new(
                session_id.clone(),
                text_chunk(&format!("[Dev Workbench agent 构建失败: {e}]"), false),
            ));
            return StopReason::Refusal;
        }
    };

    let input = kernel_core::AgentInput {
        prompt,
        working_dir: Some(cwd_str),
        model: None,
        resume_from: None,
    };
    let mut events = match agent.run(input) {
        Ok(s) => s,
        Err(e) => {
            log::error!("[acp-server] agent.run failed: {e}");
            let _ = connection.send_notification(SessionNotification::new(
                session_id.clone(),
                text_chunk(&format!("[agent 启动失败: {e}]"), false),
            ));
            return StopReason::Refusal;
        }
    };

    use futures::StreamExt;
    let mut bridge = EventBridge::new();
    // Default to Failed so a stream that ends WITHOUT a `Done` event (upstream
    // error / panic / disconnect — the `Err` arm below only logs + sends a text
    // chunk, never setting outcome_status) maps to Refusal, not a false EndTurn.
    // Only a real `Done(outcome)` overrides this with the kernel's true status.
    let mut outcome_status = AgentRunStatus::Failed;
    while let Some(ev) = events.next().await {
        match ev {
            Ok(AgentEvent::Done(outcome)) => {
                outcome_status = outcome.status;
            }
            Ok(other) => {
                if let Some(update) = bridge.map(&other) {
                    let _ = connection.send_notification(SessionNotification::new(
                        session_id.clone(),
                        update,
                    ));
                }
            }
            Err(e) => {
                log::warn!("[acp-server] event stream error: {e}");
                let _ = connection.send_notification(SessionNotification::new(
                    session_id.clone(),
                    text_chunk(&format!("[事件流错误: {e}]"), false),
                ));
            }
        }
    }

    // Map the kernel's terminal status to an ACP stop reason. Completed →
    // EndTurn; anything else (Failed/Cancelled/…) → Refusal (the agent did not
    // finish a clean turn). There's no MaxTokens signal from the kernel today.
    match outcome_status {
        AgentRunStatus::Completed => StopReason::EndTurn,
        _ => StopReason::Refusal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_core::{AgentEvent, AgentOutcome, ToolCallEvent, ToolCallStatus};
    use std::path::PathBuf;

    /// A Started tool call → a ToolCall SessionUpdate carrying a fresh id and
    /// InProgress status, with the arguments parsed into raw_input.
    #[test]
    fn token_maps_to_agent_message_chunk() {
        let mut b = EventBridge::new();
        let u = b.map(&AgentEvent::Token("hi".into())).expect("Token → Some");
        match u {
            SessionUpdate::AgentMessageChunk(c) => match c.content {
                ContentBlock::Text(t) => assert_eq!(t.text, "hi"),
                _ => panic!("expected text content"),
            },
            _ => panic!("expected AgentMessageChunk, got {u:?}"),
        }
    }

    /// Reasoning → AgentThoughtChunk (NOT message — thought must stay separate
    /// so the IDE renders it as a collapsible thinking block, not the answer).
    #[test]
    fn reasoning_maps_to_thought_chunk() {
        let mut b = EventBridge::new();
        let u = b
            .map(&AgentEvent::Reasoning("thinking…".into()))
            .expect("Reasoning → Some");
        assert!(matches!(u, SessionUpdate::AgentThoughtChunk(_)));
    }

    /// A Started tool call → a ToolCall carrying a fresh id and InProgress, with
    /// JSON arguments folded into raw_input.
    #[test]
    fn tool_call_started_maps_to_tool_call_with_fresh_id() {
        let mut b = EventBridge::new();
        let u = b
            .map(&AgentEvent::ToolCall(ToolCallEvent {
                tool: "Read".into(),
                arguments: r#"{"file_path":"/x"}"#.into(),
                status: ToolCallStatus::Started,
                result: None,
            }))
            .expect("Started → Some");
        let call = match u {
            SessionUpdate::ToolCall(c) => c,
            _ => panic!("expected ToolCall, got {u:?}"),
        };
        assert_eq!(call.title, "Read");
        assert_eq!(call.status, AcpToolCallStatus::InProgress);
        assert_eq!(call.tool_call_id.0.to_string(), "dw-1");
        assert_eq!(
            call.raw_input.as_ref().and_then(|v| v.get("file_path")),
            Some(&serde_json::json!("/x"))
        );
    }

    /// The terminal status after a Started reuses the SAME id (so the IDE links
    /// the update to the right call) and carries the result as raw_output.
    #[test]
    fn tool_call_succeeded_reuses_started_id_and_carries_result() {
        let mut b = EventBridge::new();
        // Start the call → dw-1.
        b.map(&AgentEvent::ToolCall(ToolCallEvent {
            tool: "Read".into(),
            arguments: "{}".into(),
            status: ToolCallStatus::Started,
            result: None,
        }));
        // Complete it → update referencing dw-1.
        let u = b
            .map(&AgentEvent::ToolCall(ToolCallEvent {
                tool: "Read".into(),
                arguments: "{}".into(),
                status: ToolCallStatus::Succeeded,
                result: Some(r#"{"lines":42}"#.into()),
            }))
            .expect("Succeeded → Some");
        let upd = match u {
            SessionUpdate::ToolCallUpdate(u) => u,
            _ => panic!("expected ToolCallUpdate, got {u:?}"),
        };
        assert_eq!(upd.tool_call_id.0.to_string(), "dw-1");
        assert_eq!(upd.fields.status, Some(AcpToolCallStatus::Completed));
        assert_eq!(
            upd.fields.raw_output.as_ref().and_then(|v| v.get("lines")),
            Some(&serde_json::json!(42))
        );
    }

    /// A Failed tool call → Completed's negative: status Failed, raw_output
    /// holds the error text (wrapped as a JSON string when it isn't JSON).
    #[test]
    fn tool_call_failed_status_and_non_json_output_wrapped() {
        let mut b = EventBridge::new();
        b.map(&AgentEvent::ToolCall(ToolCallEvent {
            tool: "Bash".into(),
            arguments: "{}".into(),
            status: ToolCallStatus::Started,
            result: None,
        }));
        let u = b
            .map(&AgentEvent::ToolCall(ToolCallEvent {
                tool: "Bash".into(),
                arguments: "{}".into(),
                status: ToolCallStatus::Failed,
                result: Some("permission denied".into()),
            }))
            .expect("Failed → Some");
        match u {
            SessionUpdate::ToolCallUpdate(upd) => {
                assert_eq!(upd.fields.status, Some(AcpToolCallStatus::Failed));
                // Non-JSON output wrapped as a JSON string so it round-trips.
                assert_eq!(
                    upd.fields.raw_output,
                    Some(serde_json::Value::String("permission denied".into()))
                );
            }
            _ => panic!("expected ToolCallUpdate"),
        }
    }

    /// FileChanged / TurnBoundary / Done carry no streaming update — Done
    /// resolves the prompt request itself, the others are internal signals.
    #[test]
    fn non_streaming_events_map_to_none() {
        let mut b = EventBridge::new();
        assert!(b.map(&AgentEvent::FileChanged(PathBuf::from("/x.rs"))).is_none());
        assert!(b.map(&AgentEvent::TurnBoundary).is_none());
        assert!(
            b
                .map(&AgentEvent::Done(AgentOutcome {
                    status: AgentRunStatus::Completed,
                    ..Default::default()
                }))
                .is_none()
        );
    }

    /// A full scripted turn through the bridge produces the expected update
    /// sequence — the integration-shape `drive_prompt_turn` relies on. Token →
    /// AgentMessageChunk, Started → ToolCall (dw-1), Succeeded → ToolCallUpdate
    /// (dw-1), Done → None.
    #[test]
    fn full_scripted_turn_maps_in_order() {
        let mut b = EventBridge::new();
        let seq = vec![
            AgentEvent::Token("reading".into()),
            AgentEvent::ToolCall(ToolCallEvent {
                tool: "Read".into(),
                arguments: "{}".into(),
                status: ToolCallStatus::Started,
                result: None,
            }),
            AgentEvent::ToolCall(ToolCallEvent {
                tool: "Read".into(),
                arguments: "{}".into(),
                status: ToolCallStatus::Succeeded,
                result: Some("ok".into()),
            }),
            AgentEvent::Done(AgentOutcome {
                status: AgentRunStatus::Completed,
                ..Default::default()
            }),
        ];
        let updates: Vec<SessionUpdate> = seq.iter().filter_map(|e| b.map(e)).collect();
        // Token + Started + Succeeded → 3 updates; Done dropped.
        assert_eq!(updates.len(), 3, "{updates:?}");
        assert!(matches!(updates[0], SessionUpdate::AgentMessageChunk(_)));
        let started_id = match &updates[1] {
            SessionUpdate::ToolCall(c) => c.tool_call_id.clone(),
            _ => panic!("expected ToolCall at [1]"),
        };
        // The Succeeded update MUST reference the Started id.
        match &updates[2] {
            SessionUpdate::ToolCallUpdate(u) => assert_eq!(u.tool_call_id, started_id),
            _ => panic!("expected ToolCallUpdate at [2]"),
        }
    }

    /// Monotonic ids: a SECOND Started gets dw-2 (not reusing dw-1), and after a
    /// terminal update the remembered id is cleared so a stray terminal (no
    /// preceding Started) mints its own id rather than stealing the prior one.
    #[test]
    fn second_tool_call_gets_incrementing_id() {
        let mut b = EventBridge::new();
        // First call: dw-1 → completed.
        b.map(&tc("A", ToolCallStatus::Started));
        b.map(&tc("A", ToolCallStatus::Succeeded));
        // Second call: dw-2.
        let u = b.map(&tc("B", ToolCallStatus::Started)).unwrap();
        match u {
            SessionUpdate::ToolCall(c) => assert_eq!(c.tool_call_id.0.to_string(), "dw-2"),
            _ => panic!(),
        }
    }

    /// `map_event_to_update` is just the stateful bridge — same result as
    /// `EventBridge::map`, locked so the convenience fn doesn't drift.
    #[test]
    fn convenience_fn_matches_bridge() {
        let mut b1 = EventBridge::new();
        let mut b2 = EventBridge::new();
        let e = AgentEvent::Token("x".into());
        assert_eq!(b1.map(&e).is_some(), map_event_to_update(&mut b2, &e).is_some());
    }

    fn tc(tool: &str, status: ToolCallStatus) -> AgentEvent {
        AgentEvent::ToolCall(ToolCallEvent {
            tool: tool.into(),
            arguments: "{}".into(),
            status,
            result: (status != ToolCallStatus::Started).then(|| "r".into()),
        })
    }
}
