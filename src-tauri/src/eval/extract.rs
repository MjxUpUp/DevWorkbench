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

// ─────────────────────────────────────────────────────────────────────────────
// Rich trajectory (P3) + span tree (A1).
//
// `extract_trajectory` above returns just tool names; P3's prototype also shows
// files_changed / tokens / cost, and A1 needs a span tree (LLM call = parent,
// tool calls = children) for paired alignment. Both derive from the SAME trace
// rows + the session's recorded file diff — no new I/O, no LLM.
// ─────────────────────────────────────────────────────────────────────────────

/// A node in the OTel-style span tree. `kind="llm"` is a parent LLM call span
/// (one per trace); `kind="tool"` is a child tool-call span hanging off it.
/// `latency_ms`/`status` come straight off the trace row (A1's alignment base).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Span {
    pub kind: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub children: Vec<Span>,
}

/// A session's span forest — one root span per LLM trace, tool calls nested.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct SpanTree {
    pub roots: Vec<Span>,
}

/// A richly-extracted trajectory: tool steps + files changed + token usage +
/// estimated cost + the span tree. `cost_cents` is a rough estimate at a fixed
/// nominal rate (tokens are the real signal; per-provider pricing lives in the
/// cost module and isn't always available at preview time).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FullTrajectory {
    pub steps: Vec<ToolStep>,
    pub files_changed: Vec<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Rough cost estimate at ESTIMATE_RATE_PER_M_TOKENS (USD¢). Labeled 估算.
    pub cost_cents: f64,
    pub span_tree: SpanTree,
}

/// Nominal per-million-token rate (USD) used for the preview cost estimate.
/// Real per-provider pricing is applied when the run is scored/persisted; this
/// is only for the P3 preview "≈ $0.02" hint.
pub const ESTIMATE_RATE_PER_M_TOKENS: f64 = 0.60;

/// Extract the full rich trajectory from a session's traces + its recorded file
/// diff. Steps come from [`extract_trajectory`]; tokens sum the trace rows;
/// the span tree nests each trace's tool calls under an LLM-parent span.
pub fn extract_full(traces: &[LlmTraceRow], files_changed: &[String]) -> FullTrajectory {
    let steps = extract_trajectory(traces);
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut roots = Vec::with_capacity(traces.len());
    for t in traces {
        if let Some(i) = t.input_tokens {
            input_tokens = input_tokens.saturating_add(i.max(0) as u64);
        }
        if let Some(o) = t.output_tokens {
            output_tokens = output_tokens.saturating_add(o.max(0) as u64);
        }
        roots.push(trace_to_span(t));
    }
    let total_tokens = input_tokens + output_tokens;
    let cost_cents = (total_tokens as f64 / 1_000_000.0) * ESTIMATE_RATE_PER_M_TOKENS * 100.0;
    FullTrajectory {
        steps,
        files_changed: files_changed.to_vec(),
        input_tokens,
        output_tokens,
        cost_cents,
        span_tree: SpanTree { roots },
    }
}

/// Build the span tree root for one trace: an LLM-call parent whose children are
/// the tool calls in its response body. Latency + status come from the row.
fn trace_to_span(t: &LlmTraceRow) -> Span {
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
    let mut children = Vec::new();
    if let Some(names) = tool_call_names(t.resp_body.as_deref()) {
        for name in names {
            children.push(Span {
                kind: "tool".into(),
                name,
                latency_ms: None,
                status: status.clone(),
                children: Vec::new(),
            });
        }
    }
    Span {
        kind: "llm".into(),
        name: t.model.clone(),
        latency_ms: t.latency_ms.and_then(|l| u64::try_from(l.max(0)).ok()),
        status: status.clone(),
        children,
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
            ttfb_ms: None,
            stream_ms: None,
            span_id: None,
            parent_span_id: None,
            span_name: None,
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

    fn trace_tok(id: &str, resp: Option<&str>, status: i64, lat: i64, it: i64, ot: i64) -> LlmTraceRow {
        let mut r = trace(id, resp, Some(status), None);
        r.latency_ms = Some(lat);
        r.input_tokens = Some(it);
        r.output_tokens = Some(ot);
        r
    }

    #[test]
    fn extract_full_sums_tokens_and_estimates_cost() {
        let traces = vec![
            trace_tok(
                "a",
                Some(r#"{"content":[{"type":"tool_use","name":"read","id":"1"}]}"#),
                200,
                120,
                1000,
                200,
            ),
            trace_tok("b", None, 200, 80, 500, 100),
        ];
        let full = extract_full(&traces, &["BlocksView.tsx".into()]);
        assert_eq!(full.input_tokens, 1500);
        assert_eq!(full.output_tokens, 300);
        // cost = (1800 / 1e6) * 0.60 * 100  ≈ 0.108 ¢
        assert!((full.cost_cents - 0.108).abs() < 1e-6, "got {}", full.cost_cents);
        assert_eq!(full.files_changed, vec!["BlocksView.tsx"]);
    }

    #[test]
    fn extract_full_builds_span_tree_with_tool_children() {
        let traces = vec![trace_tok(
            "a",
            Some(r#"{"content":[{"type":"tool_use","name":"read","id":"1"},{"type":"tool_use","name":"edit","id":"2"}]}"#),
            200,
            420,
            10,
            20,
        )];
        let full = extract_full(&traces, &[]);
        assert_eq!(full.span_tree.roots.len(), 1);
        let root = &full.span_tree.roots[0];
        assert_eq!(root.kind, "llm");
        assert_eq!(root.name, "glm-4.6");
        assert_eq!(root.latency_ms, Some(420));
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].kind, "tool");
        assert_eq!(root.children[0].name, "read");
    }

    #[test]
    fn extract_full_marks_failed_trace_spans_error() {
        let traces = vec![trace_tok(
            "a",
            Some(r#"{"content":[{"type":"tool_use","name":"bash","id":"1"}]}"#),
            500,
            2000,
            10,
            20,
        )];
        let full = extract_full(&traces, &[]);
        let root = &full.span_tree.roots[0];
        assert_eq!(root.status.as_deref(), Some("error"));
        assert_eq!(root.children[0].status.as_deref(), Some("error"));
    }
}
