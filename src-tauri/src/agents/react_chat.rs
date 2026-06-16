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
    AgentEvent, AgentRunStatus, FunctionCall, Message, Role, ToolCall, ToolCallStatus,
};
use serde_json::Value;

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
/// - `FileChanged`        → `[]` (no variant yet — Phase E followup)
/// - `TurnBoundary`       → `[]` (same)
/// - `Done(outcome)`      → `[Result{is_error: status != Completed, secs}]`
///
/// NB: the transparent agent's Succeeded/Failed tool events carry NO result
/// content (only the status — see `react_agent::run`). So the ToolResult content
/// is a status placeholder, not real tool output. Backfilling the real result is
/// a Phase E followup (needs `react_agent` to yield it alongside the status).
pub fn map_agent_event(ev: AgentEvent, secs: u64) -> Vec<ChatStreamEvent> {
    match ev {
        AgentEvent::Token(s) => vec![ChatStreamEvent::Text { content: s }],
        AgentEvent::ToolCall(tc) => match tc.status {
            ToolCallStatus::Started => vec![ChatStreamEvent::ToolUse {
                name: tc.tool,
                input: parse_tool_arguments(&tc.arguments),
            }],
            ToolCallStatus::Succeeded => vec![ChatStreamEvent::ToolResult {
                content: "(ok)".to_string(),
                is_error: false,
            }],
            ToolCallStatus::Failed => vec![ChatStreamEvent::ToolResult {
                content: "(failed)".to_string(),
                is_error: true,
            }],
        },
        AgentEvent::FileChanged(_) => Vec::new(),
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
///   user(prompt) + [assistant(text+tool_calls) + tool messages]  (from blocks)
///   user(prompt) + assistant(output_summary)                      (legacy fallback)
///   user(prompt)                                                  (no output at all)
/// Running turns are skipped (no finalized content yet). When the result
/// exceeds the message or total-char caps, the OLDEST whole turns are dropped
/// — turns are never split mid-way, so a tool_call and its tool result always
/// travel together.
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

        let assistant_and_tools = sess
            .blocks
            .as_ref()
            .filter(|v| !v.is_null())
            .and_then(|v| serde_json::from_value::<Vec<ChatStreamEvent>>(v.clone()).ok())
            .map(|blocks| {
                if blocks.is_empty() {
                    Vec::new()
                } else {
                    let turn_idx = groups.len();
                    blocks_to_assistant_and_tool_messages(turn_idx, &blocks, turn_text_cap)
                }
            })
            .unwrap_or_else(|| {
                // No persisted blocks (raw agent, or pre-G1 session) → fall back
                // to the text-only summary so the model at least sees the reply.
                sess.output_summary
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|s| vec![Message::assistant(tail(s, turn_text_cap))])
                    .unwrap_or_default()
            });

        group.extend(assistant_and_tools);
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

/// Turn one prior session's persisted blocks back into the assistant message
/// (text + tool_calls) followed by one tool message per tool result. The
/// ChatStreamEvent wire schema carries no correlation ids, so we synthesize
/// stable ones per turn (`turn{N}_call{M}`) and thread them through both the
/// assistant's `tool_calls[i].id` and the matching tool message's
/// `tool_call_id` — providers (GLM/Anthropic) require them to correlate.
///
/// Tool results are paired positionally with tool uses (zip). If a turn has
/// more results than uses (or vice versa), the orphan tail is dropped rather
/// than fabricated — a malformed history must not feed the model bad pairs.
fn blocks_to_assistant_and_tool_messages(
    turn_idx: usize,
    blocks: &[ChatStreamEvent],
    text_cap: usize,
) -> Vec<Message> {
    let mut text_chunks: Vec<&str> = Vec::new();
    let mut tool_uses: Vec<(&str, &Value)> = Vec::new();
    let mut tool_results: Vec<&str> = Vec::new();
    for ev in blocks {
        match ev {
            ChatStreamEvent::Text { content } => text_chunks.push(content.as_str()),
            ChatStreamEvent::ToolUse { name, input } => tool_uses.push((name.as_str(), input)),
            ChatStreamEvent::ToolResult { content, .. } => tool_results.push(content.as_str()),
            ChatStreamEvent::Result { .. } => {} // terminal marker, not history content
        }
    }

    let assistant_text = tail(&text_chunks.join(""), text_cap);
    let tool_calls: Vec<ToolCall> = tool_uses
        .iter()
        .enumerate()
        .map(|(k, (name, input))| ToolCall {
            id: format!("turn{}_call{}", turn_idx, k),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: (*name).to_string(),
                arguments: serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string()),
            },
        })
        .collect();

    let mut out: Vec<Message> = Vec::with_capacity(1 + tool_results.len());
    out.push(Message {
        role: Role::Assistant,
        content: assistant_text,
        tool_calls: tool_calls.clone(),
        tool_call_id: None,
        reasoning: None,
    });
    // Pair each assistant tool_call with its result positionally. The id on the
    // tool message MUST match the assistant tool_call's id for correlation.
    for (k, result_content) in tool_results.iter().enumerate() {
        let id = format!("turn{}_call{}", turn_idx, k);
        out.push(Message {
            role: Role::Tool,
            content: tail(result_content, text_cap / 2),
            tool_calls: Vec::new(),
            tool_call_id: Some(id),
            reasoning: None,
        });
    }
    out
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
    fn tool_call_started_maps_to_tool_use_with_parsed_input() {
        let ev = AgentEvent::ToolCall(ToolCallEvent {
            tool: "Read".to_string(),
            arguments: r#"{"file_path":"/a.txt"}"#.to_string(),
            status: ToolCallStatus::Started,
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
    fn tool_call_failed_maps_to_error_result() {
        let ev = AgentEvent::ToolCall(ToolCallEvent {
            tool: "Bash".to_string(),
            arguments: "{}".to_string(),
            status: ToolCallStatus::Failed,
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
    fn file_changed_is_dropped() {
        let ev = AgentEvent::FileChanged(PathBuf::from("/x.rs"));
        assert!(map_agent_event(ev, 0).is_empty());
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
    fn single_turn_with_blocks_expands_to_user_assistant_and_tool_messages() {
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
        // user + assistant(+1 tool_call) + 1 tool message
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].role, Role::User);
        assert_eq!(out[0].content, "read the file");
        assert_eq!(out[1].role, Role::Assistant);
        assert_eq!(out[1].tool_calls.len(), 1);
        assert_eq!(out[1].tool_calls[0].function.name, "Read");
        // assistant text = merged text chunks
        assert!(out[1].content.contains("reading now") && out[1].content.contains("done"));
        assert_eq!(out[2].role, Role::Tool);
        // id correlation: tool message's tool_call_id == assistant tool_call's id
        assert_eq!(out[2].tool_call_id.as_deref(), Some(out[1].tool_calls[0].id.as_str()));
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
    fn multiple_tools_pair_positionally_and_correlate_ids() {
        let t = turn("t0", "do two things", Some(blocks_json(&[
            ChatStreamEvent::Text { content: "ok".into() },
            ChatStreamEvent::ToolUse { name: "A".into(), input: json!({}) },
            ChatStreamEvent::ToolUse { name: "B".into(), input: json!({}) },
            ChatStreamEvent::ToolResult { content: "resA".into(), is_error: false },
            ChatStreamEvent::ToolResult { content: "resB".into(), is_error: false },
        ])), None);
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        // user + assistant(2 tool_calls) + toolA + toolB
        assert_eq!(out.len(), 4);
        assert_eq!(out[1].tool_calls.len(), 2);
        assert_eq!(out[1].tool_calls[0].function.name, "A");
        assert_eq!(out[1].tool_calls[1].function.name, "B");
        assert_eq!(out[2].role, Role::Tool);
        assert_eq!(out[2].content, "resA");
        assert_eq!(out[3].content, "resB");
        // Each tool message correlates with its assistant tool_call id, in order.
        assert_eq!(out[2].tool_call_id.as_deref(), Some(out[1].tool_calls[0].id.as_str()));
        assert_eq!(out[3].tool_call_id.as_deref(), Some(out[1].tool_calls[1].id.as_str()));
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
        // 30 turns × 3 messages = 90 > cap of 40. The kept prefix is newest.
        let out = turns_to_history(&turns, REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        assert!(out.len() <= REACT_HISTORY_TOTAL_MESSAGES);
        // Newest turn's prompt must be present; oldest must be gone.
        let prompts: Vec<&str> = out.iter().filter(|m| m.role == Role::User).map(|m| m.content.as_str()).collect();
        assert!(prompts.contains(&"p29"));
        assert!(!prompts.contains(&"p0"));
    }

    #[test]
    fn long_tool_result_is_truncated_to_half_turn_cap() {
        let long = "x".repeat(REACT_HISTORY_TURN_TEXT_CAP * 2);
        let t = turn("t0", "ask", Some(blocks_json(&[
            ChatStreamEvent::ToolUse { name: "Read".into(), input: json!({}) },
            ChatStreamEvent::ToolResult { content: long.clone(), is_error: false },
        ])), None);
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        // tool message content must be ≤ turn_cap/2 + the "..." prefix (4 chars).
        let tool_msg = out.iter().find(|m| m.role == Role::Tool).unwrap();
        assert!(tool_msg.content.len() <= REACT_HISTORY_TURN_TEXT_CAP / 2 + 4);
        assert!(tool_msg.content.starts_with("..."));
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
