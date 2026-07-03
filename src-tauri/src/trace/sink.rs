//! LLM call trace sink — the seam between the ChatModel (which observes each
//! HTTP request/response to an Anthropic-compatible endpoint) and the
//! `llm_traces` table. The ChatModel holds an `Option<Arc<dyn TraceSink>>`; when
//! present, every LLM call records its request body, HTTP status, response
//! body (on error), latency, and token usage. `DbTraceSink` writes to SQLite
//! on a blocking thread (fire-and-forget — a trace-write failure must never
//! break the agent loop). Mirrors `crate::cost::sink` exactly.
//!
//! WHY this exists: before tracing, a non-2xx response was compressed to
//! `format!("LLM stream failed: {status}")` and the error body (the actual
//! reason — quota, schema, model-not-found) was discarded along with the
//! request body. Sessions like 41f2ddca failed in 0.8s with no recoverable
//! clue. This sink keeps the real request/response on disk so the cause is
//! always one query away.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::db::DbState;
use crate::trace::db::{LlmTraceRow, insert_llm_trace};

/// One LLM HTTP call observed at the ChatModel boundary. Built by the call
/// site (stream/generate); `DbTraceSink` maps it to a table row. `session_id`
/// is passed separately to `record_llm_call` (it lives on the ChatModel, not on
/// the per-call trace).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmTrace {
    pub model: String,
    pub base_url: String,
    /// HTTP status code. None when the call never reached HTTP (network error,
    /// circuit open, decode failure before a response).
    pub status_code: Option<u16>,
    /// Coarse classification of why the call ended this way:
    /// `non_2xx` | `network` | `decode` | `circuit`. None on a clean 2xx.
    pub error_kind: Option<String>,
    /// The full build_body JSON (the request), truncated to a size useful for
    /// diagnosis without bloating the table. api_key travels in a header,
    /// never the body, so this is safe to persist.
    pub req_body: String,
    /// The raw wire response body, truncated. On a clean 2xx this is the full
    /// response (JSON for generate(), the SSE stream for stream()) so the
    /// request↔response pair is one query away — symmetric with the error path,
    /// which stores the error JSON. None only when the call never produced a
    /// body (network error / circuit open / decode failure before any response).
    /// See the 2026-06-19 trace observability research.
    pub resp_body: Option<String>,
    pub latency_ms: Option<u64>,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    /// B3: request-send → first response signal (time-to-first-byte), in ms.
    /// None when the call never reached a first byte (pure network failure).
    /// Drives the "model slow to start" diagnosis distinct from slow output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    /// B3: first-byte → completion (output/stream duration), in ms. None when
    /// there was no streaming phase (e.g. headers-only non_2xx) or pre-B3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_ms: Option<u64>,
    /// A1 (OTel span tree): the span this call belongs to. One span per agent
    /// instance — every LLM call a model makes shares its `span_id`, so
    /// TraceView groups calls by the agent that issued them and renders the
    /// agent-DAG nesting (main → subagent). None for ad-hoc/test agents that
    /// carry no span context (honest absence, not a faked root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
    /// A1: the orchestrating agent's `span_id` (the span that spawned this
    /// one). None for the root agent (top of the tree) or ad-hoc/test agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    /// A1: human label for the span ("agent" | "subagent" | …) so the tree
    /// renders a name per node instead of a bare id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_name: Option<String>,
}

/// Receives one trace record per LLM call. Implementations must be cheap to
/// share (held inside the ChatModel behind an `Arc`) and must NOT propagate
/// errors into the caller — a failed trace write is logged, not fatal.
pub trait TraceSink: Send + Sync {
    fn record_llm_call(&self, session_id: Option<&str>, trace: LlmTrace);
}

/// A TraceSink that drops everything — the default when no DB context is
/// available (an ad-hoc agent without a session id, or tests).
#[derive(Debug, Default, Clone, Copy)]
pub struct NullTraceSink;

impl TraceSink for NullTraceSink {
    fn record_llm_call(&self, _: Option<&str>, _: LlmTrace) {}
}

/// A TraceSink writing to the `llm_traces` table. `record_llm_call` spawns a
/// blocking task so the synchronous rusqlite INSERT never stalls the async
/// stream, and swallows errors (logged at warn) so tracing can't crash the
/// agent. Holds `conversation_id` (known at agent build time); `session_id`
/// arrives per-call from the ChatModel.
pub struct DbTraceSink {
    db: DbState,
    conversation_id: Option<String>,
}

impl DbTraceSink {
    pub fn new(db: DbState, conversation_id: Option<String>) -> Self {
        Self {
            db,
            conversation_id,
        }
    }
}

impl TraceSink for DbTraceSink {
    fn record_llm_call(&self, session_id: Option<&str>, trace: LlmTrace) {
        let row = LlmTraceRow {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.map(|s| s.to_string()),
            conversation_id: self.conversation_id.clone(),
            model: trace.model,
            base_url: trace.base_url,
            status_code: trace.status_code.map(|c| c as i64),
            error_kind: trace.error_kind,
            req_body: trace.req_body,
            resp_body: trace.resp_body,
            latency_ms: trace.latency_ms.map(|l| l as i64),
            input_tokens: trace.input_tokens.map(|t| t as i64),
            output_tokens: trace.output_tokens.map(|t| t as i64),
            ttfb_ms: trace.ttfb_ms.map(|t| t as i64),
            stream_ms: trace.stream_ms.map(|t| t as i64),
            span_id: trace.span_id,
            parent_span_id: trace.parent_span_id,
            span_name: trace.span_name,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        let db = self.db.clone();
        // Fire-and-forget blocking write. DbTraceSink is only ever constructed
        // on the agent path (inside a tokio runtime), so a runtime is present.
        tokio::task::spawn_blocking(move || match db.get() {
            Ok(conn) => {
                if let Err(e) = insert_llm_trace(&conn, &row) {
                    log::warn!("[trace] insert_llm_trace failed: {e}");
                }
            }
            Err(e) => log::warn!("[trace] db lock failed: {e}"),
        });
    }
}

/// Build a shared sink, or a `NullTraceSink` when `db` is absent. Convenience
/// for the agent construction path (build_react_agent).
pub fn optional_shared(db: Option<DbState>, conversation_id: Option<String>) -> Arc<dyn TraceSink> {
    match db {
        Some(db) => Arc::new(DbTraceSink::new(db, conversation_id)),
        None => Arc::new(NullTraceSink),
    }
}

/// Truncate a string to `max` bytes on a UTF-8 boundary, with a `...(N more)`
/// suffix. Used to cap request/response bodies before persisting so a huge
/// tool result or prompt can't blow up the table.
pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...({} more)", &s[..end], s.len() - end)
}

/// Redact secret-shaped values from a string before it is persisted to a trace
/// row or written to the operator log. Some gateways echo request credentials
/// back inside a 4xx error body (e.g. `x-api-key: sk-...`, `Authorization:
/// Bearer ...`), so an un-redacted `resp_body` would land the caller's API key
/// in the `llm_traces` table.
///
/// Three shapes, case-insensitive:
/// 1. Bare live keys `sk-…` / `sk_…` (OpenAI/Anthropic/GLM-style, ≥12 chars).
/// 2. `Bearer <token>` — the credential after the scheme name.
/// 3. `key: value` / `"key": "value"` / `key=value` for secret-bearing names
///    (`api_key`, `x-api-key`, `authorization`, `password`, …).
///
/// The key list deliberately excludes bare `token`/`tokens`/`max_tokens` —
/// those appear in every LLM usage JSON and carry counts, not credentials, so
/// matching them would mangle legitimate trace content. Run order is SK →
/// BEARER → KV so a `Bearer sk-…` value is consumed by the SK/BEARER passes
/// before the KV pass can leak the tail after `Bearer`.
pub fn redact_secrets(s: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static SK: OnceLock<Regex> = OnceLock::new();
    static BEARER: OnceLock<Regex> = OnceLock::new();
    static KV: OnceLock<Regex> = OnceLock::new();
    let sk = SK.get_or_init(|| Regex::new(r"sk[_-][A-Za-z0-9_\-]{12,}").expect("static regex"));
    let bearer = BEARER
        .get_or_init(|| Regex::new(r"(?i)\bbearer\s+[A-Za-z0-9_\-\.]+").expect("static regex"));
    let kv = KV.get_or_init(|| {
        Regex::new(
            r#"(?i)(api[_-]?key|x-api-key|api[_-]?secret|secret[_-]?key|access[_-]?token|auth[_-]?token|authorization|password|passwd|apikey)"?\s*[:=]\s*"?[^"\s,}\]]+"#,
        )
        .expect("static regex")
    });
    let out = sk.replace_all(s, "[REDACTED]");
    let out = bearer.replace_all(&out, "bearer [REDACTED]");
    kv.replace_all(&out, r#"$1: "[REDACTED]""#).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_short_input_unchanged() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("", 10), "");
        // Exactly at the cap → unchanged (no suffix).
        assert_eq!(truncate("abcd", 4), "abcd");
    }

    #[test]
    fn truncate_caps_long_input_with_more_suffix() {
        let s = "x".repeat(100);
        let t = truncate(&s, 10);
        assert!(t.starts_with("xxxxxxxxxx"), "prefix preserved: {t}");
        assert!(t.contains("more"), "suffix marks truncation: {t}");
    }

    #[test]
    fn truncate_never_splits_a_multibyte_char() {
        // 你好 = 6 bytes (3 per char). A cap at 7 bytes lands mid-character;
        // truncate must back off to a char boundary (6) so the result stays
        // valid UTF-8 and is a real prefix of the input.
        let s = "你好你好";
        let t = truncate(s, 7);
        let prefix = t.split("...(").next().unwrap();
        assert!(
            s.starts_with(prefix),
            "truncated prefix must sit on a char boundary: {prefix:?}"
        );
    }

    #[test]
    fn redact_secrets_strips_bearer_and_sk_keys() {
        let s = "Authorization: Bearer sk-abcdef1234567890abcdefXYZ";
        let r = redact_secrets(s);
        assert!(
            !r.contains("sk-abcdef1234567890abcdefXYZ"),
            "live key leaked: {r}"
        );
        assert!(!r.contains("Bearer sk"), "bearer+token not consumed: {r}");
    }

    #[test]
    fn redact_secrets_strips_api_key_json_and_header_forms() {
        let json = r#"{"api_key": "sk-livekey1234567890SECRET", "model": "glm"}"#;
        let r = redact_secrets(json);
        assert!(!r.contains("sk-livekey"), "json api_key leaked: {r}");
        assert!(r.contains("[REDACTED]"), "must mark redaction: {r}");

        let header = "x-api-key: sk-9876543210abcdefghij";
        let r = redact_secrets(header);
        assert!(!r.contains("sk-9876543210"), "header key leaked: {r}");
    }

    #[test]
    fn redact_secrets_leaves_token_counts_intact() {
        // LLM usage JSON carries token COUNTS — these are not credentials and
        // must not be mangled by the bare-token exclusion.
        let s = r#"{"input_tokens": 1234, "output_tokens": 567, "max_tokens": 4096}"#;
        let r = redact_secrets(s);
        assert!(r.contains("1234"), "input_tokens count mangled: {r}");
        assert!(r.contains("4096"), "max_tokens mangled: {r}");
        assert!(!r.contains("[REDACTED]"), "counts falsely redacted: {r}");
    }

    #[test]
    fn redact_secrets_leaves_plain_text_intact() {
        let s = "the model returned an error about invalid request body";
        assert_eq!(redact_secrets(s), s);
    }

    /// DbTraceSink::record_llm_call is fire-and-forget via spawn_blocking. This
    /// proves the full write path end-to-end: sink → blocking INSERT → a readable
    /// row carrying the per-call session_id + sink-scoped conversation_id — the
    /// exact fields TraceView queries to explain a failed turn. This is the
    /// contract the whole feature exists to deliver.
    #[tokio::test]
    async fn db_trace_sink_persists_call_with_session_and_conversation() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db = crate::db::DbState::open(tmp.path()).unwrap();
        let sink = DbTraceSink::new(db.clone(), Some("conv-1".to_string()));
        sink.record_llm_call(
            Some("sess-1"),
            LlmTrace {
                model: "glm-4.6".to_string(),
                base_url: "https://api.example".to_string(),
                status_code: Some(400),
                error_kind: Some("non_2xx".to_string()),
                req_body: r#"{"model":"glm-4.6"}"#.to_string(),
                resp_body: Some("invalid_request_error".to_string()),
                latency_ms: Some(8),
                input_tokens: None,
                output_tokens: None,
                ttfb_ms: None,
                stream_ms: None,
                span_id: None,
                parent_span_id: None,
                span_name: None,
            },
        );
        // spawn_blocking is fire-and-forget; poll the table until the row lands
        // (or time out) instead of a fixed sleep — robust on a loaded CI box.
        let rows = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let conn = db.get().unwrap();
                let rows = crate::trace::db::list_traces_for_session(&conn, "sess-1").unwrap();
                if !rows.is_empty() {
                    return rows;
                }
                drop(conn);
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("trace row never landed in llm_traces");
        assert_eq!(rows.len(), 1, "exactly one trace row written");
        let r = &rows[0];
        assert_eq!(r.session_id.as_deref(), Some("sess-1"));
        assert_eq!(r.conversation_id.as_deref(), Some("conv-1"));
        assert_eq!(r.status_code, Some(400));
        assert_eq!(r.error_kind.as_deref(), Some("non_2xx"));
        assert_eq!(r.resp_body.as_deref(), Some("invalid_request_error"));
    }

    #[test]
    fn null_trace_sink_is_a_silent_no_op() {
        // The ad-hoc / test path: no DB, no panic, nothing recorded.
        NullTraceSink.record_llm_call(
            Some("x"),
            LlmTrace {
                model: "m".to_string(),
                base_url: "u".to_string(),
                status_code: None,
                error_kind: None,
                req_body: "{}".to_string(),
                resp_body: None,
                latency_ms: None,
                input_tokens: None,
                output_tokens: None,
                ttfb_ms: None,
                stream_ms: None,
                span_id: None,
                parent_span_id: None,
                span_name: None,
            },
        );
    }
}
