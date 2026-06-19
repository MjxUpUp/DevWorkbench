//! LLM call trace sink — the seam between GlmChatModel (which observes each
//! HTTP request/response to an Anthropic-compatible endpoint) and the
//! `llm_traces` table. GlmChatModel holds an `Option<Arc<dyn TraceSink>>`; when
//! present, every LLM call records its request body, HTTP status, response
//! body (on error), latency, and token usage. `DbTraceSink` writes to SQLite
//! on a blocking thread (fire-and-forget — a trace-write failure must never
//! break the agent loop). Mirrors `crate::cost::sink` exactly.
//!
//! WHY this exists: before tracing, a non-2xx response was compressed to
//! `format!("GLM stream failed: {status}")` and the error body (the actual
//! reason — quota, schema, model-not-found) was discarded along with the
//! request body. Sessions like 41f2ddca failed in 0.8s with no recoverable
//! clue. This sink keeps the real request/response on disk so the cause is
//! always one query away.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::db::DbState;
use crate::trace::db::{insert_llm_trace, LlmTraceRow};

/// One LLM HTTP call observed at the GlmChatModel boundary. Built by the call
/// site (stream/generate); `DbTraceSink` maps it to a table row. `session_id`
/// is passed separately to `record_llm_call` (it lives on GlmChatModel, not on
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
}

/// Receives one trace record per LLM call. Implementations must be cheap to
/// share (held inside GlmChatModel behind an `Arc`) and must NOT propagate
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
/// arrives per-call from GlmChatModel.
pub struct DbTraceSink {
    db: DbState,
    conversation_id: Option<String>,
}

impl DbTraceSink {
    pub fn new(db: DbState, conversation_id: Option<String>) -> Self {
        Self { db, conversation_id }
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
pub fn optional_shared(
    db: Option<DbState>,
    conversation_id: Option<String>,
) -> Arc<dyn TraceSink> {
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
        assert!(s.starts_with(prefix), "truncated prefix must sit on a char boundary: {prefix:?}");
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
            },
        );
    }
}
