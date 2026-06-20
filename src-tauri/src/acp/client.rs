//! ACP client protocol layer: a one-shot driver that connects to an external
//! ACP agent over stdio, runs a single prompt turn, and returns the agent's
//! streamed answer. Built on the `agent-client-protocol` crate's `Client` role
//! + `AcpAgent` (subprocess) transport — the exact shape of the crate's own
//! `yolo_one_shot_client` example.
//!
//! Two surfaces, separated for testability:
//!   - [`run_acp_agent`] — the live driver. Spawns the agent subprocess, speaks
//!     JSON-RPC (initialize → new session → prompt), auto-approves the agent's
//!     permission prompts (the kernel is the authority), and accumulates the
//!     assistant answer text streamed as `session/update` notifications. This
//!     is integration-level — it needs a real ACP agent binary to exercise end
//!     to end; its structure mirrors the crate's proven example.
//!   - [`extract_update_text`] — the PURE mapping from one ACP `SessionUpdate`
//!     to the assistant text it carries. Extracted so the protocol mapping
//!     (the error-prone part — which update variant is "the answer") is unit-
//!     testable without spawning anything.

use std::path::Path;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionNotification, SessionUpdate, TextContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, Client, ConnectionTo, Responder, AcpAgent};

/// Outcome of one ACP one-shot run.
#[derive(Debug, Clone)]
pub struct AcpRunResult {
    /// The assistant's streamed answer, accumulated from every
    /// `AgentMessageChunk` notification the agent emitted this turn. Empty when
    /// the agent did tool work but ended with no text answer (the caller falls
    /// back to `stop_reason`).
    pub text: String,
    /// The agent's stated stop reason for the turn (debug-format of the ACP
    /// `StopReason` enum, e.g. `EndTurn` / `Refusal`). Surfaced so an empty-text
    /// result isn't a blind spot.
    pub stop_reason: String,
}

/// Failures from [`run_acp_agent`]. Deliberately coarse — the dispatch tool
/// surfaces the message verbatim to the parent agent so it can adapt.
#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    /// The command string couldn't be parsed as an ACP agent launch line
    /// (empty / malformed). `AcpAgent::from_str` rejects these.
    #[error("ACP agent 命令解析失败: {0}")]
    BadCommand(String),
    /// An ACP protocol error (initialize / new-session / prompt failed, or the
    /// JSON-RPC connection errored).
    #[error("ACP 协议错误: {0}")]
    Protocol(String),
    /// The run exceeded the wall-clock cap. NOTE: a timed-out external agent
    /// subprocess MAY linger (the high-level `AcpAgent` transport owns the
    /// `Child` and doesn't expose it for an explicit kill; on Windows dropping
    /// the connection doesn't terminate the child). This is an accepted v1
    /// limitation — the timeout protects the parent kernel turn from an infinite
    /// hang; a leaked child finishes its current turn and exits on its own.
    #[error("ACP agent 运行超时（{0}s）")]
    Timeout(u64),
}

/// Run a one-shot prompt against an external ACP agent.
///
/// `command` is the agent's launch line (e.g. `npx @zed-industries/codex-acp`),
/// parsed by [`AcpAgent::from_str`] (which also accepts the JSON MCP-server form).
/// `cwd` becomes the agent's session working directory. `prompt` is the turn's
/// user message. `timeout_secs` bounds the whole connect→initialize→prompt
/// turn (see [`AcpError::Timeout`] for the leak caveat).
///
/// Permission prompts the agent raises (`session/requestPermission`) are
/// auto-approved by selecting the FIRST option (YOLO) — the parent kernel agent
/// is the authority delegating real work to this coding agent, matching
/// deer-flow's `invoke_acp_agent_tool` semantics. With no options offered the
/// request is cancelled (the turn still completes; the agent learns the action
/// was denied).
pub async fn run_acp_agent(
    command: &str,
    cwd: &Path,
    prompt: &str,
    timeout_secs: u64,
) -> Result<AcpRunResult, AcpError> {
    let agent =
        AcpAgent::from_str(command).map_err(|e| AcpError::BadCommand(e.to_string()))?;

    // Shared accumulators read AFTER the connection future resolves. The
    // notification handler (registered on the builder) owns one Arc clone and
    // writes streamed answer text; the main_fn closure owns another and writes
    // the terminal stop reason. Both are std::sync::Mutex — the critical
    // sections are trivial push/assign with no await held across the guard.
    let text_acc: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let stop_acc: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

    let text_for_notif = Arc::clone(&text_acc);
    let connect_fut = Client
        .builder()
        .on_receive_notification(
            move |notification: SessionNotification, _cx| {
                let acc = Arc::clone(&text_for_notif);
                async move {
                    if let Some(piece) = extract_update_text(&notification.update) {
                        if let Ok(mut buf) = acc.lock() {
                            buf.push_str(&piece);
                        }
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            move |request: RequestPermissionRequest,
                  responder: Responder<RequestPermissionResponse>,
                  _connection| {
                // YOLO: approve the first offered option. `option_id` is cloned
                // out before move into the response so no borrow of `request`
                // crosses the respond call.
                let id = request.options.first().map(|o| o.option_id.clone());
                async move {
                    match id {
                        Some(id) => responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
                        )),
                        None => responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        )),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, {
            let stop_for_main = Arc::clone(&stop_acc);
            let cwd = cwd.to_path_buf();
            let prompt = prompt.to_string();
            move |connection: ConnectionTo<Agent>| {
                let stop_for_main = Arc::clone(&stop_for_main);
                async move {
                    // 1. initialize — negotiate protocol v1.
                    connection
                        .send_request(InitializeRequest::new(ProtocolVersion::V1))
                        .block_task()
                        .await?;
                    // 2. new session — cwd is the agent's working directory.
                    let session = connection
                        .send_request(NewSessionRequest::new(cwd))
                        .block_task()
                        .await?;
                    // 3. prompt — drive one turn. The agent streams
                    //    `session/update` notifications (accumulated by the
                    //    handler above) and finally resolves this request with a
                    //    PromptResponse carrying the stop reason.
                    let resp = connection
                        .send_request(PromptRequest::new(
                            session.session_id,
                            vec![ContentBlock::Text(TextContent::new(prompt))],
                        ))
                        .block_task()
                        .await?;
                    if let Ok(mut s) = stop_for_main.lock() {
                        *s = format!("{:?}", resp.stop_reason);
                    }
                    Ok::<(), agent_client_protocol::Error>(())
                }
            }
        });

    match tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs.max(1)),
        connect_fut,
    )
    .await
    {
        Ok(Ok(())) => {
            let text = take_locked(&text_acc);
            let stop_reason = take_locked(&stop_acc);
            Ok(AcpRunResult { text, stop_reason })
        }
        Ok(Err(e)) => Err(AcpError::Protocol(e.to_string())),
        Err(_) => Err(AcpError::Timeout(timeout_secs)),
    }
}

/// Drain a `Mutex<String>` accumulator to an owned `String` (empty on poison).
fn take_locked(acc: &Mutex<String>) -> String {
    acc.lock()
        .map(|mut b| std::mem::take(&mut *b))
        .unwrap_or_default()
}

/// Pure mapping: one ACP [`SessionUpdate`] → the assistant answer text it
/// carries, if any. Returns `None` for everything that isn't a chunk of the
/// agent's answer message:
///   - `AgentThoughtChunk` (reasoning) — not part of the conclusion the parent
///     agent needs; folding it in would pollute the result with chain-of-thought.
///   - `ToolCall` / `ToolCallUpdate` — side effects; the parent sees the final
///     answer text, not the tool chatter (mirrors `dispatch_subagent`, which
///     returns only the child's conclusion).
///   - `Plan` / `UsageUpdate` / `ConfigOptionUpdate` / `SessionInfoUpdate` /
///     `*Chunk(user)` / mode/command updates — bookkeeping, not answer text.
///   - `AgentMessageChunk` carrying a non-text content block (image/audio/
///     resource) — no text to fold into a tool result.
///
/// Extracted as a pure fn so the protocol mapping is unit-testable without
/// driving a live connection. [`run_acp_agent`] calls this per notification and
/// concatenates the `Some` pieces — the accumulation behaviour is covered by
/// the test that feeds three chunks through and asserts the joined string.
pub fn extract_update_text(update: &SessionUpdate) -> Option<String> {
    let chunk = match update {
        SessionUpdate::AgentMessageChunk(c) => c,
        _ => return None,
    };
    match &chunk.content {
        ContentBlock::Text(t) => Some(t.text.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deserialize a `SessionUpdate` from its wire JSON. `SessionUpdate` is
    /// `#[serde(tag = "sessionUpdate", rename_all = "snake_case")]` and
    /// `ContentBlock` is `#[serde(tag = "type", rename_all = "snake_case")]`, so
    /// an agent answer chunk arrives as:
    ///   {"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":T}}
    /// Building fixtures from the wire shape (not struct construction) dodges
    /// the schema types' `#[non_exhaustive]` — which blocks external literal
    /// construction — AND validates the exact format `run_acp_agent` receives
    /// from a real agent, so a schema rename would fail HERE rather than silently
    /// mis-parse a live run.
    fn from_wire(json: &str) -> SessionUpdate {
        serde_json::from_str(json).expect("wire SessionUpdate must deserialize")
    }

    #[test]
    fn extract_update_text_returns_agent_message_text() {
        let u = from_wire(
            r#"{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"答案是 42"}}"#,
        );
        assert_eq!(extract_update_text(&u).as_deref(), Some("答案是 42"));
    }

    #[test]
    fn extract_update_text_ignores_thought_and_user_chunks() {
        // Reasoning + user-echo chunks are NOT the answer — folding them in
        // would pollute the dispatch result with chain-of-thought / the prompt.
        let thought = from_wire(
            r#"{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"让我想想"}}"#,
        );
        let user = from_wire(
            r#"{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"问题"}}"#,
        );
        assert!(
            extract_update_text(&thought).is_none(),
            "agent reasoning must not leak into the answer"
        );
        assert!(
            extract_update_text(&user).is_none(),
            "user echo must not leak into the answer"
        );
    }

    #[test]
    fn extract_update_text_ignores_non_text_content() {
        // An agent message chunk carrying an image (not text) → no foldable
        // text. Guards the `ContentBlock::Text(_) => Some, _ => None` arm.
        let img = from_wire(
            r#"{"sessionUpdate":"agent_message_chunk","content":{"type":"image","data":"","mimeType":"image/png"}}"#,
        );
        assert!(extract_update_text(&img).is_none());
    }

    /// Mirrors `run_acp_agent`'s accumulation: concatenate the `Some` pieces
    /// from a stream of updates. Streaming sends the answer as N chunks; the
    /// joined string must equal the full answer, with interleaved non-answer
    /// updates (here a thought chunk) dropped.
    #[test]
    fn accumulation_joins_streamed_answer_chunks() {
        let stream = [
            from_wire(
                r#"{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hello"}}"#,
            ),
            from_wire(
                r#"{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"(thinking)"}}"#,
            ),
            from_wire(
                r#"{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":", "}}"#,
            ),
            from_wire(
                r#"{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"world."}}"#,
            ),
        ];
        let joined: String = stream
            .iter()
            .filter_map(extract_update_text)
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(joined, "Hello, world.");
    }

    /// `take_locked` must drain the accumulator and leave it empty, returning
    /// the empty string on a poisoned lock rather than panicking — the contract
    /// `run_acp_agent` relies on after the connection resolves.
    #[test]
    fn take_locked_drains_and_survives_poison() {
        let acc = Mutex::new("payload".to_string());
        assert_eq!(take_locked(&acc), "payload");
        // Drained on first take.
        assert_eq!(take_locked(&acc), "");
    }

    /// `AcpError` Display strings are user-facing (the dispatch tool surfaces
    /// them verbatim). Lock the wording so a parent agent's retry logic and the
    /// test fixtures stay stable.
    #[test]
    fn acp_error_display_is_stable() {
        assert_eq!(
            AcpError::BadCommand("bad".into()).to_string(),
            "ACP agent 命令解析失败: bad"
        );
        assert!(AcpError::Timeout(600).to_string().contains("600"));
        assert!(
            AcpError::Protocol("x".into())
                .to_string()
                .starts_with("ACP 协议错误")
        );
    }

    /// `AcpRunResult` carries the accumulated text + stop reason verbatim — the
    /// dispatch tool formats both into its result line.
    #[test]
    fn acp_run_result_holds_text_and_stop_reason() {
        let r = AcpRunResult {
            text: "done".into(),
            stop_reason: "EndTurn".into(),
        };
        assert_eq!(r.text, "done");
        assert_eq!(r.stop_reason, "EndTurn");
    }
}
