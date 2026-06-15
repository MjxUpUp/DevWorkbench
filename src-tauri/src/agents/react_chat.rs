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
use kernel_core::{AgentEvent, AgentRunStatus, ToolCallStatus};
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
}
