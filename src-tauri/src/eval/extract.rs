//! B7 trajectory extraction — reconstruct the ordered tool-call sequence from a
//! session's persisted LLM traces. Each `LlmTraceRow` carries the raw response
//! wire body; the assistant's `tool_use` blocks (Anthropic shape:
//! `content[].type == "tool_use"`) or `tool_calls` (OpenAI shape:
//! `choices[].message.tool_calls[].function.name`) name the tools the agent
//! decided to invoke. Traces are ASC by `created_at`, so walking them in order
//! yields the trajectory.
//!
//! Best-effort and defensive: malformed or truncated bodies (the trace sink
//! truncates long payloads on a UTF-8 boundary) are skipped, never panic.

use serde::{Deserialize, Serialize};

use crate::trace::db::LlmTraceRow;

/// One step in a reconstructed trajectory: the tool called, and a coarse
/// success/error status derived from the surrounding trace's HTTP outcome.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolStep {
    pub name: String,
    /// `"error"` when the trace carrying this call was non-2xx or had an
    /// `error_kind`; otherwise `None` (treated as success). Coarse by design —
    /// the trace row doesn't carry per-tool-call status.
    pub status: Option<String>,
}

/// Reconstruct the tool-call trajectory from a session's traces (oldest-first,
/// as `list_traces_for_session` returns them). Malformed bodies are skipped; a
/// single trace may contribute zero, one, or many steps (parallel tool calls).
/// Handles both Anthropic (`content[].tool_use`) and OpenAI
/// (`choices[].message.tool_calls`) response shapes.
pub fn extract_trajectory(traces: &[LlmTraceRow]) -> Vec<ToolStep> {
    let mut out = Vec::new();
    for t in traces {
        let failed = t
            .status_code
            .map(|c| !(200..300).contains(&c))
            .unwrap_or(false)
            || t.error_kind.is_some();
        let status = if failed {
            Some("error".to_string())
        } else {
            None
        };
        if let Some(names) = tool_call_names(t.resp_body.as_deref()) {
            for name in names {
                out.push(ToolStep {
                    name,
                    status: status.clone(),
                });
            }
        }
    }
    out
}

impl ToolStep {
    /// The ordered tool-name slice a scoring call needs. Borrows the owned
    /// `String`s for the `&[&str]` scoring API without reallocating.
    pub fn name_refs(steps: &[ToolStep]) -> Vec<&str> {
        steps.iter().map(|s| s.name.as_str()).collect()
    }
}

/// Pull tool-call names out of a response body. Tries the Anthropic shape
/// first, then the OpenAI shape. Returns `None` on any parse failure or when
/// the body carries no tool calls (a plain text response) — the caller skips.
fn tool_call_names(resp_body: Option<&str>) -> Option<Vec<String>> {
    let body = resp_body?;
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    // Anthropic: {"content":[{"type":"tool_use","name":"..."}, ...]}
    if let Some(content) = v.get("content").and_then(|c| c.as_array()) {
        let names: Vec<String> = content
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
            .filter_map(|b| b.get("name").and_then(|n| n.as_str()).map(String::from))
            .collect();
        if !names.is_empty() {
            return Some(names);
        }
    }
    // OpenAI: {"choices":[{"message":{"tool_calls":[{"function":{"name":...}}]}}]}
    if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
        let names: Vec<String> = choices
            .iter()
            .filter_map(|c| c.get("message")?.get("tool_calls")?.as_array())
            .flatten()
            .filter_map(|tc| tc.get("function")?.get("name")?.as_str().map(String::from))
            .collect();
        if !names.is_empty() {
            return Some(names);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(id: &str, resp: Option<&str>, status: Option<i64>, err: Option<&str>) -> LlmTraceRow {
        LlmTraceRow {
            id: id.into(),
            session_id: Some("s1".into()),
            conversation_id: None,
            model: "glm-4.6".into(),
            base_url: "https://x".into(),
            status_code: status,
            error_kind: err.map(String::from),
            req_body: "{}".into(),
            resp_body: resp.map(String::from),
            latency_ms: None,
            input_tokens: None,
            output_tokens: None,
            created_at: "2026-06-19T00:00:00Z".into(),
        }
    }

    #[test]
    fn extracts_anthropic_tool_use_names_in_order() {
        let traces = vec![
            trace(
                "a",
                Some(r#"{"content":[{"type":"text","text":"hi"},{"type":"tool_use","name":"read","id":"1"}]}"#),
                Some(200),
                None,
            ),
            trace(
                "b",
                Some(r#"{"content":[{"type":"tool_use","name":"grep","id":"2"}]}"#),
                Some(200),
                None,
            ),
        ];
        let steps = extract_trajectory(&traces);
        assert_eq!(
            steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["read", "grep"]
        );
        assert!(steps.iter().all(|s| s.status.is_none()));
    }

    #[test]
    fn extracts_openai_tool_calls_shape() {
        let body = r#"{"choices":[{"message":{"tool_calls":[{"function":{"name":"edit"}}]}}]}"#;
        let steps = extract_trajectory(&[trace("a", Some(body), Some(200), None)]);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].name, "edit");
    }

    #[test]
    fn parallel_tool_calls_become_multiple_steps() {
        // One response carrying two parallel tool_use blocks → two steps.
        let body = r#"{"content":[{"type":"tool_use","name":"read","id":"1"},{"type":"tool_use","name":"grep","id":"2"}]}"#;
        let steps = extract_trajectory(&[trace("a", Some(body), Some(200), None)]);
        assert_eq!(
            steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["read", "grep"]
        );
    }

    #[test]
    fn failed_trace_marks_steps_error() {
        let body = r#"{"content":[{"type":"tool_use","name":"bash","id":"1"}]}"#;
        let steps = extract_trajectory(&[trace("a", Some(body), Some(500), Some("non_2xx"))]);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].status.as_deref(), Some("error"));
    }

    #[test]
    fn malformed_body_skipped_not_panicked() {
        let steps = extract_trajectory(&[
            trace("bad", Some("not json{"), Some(200), None),
            trace(
                "good",
                Some(r#"{"content":[{"type":"tool_use","name":"read","id":"1"}]}"#),
                Some(200),
                None,
            ),
        ]);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].name, "read");
    }

    #[test]
    fn empty_traces_yield_empty_trajectory() {
        assert!(extract_trajectory(&[]).is_empty());
    }

    #[test]
    fn name_refs_views_for_scoring() {
        let steps = extract_trajectory(&[trace(
            "a",
            Some(r#"{"content":[{"type":"tool_use","name":"read","id":"1"}]}"#),
            Some(200),
            None,
        )]);
        assert_eq!(ToolStep::name_refs(&steps), vec!["read"]);
    }
}
