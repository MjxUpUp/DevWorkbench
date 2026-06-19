//! LLM trace observability commands. The read side of `trace::sink` — the
//! write side is `DbTraceSink::record_llm_call`, fire-and-forget from
//! `GlmChatModel::stream`/`generate`. This command powers the frontend
//! TraceView: given a session id, return every LLM HTTP call that session
//! made (req/resp body + status + latency), oldest-first.

use tauri::State;

use crate::db::DbState;
use crate::error::AppError;
use crate::trace::db::{list_traces_for_session, LlmTraceRow};

/// Load all LLM HTTP traces for a session, oldest-first. Returns an empty vec
/// (NOT an error) when the session made no LLM calls — e.g. it failed before
/// its first request, or it's a non-kernel agent (CLI) with no trace sink.
/// That distinction matters for the UI: empty = "nothing to show", error =
/// "the trace store is broken".
#[tauri::command]
pub async fn list_llm_traces(
    db: State<'_, DbState>,
    session_id: String,
) -> Result<Vec<LlmTraceRow>, AppError> {
    let conn = db.get().map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    list_traces_for_session(&conn, &session_id)
}
