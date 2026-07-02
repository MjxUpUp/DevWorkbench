//! ReactAgent → chat wire-event mapping.
//!
//! The transparent ReactAgent (kernel layer) emits kernel-core `AgentEvent`s.
//! The chat UI consumes `ChatStreamEvent`s — the same wire schema claude's
//! stream-json parser produces (see `pty.rs`). This module is the bridge: a pure
//! `map_agent_event` turning one `AgentEvent` into zero or more
//! `ChatStreamEvent`s, so the ReactAgent reuses the EXACT BlocksView rendering
//! path claude uses. No second presentation layer, no terminal serialization.
//!
//! Design note (plan D4): kernel-core's `AgentEvent` deliberately has NO serde
//! derive — it's a domain model that must evolve independently of the wire
//! schema. So we map by hand here instead of coupling the two with a derive on
//! the enum. `ChatStreamEvent` is the UI schema; this fn is the only thing that
//! knows both sides.

use crate::agents::pty::ChatStreamEvent;
use crate::models::{Session, SessionStatus};
use kernel_core::{
    AgentEvent, AgentRunStatus, Message, Role, ToolCallEvent, ToolCallStatus,
};
use serde_json::Value;
use std::collections::VecDeque;

/// Map one kernel-core `AgentEvent` to zero or more chat wire events for the
/// `agent:event` channel. Pure + testable: the caller passes `secs` (elapsed
/// since the run started) so the Result block's duration is deterministic under
/// test — this fn has no time side-effect of its own.
///
/// Mapping:
/// - `Token(s)`           → `[Text{content: s}]`
/// - `ToolCall` Started   → `[ToolUse{name, input: parse(arguments)}]`
/// - `ToolCall` Succeeded → `[ToolResult{content: "(ok)",   is_error: false}]`
/// - `ToolCall` Failed    → `[ToolResult{content: "(failed)", is_error: true}]`
/// - `FileChanged(p)`     → `[FileChanged{path: p}]` (per-write mutation line)
/// - `TurnBoundary`       → `[]` (same)
/// - `Done(outcome)`      → `[Result{is_error: status != Completed, secs}]`
///
/// NB: the transparent ReactAgent now fills `ToolCallEvent.result` with the real
/// tool output (see `react_agent::run`), so Succeeded/Failed map to the actual
/// content. The `"(ok)"/"(failed)"` fallback only applies when an emitter
/// reports status without a result (e.g. some opaque-agent reverse-mapping paths).
pub fn map_agent_event(ev: AgentEvent, secs: u64) -> Vec<ChatStreamEvent> {
    match ev {
        AgentEvent::Token(s) => vec![ChatStreamEvent::Text { content: s }],
        AgentEvent::Reasoning(s) => vec![ChatStreamEvent::Thinking { content: s }],
        AgentEvent::ToolCall(tc) => match tc.status {
            ToolCallStatus::Started => vec![ChatStreamEvent::ToolUse {
                name: tc.tool,
                input: parse_tool_arguments(&tc.arguments),
            }],
            ToolCallStatus::Succeeded => vec![ChatStreamEvent::ToolResult {
                content: tc.result.unwrap_or_else(|| "(ok)".to_string()),
                is_error: false,
            }],
            ToolCallStatus::Failed => vec![ChatStreamEvent::ToolResult {
                content: tc.result.unwrap_or_else(|| "(failed)".to_string()),
                is_error: true,
            }],
        },
        AgentEvent::FileChanged(p) => vec![ChatStreamEvent::FileChanged {
            path: p.display().to_string(),
        }],
        AgentEvent::TurnBoundary => Vec::new(),
        AgentEvent::Done(outcome) => {
            vec![ChatStreamEvent::Result {
                is_error: outcome.status != AgentRunStatus::Completed,
                secs,
            }]
        }
    }
}

/// Parse a tool's raw arguments string into a JSON value for the ToolUse card.
/// The transparent agent always emits valid JSON (LLM tool-call arguments); if
/// it's ever empty or malformed, fall back to `null` rather than panicking the
/// stream — the card renders `null` harmlessly.
fn parse_tool_arguments(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Value::Null;
    }
    serde_json::from_str(trimmed).unwrap_or(Value::Null)
}

/// Reverse of `map_agent_event`: turn a claude `agent:event` wire block back
/// into kernel-core `AgentEvent`s for the OpaqueAgent stream. Unlike the
/// forward map (ReactAgent Succeeded/Failed → placeholder "(ok)"), claude's
/// ToolResult.content is the REAL tool output — but `ToolCallEvent` has no
/// content field (kernel-core/agent.rs:81), so the reverse map restores only
/// name/arguments via positional pairing. The workflow path's downstream
/// `map_agent_to_chunks` then re-emits the ReactAgent "(ok)"/"(failed)"
/// placeholder for the tool_result card (inherited behavior, same as a
/// transparent agent's tool call).
///
/// `pending_tools`: FIFO queue of (name, arguments_json) — enqueued on ToolUse,
/// dequeued on ToolResult. The wire schema carries no tool_call_id
/// (`ClaudeBlock::ToolResult`'s id is dropped in `to_event`, pty.rs:470), so
/// pairing is positional. **FIFO (not LIFO) is required**: claude may emit
/// multiple tool_use blocks in one assistant turn, then their tool_results in
/// the same id order (use(A), use(B), result(A), result(B)) — a LIFO stack
/// would mis-pair result(A) onto B. FIFO is order-correct under both the
/// alternating (use,result,use,result) and batched (use,use,result,result)
/// emission shapes.
///
/// Mapping:
/// - `Text{content}`                → `[Token(content)]`
/// - `ToolUse{name, input}`         → enqueue; `[ToolCall(Started)]`
/// - `ToolResult{content, is_error}`→ dequeue; paired `[ToolCall(Succeeded|Failed)]`,
///   orphan (queue empty) `[Token(content)]` (demote — never drop the signal)
/// - `Result{..}`                   → `[]` (Done owned by agent:completed;
///   emitting here would duplicate the terminal event and double-end the stream)
pub fn chat_event_to_agent_events(
    ev: &ChatStreamEvent,
    pending_tools: &mut VecDeque<(String, String)>,
) -> Vec<AgentEvent> {
    match ev {
        ChatStreamEvent::Text { content } => vec![AgentEvent::Token(content.clone())],
        ChatStreamEvent::Thinking { content } => vec![AgentEvent::Reasoning(content.clone())],
        ChatStreamEvent::ToolUse { name, input } => {
            let args = serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
            pending_tools.push_back((name.clone(), args.clone()));
            vec![AgentEvent::ToolCall(ToolCallEvent {
                tool: name.clone(),
                arguments: args,
                status: ToolCallStatus::Started,
                result: None,
            })]
        }
        ChatStreamEvent::ToolResult { content, is_error } => match pending_tools.pop_front() {
            // Paired: restore name/arguments from the OLDEST pending ToolUse (FIFO).
            Some((name, args)) => {
                let status = if *is_error {
                    ToolCallStatus::Failed
                } else {
                    ToolCallStatus::Succeeded
                };
                vec![AgentEvent::ToolCall(ToolCallEvent {
                    tool: name,
                    arguments: args,
                    status,
                    // claude's ToolResult.content IS the real tool output —
                    // carry it through so downstream renders the actual result.
                    result: Some(content.clone()),
                })]
            }
            // Orphan (no pending ToolUse): demote content to a Token so it
            // surfaces as text rather than vanishing. Do NOT fabricate a
            // ToolCall(Started) — would desync downstream use/result counts.
            None => {
                log::warn!(
                    "[chat_event_to_agent_events] orphan ToolResult; content demoted to text: {}",
                    content.chars().take(80).collect::<String>()
                );
                vec![AgentEvent::Token(content.clone())]
            }
        },
        // Done is owned by agent:completed; emitting here would duplicate the
        // terminal event and double-end the stream.
        ChatStreamEvent::Result { .. } => Vec::new(),
        // FileChanged never arrives on the chat wire from an opaque CLI (CLIs
        // don't surface per-write events); it's emitted only by the transparent
        // ReactAgent forward path. The reverse map exists for the OpaqueAgent
        // stream, so this arm keeps the match exhaustive as a no-op rather than
        // fabricating a kernel event.
        ChatStreamEvent::FileChanged { .. } => Vec::new(),
        // Compact is a meta-event emitted by the compaction sink, never by an
        // opaque CLI — kept exhaustive. It carries no kernel AgentEvent (it
        // never enters the model's stream, only the UI's block list).
        ChatStreamEvent::Compact { .. } => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Multi-turn history — turn prior Sessions back into kernel-core `Message`s so
// the ReactAgent resumes a conversation with real context. Symmetric to the
// CLI path's `inject_conversation_context` (pty.rs), but structured: blocks
// (text/tool_use/tool_result) round-trip into assistant.tool_calls + tool
// messages instead of a flattened output_summary string. Without this the
// self-built agent sees each turn in isolation — the last structural gap vs
// the CLI agents.
// ---------------------------------------------------------------------------

/// Per-turn char budget for one prior turn's assistant text. Large enough for a
/// full reply, small enough that a runaway turn doesn't eat the whole history.
pub const REACT_HISTORY_TURN_TEXT_CAP: usize = 2000;
/// Total chars across ALL prior turns. Mirrors the CLI path's 8000 overall cap
/// but lifted a bit — structured tool turns carry more useful signal per char
/// than a flat summary does.
pub const REACT_HISTORY_TOTAL_TEXT_CAP: usize = 12000;
/// Hard cap on prior-turn messages before we start dropping whole turns.
pub const REACT_HISTORY_TOTAL_MESSAGES: usize = 40;

/// Keep the tail of `s` up to `max` chars, snapped to a UTF-8 boundary and
/// `...`-prefixed. Mirrors pty.rs's private `tail` — duplicated here to avoid
/// widening the CLI module's visibility just for one helper.
fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    format!("...{}", &s[start..])
}

/// Convert prior conversation turns (ASC by started_at, as returned by
/// `pty::load_prior_turns`) into kernel-core `Message`s for the ReactAgent's
/// `run()` history. Each completed/failed turn expands to:
///   user(prompt) + assistant(text+reasoning, tool calls STRIPPED)  (from blocks)
///   user(prompt) + assistant(output_summary)                        (legacy fallback)
///   user(prompt)                                                    (no output at all)
/// Running turns are skipped (no finalized content yet). Tool calls and their
/// raw results are NOT replayed into history (see blocks_to_assistant_message),
/// so the model never copies a prior turn's tool choice by example. When the
/// result exceeds the message or total-char caps, the OLDEST whole turns are
/// dropped — turns are never split mid-way, so a prompt and its assistant reply
/// always travel together.
pub fn turns_to_history(
    turns: &[Session],
    turn_text_cap: usize,
    total_text_cap: usize,
) -> Vec<Message> {
    // Build per-turn message groups, oldest-first. Each group is a whole turn:
    // user + assistant (+ its tool messages). Caps operate on whole groups.
    let mut groups: Vec<Vec<Message>> = Vec::new();
    for sess in turns {
        // Skip turns that haven't finalized — they have no assistant reply yet,
        // and emitting a lone user message would hand the model an unanswered
        // question as "history".
        if sess.status == SessionStatus::Running {
            continue;
        }
        let mut group: Vec<Message> = Vec::with_capacity(2);
        group.push(Message::user(sess.prompt.clone()));

        let assistant_msg = sess
            .blocks
            .as_ref()
            .filter(|v| !v.is_null())
            .and_then(|v| serde_json::from_value::<Vec<ChatStreamEvent>>(v.clone()).ok())
            .and_then(|blocks| {
                if blocks.is_empty() {
                    None
                } else {
                    blocks_to_assistant_message(&blocks, turn_text_cap)
                }
            })
            .or_else(|| {
                // No persisted blocks (raw agent, or pre-G1 session) → fall back
                // to the text-only summary so the model at least sees the reply.
                sess.output_summary
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|s| Message::assistant(tail(s, turn_text_cap)))
            });

        if let Some(msg) = assistant_msg {
            group.push(msg);
        }
        // A turn with only a user prompt (no assistant reply recorded) is still
        // useful context — the model learns the user asked this. Keep it.
        groups.push(group);
    }

    // Greedily keep newest turns until we breach a cap; then stop adding older.
    // This preserves the most recent context (most relevant for a follow-up)
    // and drops oldest whole turns.
    let mut kept: Vec<&Vec<Message>> = Vec::new();
    let mut msg_count = 0usize;
    let mut char_count = 0usize;
    for group in groups.iter().rev() {
        let group_chars: usize = group.iter().map(|m| m.content.len()).sum();
        let would_msgs = msg_count + group.len();
        let would_chars = char_count + group_chars;
        // Stop before adding a turn that would breach EITHER cap — unless we
        // have nothing yet (always keep at least the most recent turn).
        if !kept.is_empty()
            && (would_msgs > REACT_HISTORY_TOTAL_MESSAGES || would_chars > total_text_cap)
        {
            break;
        }
        msg_count = would_msgs;
        char_count = would_chars;
        kept.push(group);
    }
    // kept is newest-first; reverse back to chronological for the history.
    let mut out: Vec<Message> = Vec::new();
    for group in kept.into_iter().rev() {
        out.extend(group.iter().cloned());
    }
    out
}

/// Turn one prior session's persisted blocks back into a SINGLE assistant
/// message — text + reasoning only. ToolUse/ToolResult are deliberately
/// DROPPED from history for two reasons:
///
///   1. Context bloat — a raw tool result (a file body, a command's stdout) can
///      be kilobytes; replaying every prior tool call+result across turns fills
///      the window with content the model rarely needs verbatim.
///   2. Tool-selection pollution — replaying a prior tool_call teaches the model
///      "this is how we do X here" by example, so it re-plays the same command
///      (e.g. claude_code's `bash lark-cli`) instead of routing through the
///      matching skill abstraction (skill__lark-doc). The CLI path already
///      avoids this — it carries only a text summary — so the kernel path now
///      matches; the base system prompt reinforces it with explicit discipline.
///
/// A turn whose blocks are ALL tool calls (no assistant prose, no reasoning)
/// yields None: nothing survives the strip, and emitting an empty assistant
/// message would be vacuous (some providers reject empty assistant turns).
fn blocks_to_assistant_message(
    blocks: &[ChatStreamEvent],
    text_cap: usize,
) -> Option<Message> {
    let mut text_chunks: Vec<&str> = Vec::new();
    let mut reasoning_chunks: Vec<&str> = Vec::new();
    for ev in blocks {
        match ev {
            ChatStreamEvent::Text { content } => text_chunks.push(content.as_str()),
            ChatStreamEvent::Thinking { content } => reasoning_chunks.push(content.as_str()),
            // Stripped from history — see the doc comment above.
            ChatStreamEvent::ToolUse { .. } | ChatStreamEvent::ToolResult { .. } => {}
            ChatStreamEvent::Result { .. } => {} // terminal marker, not history content
            ChatStreamEvent::FileChanged { .. } => {} // per-write signal, not history prose
            ChatStreamEvent::Compact { .. } => {} // compaction meta-event, not history prose
        }
    }

    let assistant_text = tail(&text_chunks.join(""), text_cap);
    // Reassembled reasoning trace (opaque history that carried thinking blocks).
    // No signature survives the wire round-trip, so a replayed thinking block is
    // unsigned — only consequential once an opaque CLI actually emits thinking.
    let assistant_reasoning = tail(&reasoning_chunks.join(""), text_cap);
    if assistant_text.is_empty() && assistant_reasoning.is_empty() {
        return None;
    }
    Some(Message {
        role: Role::Assistant,
        content: assistant_text,
        tool_calls: Vec::new(),
        tool_call_id: None,
        reasoning: if assistant_reasoning.is_empty() {
            None
        } else {
            Some(assistant_reasoning)
        },
        reasoning_signature: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_core::{AgentOutcome, AgentRunStatus, ToolCallEvent};
    use std::path::PathBuf;

    #[test]
    fn token_maps_to_text_block() {
        let out = map_agent_event(AgentEvent::Token("hello".to_string()), 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::Text { content } => assert_eq!(content, "hello"),
            other => panic!("expected Text, got {:?}", other),
        }
    }

    #[test]
    fn reasoning_maps_to_thinking_block() {
        // GLM Interleaved Thinking surfaces as AgentEvent::Reasoning; the chat
        // layer maps it onto the Thinking wire block (collapsible UI), NOT Text.
        let out = map_agent_event(AgentEvent::Reasoning("why".to_string()), 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::Thinking { content } => assert_eq!(content, "why"),
            other => panic!("expected Thinking, got {:?}", other),
        }
    }

    #[test]
    fn thinking_wire_block_round_trips_to_reasoning_event() {
        // Reverse map (opaque → kernel): a Thinking wire block becomes a
        // Reasoning AgentEvent, independent of the tool-use pairing queue.
        let mut pending = std::collections::VecDeque::new();
        let evs = chat_event_to_agent_events(
            &ChatStreamEvent::Thinking { content: "deliberation".into() },
            &mut pending,
        );
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::Reasoning(s) => assert_eq!(s, "deliberation"),
            other => panic!("expected Reasoning, got {:?}", other),
        }
        assert!(pending.is_empty(), "thinking must not enqueue a tool pairing");
    }

    #[test]
    fn tool_call_started_maps_to_tool_use_with_parsed_input() {
        let ev = AgentEvent::ToolCall(ToolCallEvent {
            tool: "Read".to_string(),
            arguments: r#"{"file_path":"/a.txt"}"#.to_string(),
            status: ToolCallStatus::Started,
            result: None,
        });
        let out = map_agent_event(ev, 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::ToolUse { name, input } => {
                assert_eq!(name, "Read");
                assert_eq!(input["file_path"], "/a.txt");
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }
    }

    #[test]
    fn tool_call_succeeded_maps_to_ok_result() {
        let ev = AgentEvent::ToolCall(ToolCallEvent {
            tool: "Bash".to_string(),
            arguments: "{}".to_string(),
            status: ToolCallStatus::Succeeded,
            result: None,
        });
        let out = map_agent_event(ev, 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::ToolResult { content, is_error } => {
                assert_eq!(content, "(ok)");
                assert!(!is_error);
            }
            other => panic!("expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn tool_call_succeeded_with_result_maps_real_content() {
        // v1.1: ReactAgent now fills `result` with the real tool output — the
        // mapped ToolResult must carry that content, not the "(ok)" placeholder.
        let ev = AgentEvent::ToolCall(ToolCallEvent {
            tool: "Read".to_string(),
            arguments: "{}".to_string(),
            status: ToolCallStatus::Succeeded,
            result: Some("the file contents".to_string()),
        });
        let out = map_agent_event(ev, 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::ToolResult { content, is_error } => {
                assert_eq!(content, "the file contents");
                assert!(!is_error);
            }
            other => panic!("expected ToolResult with real content, got {:?}", other),
        }
    }

    #[test]
    fn tool_call_failed_with_result_maps_real_error() {
        let ev = AgentEvent::ToolCall(ToolCallEvent {
            tool: "Read".to_string(),
            arguments: "{}".to_string(),
            status: ToolCallStatus::Failed,
            result: Some("permission denied".to_string()),
        });
        let out = map_agent_event(ev, 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::ToolResult { content, is_error } => {
                assert_eq!(content, "permission denied");
                assert!(is_error);
            }
            other => panic!("expected ToolResult with real error, got {:?}", other),
        }
    }

    #[test]
    fn tool_call_failed_maps_to_error_result() {
        let ev = AgentEvent::ToolCall(ToolCallEvent {
            tool: "Bash".to_string(),
            arguments: "{}".to_string(),
            status: ToolCallStatus::Failed,
            result: None,
        });
        let out = map_agent_event(ev, 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::ToolResult { content, is_error } => {
                assert_eq!(content, "(failed)");
                assert!(is_error);
            }
            other => panic!("expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn file_changed_maps_to_file_changed_event() {
        // FileChanged is no longer dropped — it maps to a wire FileChanged
        // block so the chat UI renders per-write mutations as they land.
        let ev = AgentEvent::FileChanged(PathBuf::from("/x.rs"));
        let out = map_agent_event(ev, 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::FileChanged { path } => assert_eq!(path, "/x.rs"),
            other => panic!("expected FileChanged, got {:?}", other),
        }
    }

    #[test]
    fn turn_boundary_is_dropped() {
        assert!(map_agent_event(AgentEvent::TurnBoundary, 0).is_empty());
    }

    #[test]
    fn done_completed_maps_to_ok_result_with_elapsed_secs() {
        let outcome = AgentOutcome {
            status: AgentRunStatus::Completed,
            ..Default::default()
        };
        let out = map_agent_event(AgentEvent::Done(outcome), 42);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::Result { is_error, secs } => {
                assert!(!is_error);
                assert_eq!(*secs, 42);
            }
            other => panic!("expected Result, got {:?}", other),
        }
    }

    #[test]
    fn done_failed_maps_to_error_result() {
        let outcome = AgentOutcome {
            status: AgentRunStatus::Failed,
            ..Default::default()
        };
        let out = map_agent_event(AgentEvent::Done(outcome), 5);
        match &out[0] {
            ChatStreamEvent::Result { is_error, secs } => {
                assert!(is_error);
                assert_eq!(*secs, 5);
            }
            other => panic!("expected Result, got {:?}", other),
        }
    }

    #[test]
    fn done_cancelled_is_treated_as_error() {
        // Cancelled != Completed → red result. The UI must not show a misleading
        // green for a stopped run.
        let outcome = AgentOutcome {
            status: AgentRunStatus::Cancelled,
            ..Default::default()
        };
        let out = map_agent_event(AgentEvent::Done(outcome), 3);
        match &out[0] {
            ChatStreamEvent::Result { is_error, .. } => assert!(is_error),
            other => panic!("expected Result, got {:?}", other),
        }
    }

    #[test]
    fn parse_tool_arguments_empty_is_null() {
        assert_eq!(parse_tool_arguments(""), Value::Null);
        assert_eq!(parse_tool_arguments("   "), Value::Null);
    }

    #[test]
    fn parse_tool_arguments_malformed_is_null() {
        assert_eq!(parse_tool_arguments("not json"), Value::Null);
    }

    #[test]
    fn parse_tool_arguments_object_passes_through() {
        let v = parse_tool_arguments(r#"{"a":1,"b":"x"}"#);
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], "x");
    }

    // ---- chat_event_to_agent_events (reverse map for OpaqueAgent) ----

    /// Assert an AgentEvent is a ToolCall with the given name + status.
    fn assert_tool(ev: &AgentEvent, expected_name: &str, expected_status: ToolCallStatus) {
        match ev {
            AgentEvent::ToolCall(tc) => {
                assert_eq!(tc.tool, expected_name, "tool name mismatch");
                assert_eq!(tc.status, expected_status, "status mismatch");
            }
            other => panic!("expected ToolCall({expected_name}), got {:?}", other),
        }
    }

    #[test]
    fn chat_text_maps_to_token() {
        let mut pending = VecDeque::new();
        let out = chat_event_to_agent_events(
            &ChatStreamEvent::Text { content: "hi".into() },
            &mut pending,
        );
        assert_eq!(out.len(), 1);
        match &out[0] {
            AgentEvent::Token(s) => assert_eq!(s, "hi"),
            other => panic!("expected Token, got {:?}", other),
        }
        assert!(pending.is_empty(), "Text must not touch the pending queue");
    }

    #[test]
    fn chat_tool_use_enqueues_and_emits_started() {
        let mut pending = VecDeque::new();
        let out = chat_event_to_agent_events(
            &ChatStreamEvent::ToolUse {
                name: "Read".into(),
                input: serde_json::json!({"file_path": "/x"}),
            },
            &mut pending,
        );
        assert_eq!(out.len(), 1);
        assert_tool(&out[0], "Read", ToolCallStatus::Started);
        // Arguments round-trip the input JSON exactly (serde_json::to_string).
        match &out[0] {
            AgentEvent::ToolCall(tc) => assert_eq!(tc.arguments, r#"{"file_path":"/x"}"#),
            _ => unreachable!(),
        }
        assert_eq!(pending.len(), 1);
        let front = pending.front().unwrap();
        assert_eq!(front.0, "Read");
        assert_eq!(front.1, r#"{"file_path":"/x"}"#);
    }

    #[test]
    fn chat_tool_use_null_input_enqueues_null_args() {
        let mut pending = VecDeque::new();
        let out = chat_event_to_agent_events(
            &ChatStreamEvent::ToolUse { name: "X".into(), input: Value::Null },
            &mut pending,
        );
        match &out[0] {
            AgentEvent::ToolCall(tc) => assert_eq!(tc.arguments, "null"),
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn chat_tool_result_dequeues_and_emits_succeeded() {
        let mut pending = VecDeque::new();
        pending.push_back(("Read".to_string(), r#"{"file_path":"/x"}"#.to_string()));
        let out = chat_event_to_agent_events(
            &ChatStreamEvent::ToolResult { content: "res".into(), is_error: false },
            &mut pending,
        );
        assert_eq!(out.len(), 1);
        assert_tool(&out[0], "Read", ToolCallStatus::Succeeded);
        match &out[0] {
            AgentEvent::ToolCall(tc) => assert_eq!(tc.arguments, r#"{"file_path":"/x"}"#),
            _ => unreachable!(),
        }
        assert!(pending.is_empty(), "dequeue must drain the paired ToolUse");
    }

    #[test]
    fn chat_tool_result_is_error_emits_failed() {
        let mut pending = VecDeque::new();
        pending.push_back(("Bash".to_string(), "{}".to_string()));
        let out = chat_event_to_agent_events(
            &ChatStreamEvent::ToolResult { content: "boom".into(), is_error: true },
            &mut pending,
        );
        assert_tool(&out[0], "Bash", ToolCallStatus::Failed);
    }

    #[test]
    fn chat_tool_result_orphan_demotes_to_token() {
        // Orphan (no pending ToolUse): content must surface as text, never
        // vanish, and must NOT fabricate a Started ToolCall (would desync
        // downstream use/result counts).
        let mut pending = VecDeque::new();
        let out = chat_event_to_agent_events(
            &ChatStreamEvent::ToolResult { content: "orphan".into(), is_error: false },
            &mut pending,
        );
        assert_eq!(out.len(), 1);
        match &out[0] {
            AgentEvent::Token(s) => assert_eq!(s, "orphan"),
            other => panic!("orphan must demote to Token, got {:?}", other),
        }
        assert!(pending.is_empty());
    }

    #[test]
    fn chat_result_event_emits_nothing_and_leaves_pending() {
        let mut pending = VecDeque::new();
        pending.push_back(("Read".to_string(), "{}".to_string()));
        let out = chat_event_to_agent_events(
            &ChatStreamEvent::Result { is_error: false, secs: 5 },
            &mut pending,
        );
        assert!(out.is_empty(), "Result must not emit — Done is owned by agent:completed");
        assert_eq!(pending.len(), 1, "Result must leave the pending queue untouched");
    }

    #[test]
    fn chat_multiple_tools_pair_fifo() {
        // Claude may batch tool_uses then return their results in id order:
        //   use(A), use(B), result(A), result(B)
        // FIFO dequeue (front) pairs result(A)→A, result(B)→B. A LIFO stack
        // would mis-pair result(A) onto B — this test guards that regression.
        let mut pending = VecDeque::new();
        let a = chat_event_to_agent_events(
            &ChatStreamEvent::ToolUse { name: "A".into(), input: serde_json::json!({}) },
            &mut pending,
        );
        let b = chat_event_to_agent_events(
            &ChatStreamEvent::ToolUse { name: "B".into(), input: serde_json::json!({}) },
            &mut pending,
        );
        let r1 = chat_event_to_agent_events(
            &ChatStreamEvent::ToolResult { content: "ra".into(), is_error: false },
            &mut pending,
        );
        let r2 = chat_event_to_agent_events(
            &ChatStreamEvent::ToolResult { content: "rb".into(), is_error: false },
            &mut pending,
        );
        assert_tool(&a[0], "A", ToolCallStatus::Started);
        assert_tool(&b[0], "B", ToolCallStatus::Started);
        // FIFO: first result dequeues A (front), second dequeues B.
        assert_tool(&r1[0], "A", ToolCallStatus::Succeeded);
        assert_tool(&r2[0], "B", ToolCallStatus::Succeeded);
        assert!(pending.is_empty());
    }

    #[test]
    fn chat_roundtrip_text_preserves_token() {
        // AgentEvent::Token → map_agent_event → Text → chat_event_to_agent_events
        // → Token. Round-trip on the Text/Token axis (the axis claude's wire
        // actually exercises) proves the two maps are consistent inverses.
        let forward = map_agent_event(AgentEvent::Token("x".to_string()), 0);
        assert_eq!(forward.len(), 1);
        let mut pending = VecDeque::new();
        let back = chat_event_to_agent_events(&forward[0], &mut pending);
        assert_eq!(back.len(), 1);
        match &back[0] {
            AgentEvent::Token(s) => assert_eq!(s, "x"),
            other => panic!("roundtrip lost Token, got {:?}", other),
        }
        assert!(pending.is_empty());
    }

    // ---- turns_to_history ----

    use crate::models::{AgentType, ContextSnapshot, Session};
    use serde_json::json;

    /// Minimal completed session with the fields turns_to_history reads.
    fn turn(id: &str, prompt: &str, blocks: Option<Value>, summary: Option<&str>) -> Session {
        Session {
            id: id.to_string(),
            project_path: "/p".to_string(),
            agent_type: AgentType::ClaudeCode,
            status: SessionStatus::Completed,
            prompt: prompt.to_string(),
            model: None,
            started_at: id.to_string(), // lexical ASC == chronological for tests
            finished_at: None,
            exit_code: Some(0),
            output_summary: summary.map(|s| s.to_string()),
            context_snapshot: None as Option<ContextSnapshot>,
            linked_requirement_id: None,
            parent_session_id: None,
            conversation_id: None,
            blocks,
            task_ref: None,
        }
    }

    fn blocks_json(events: &[ChatStreamEvent]) -> Value {
        serde_json::to_value(events).unwrap()
    }

    #[test]
    fn empty_turns_yields_empty_history() {
        let out = turns_to_history(&[], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        assert!(out.is_empty());
    }

    #[test]
    fn single_turn_with_blocks_strips_tool_calls_keeps_assistant_text() {
        let t = turn(
            "t0",
            "read the file",
            Some(blocks_json(&[
                ChatStreamEvent::Text { content: "reading now".into() },
                ChatStreamEvent::ToolUse { name: "Read".into(), input: json!({"file_path":"/x"}) },
                ChatStreamEvent::ToolResult { content: "file contents".into(), is_error: false },
                ChatStreamEvent::Text { content: "done".into() },
            ])),
            None,
        );
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        // user + assistant ONLY — ToolUse/ToolResult are stripped so the next run
        // neither bloats context nor copies the prior tool call by example.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, Role::User);
        assert_eq!(out[0].content, "read the file");
        assert_eq!(out[1].role, Role::Assistant);
        // No tool_calls, no tool messages survive the strip.
        assert!(out[1].tool_calls.is_empty());
        assert!(out.iter().all(|m| m.role != Role::Tool));
        // assistant text = merged text chunks (both kept; the tool block between
        // them is dropped without splitting the surrounding text).
        assert!(out[1].content.contains("reading now") && out[1].content.contains("done"));
    }

    #[test]
    fn blocks_none_falls_back_to_output_summary() {
        let t = turn("t0", "ask", None, Some("the answer is 42"));
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, Role::User);
        assert_eq!(out[1].role, Role::Assistant);
        assert_eq!(out[1].content, "the answer is 42");
    }

    #[test]
    fn blocks_none_and_summary_none_keeps_only_user_message() {
        // No fabricated empty assistant turn — some providers reject them.
        let t = turn("t0", "ask", None, None);
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, Role::User);
    }

    #[test]
    fn two_turns_stay_chronological_oldest_first() {
        let t0 = turn("a", "first prompt", Some(blocks_json(&[
            ChatStreamEvent::Text { content: "first reply".into() },
        ])), None);
        let t1 = turn("b", "second prompt", Some(blocks_json(&[
            ChatStreamEvent::Text { content: "second reply".into() },
        ])), None);
        let out = turns_to_history(&[t0, t1], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        // [user(a), assistant(first), user(b), assistant(second)]
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].content, "first prompt");
        assert_eq!(out[1].content, "first reply");
        assert_eq!(out[2].content, "second prompt");
        assert_eq!(out[3].content, "second reply");
    }

    #[test]
    fn multiple_tool_uses_and_results_all_stripped() {
        // With tool calls stripped, several ToolUse + ToolResult in one turn must
        // collapse to a single assistant text message — no tool_calls, no tool
        // messages, regardless of how many tools the prior turn ran.
        let t = turn("t0", "do two things", Some(blocks_json(&[
            ChatStreamEvent::Text { content: "ok".into() },
            ChatStreamEvent::ToolUse { name: "A".into(), input: json!({}) },
            ChatStreamEvent::ToolUse { name: "B".into(), input: json!({}) },
            ChatStreamEvent::ToolResult { content: "resA".into(), is_error: false },
            ChatStreamEvent::ToolResult { content: "resB".into(), is_error: false },
        ])), None);
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        assert_eq!(out.len(), 2); // user + assistant only
        assert_eq!(out[1].role, Role::Assistant);
        assert!(out[1].tool_calls.is_empty());
        assert!(out.iter().all(|m| m.role != Role::Tool));
        assert_eq!(out[1].content, "ok");
    }

    #[test]
    fn total_text_cap_drops_oldest_whole_turn_keeps_newest() {
        // Tiny total cap so only the newest turn fits; the older one must be
        // dropped as a whole (user+assistant together), never split.
        let t0 = turn("a", "old prompt that is fairly long", Some(blocks_json(&[
            ChatStreamEvent::Text { content: "old reply also fairly long".into() },
        ])), None);
        let t1 = turn("b", "new prompt that is fairly long", Some(blocks_json(&[
            ChatStreamEvent::Text { content: "new reply also fairly long".into() },
        ])), None);
        let out = turns_to_history(&[t0, t1], REACT_HISTORY_TURN_TEXT_CAP, 30);
        // Only the newest turn survives, intact.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].content, "new prompt that is fairly long");
        assert_eq!(out[1].role, Role::Assistant);
    }

    #[test]
    fn message_count_cap_drops_oldest_whole_turns() {
        // Force the message-count cap by making many turns; assert we never
        // exceed REACT_HISTORY_TOTAL_MESSAGES and never split a turn.
        let turns: Vec<Session> = (0..30)
            .map(|i| turn(&format!("t{:02}", i), &format!("p{}", i),
                Some(blocks_json(&[
                    ChatStreamEvent::Text { content: format!("r{}", i) },
                    ChatStreamEvent::ToolUse { name: "X".into(), input: json!({}) },
                    ChatStreamEvent::ToolResult { content: "z".into(), is_error: false },
                ])), None))
            .collect();
        // 30 turns × 2 messages (tool calls stripped: user + assistant) = 60 > cap
        // of 40. The kept prefix is newest.
        let out = turns_to_history(&turns, REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        assert!(out.len() <= REACT_HISTORY_TOTAL_MESSAGES);
        // Newest turn's prompt must be present; oldest must be gone.
        let prompts: Vec<&str> = out.iter().filter(|m| m.role == Role::User).map(|m| m.content.as_str()).collect();
        assert!(prompts.contains(&"p29"));
        assert!(!prompts.contains(&"p0"));
    }

    #[test]
    fn turn_with_only_tool_calls_produces_no_assistant_message() {
        // A turn whose blocks are entirely tool calls (no Text/Thinking) carries
        // nothing after the strip → blocks_to_assistant_message returns None, so
        // the turn keeps ONLY its user message (no fabricated empty assistant,
        // which some providers reject).
        let t = turn("t0", "ask", Some(blocks_json(&[
            ChatStreamEvent::ToolUse { name: "Read".into(), input: json!({}) },
            ChatStreamEvent::ToolResult { content: "file contents".into(), is_error: false },
        ])), None);
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        assert_eq!(out.len(), 1); // user only — no assistant message survives
        assert_eq!(out[0].role, Role::User);
    }

    #[test]
    fn long_assistant_text_is_truncated_to_turn_cap() {
        // The tail() cap still bounds the kept assistant text — a long reply
        // doesn't blow past turn_text_cap even after tool calls are stripped.
        // (Replaces the old tool-result truncation test: tool messages no longer
        // exist, so the truncation guarantee now applies to the assistant text.)
        let long = "x".repeat(REACT_HISTORY_TURN_TEXT_CAP * 2);
        let t = turn("t0", "ask", Some(blocks_json(&[
            ChatStreamEvent::Text { content: long.clone() },
        ])), None);
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        let assistant = out.iter().find(|m| m.role == Role::Assistant).unwrap();
        // tail() prepends "..." (3 chars) + the trailing turn_text_cap chars.
        assert!(assistant.content.len() <= REACT_HISTORY_TURN_TEXT_CAP + 4);
        assert!(assistant.content.starts_with("..."));
    }

    #[test]
    fn malformed_blocks_json_does_not_panic() {
        // A blocks column that isn't a valid ChatStreamEvent array must degrade
        // gracefully — fall back to output_summary, never panic the run.
        let t = turn("t0", "ask", Some(json!({"not": "an array"})), Some("fallback summary"));
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        // from_value::<Vec<_>> fails on a non-array → summary fallback path.
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].role, Role::Assistant);
        assert_eq!(out[1].content, "fallback summary");
    }

    #[test]
    fn running_turns_are_skipped() {
        // A turn still running has no finalized reply; it must not appear as a
        // lone user message in the resumed history.
        let mut running = turn("r", "in-flight prompt", None, None);
        running.status = SessionStatus::Running;
        let done = turn("d", "settled prompt", Some(blocks_json(&[
            ChatStreamEvent::Text { content: "settled reply".into() },
        ])), None);
        let out = turns_to_history(&[running, done], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        // Only the settled turn's 2 messages appear.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].content, "settled prompt");
    }
}
