//! LLM trace observability commands. The read side of `trace::sink` — the
//! write side is `DbTraceSink::record_llm_call`, fire-and-forget from
//! `ChatModel::stream`/`generate`. This command powers the frontend
//! TraceView: given a session id, return every LLM HTTP call that session
//! made (req/resp body + status + latency), oldest-first.

use tauri::State;

use crate::db::DbState;
use crate::error::AppError;
use crate::trace::db::{
    LlmTraceRow, TraceSettings, get_trace_settings, list_traces_for_session, maybe_vacuum,
    prune_old_traces, set_trace_retention,
};

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
    let conn = db
        .get()
        .map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    list_traces_for_session(&conn, &session_id)
}

/// Read trace retention settings (retention_days NULL = infinite).
#[tauri::command]
pub async fn get_trace_settings_cmd(db: State<'_, DbState>) -> Result<TraceSettings, AppError> {
    let conn = db
        .get()
        .map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    get_trace_settings(&conn)
}

/// Set the trace retention in days (NULL/<=0 = infinite). Prunes immediately so
/// the new policy takes effect now (and VACUUMs if due), instead of waiting for
/// the next startup. Returns the number of rows pruned.
#[tauri::command]
pub async fn set_trace_retention_cmd(
    db: State<'_, DbState>,
    days: Option<i64>,
) -> Result<usize, AppError> {
    let conn = db
        .get()
        .map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    set_trace_retention(&conn, days)?;
    let pruned = prune_old_traces(&conn, days)?;
    if pruned > 0 {
        let _ = maybe_vacuum(&conn, &get_trace_settings(&conn)?);
    }
    Ok(pruned)
}

/// Manually prune now (respecting the current retention) and VACUUM. The
/// "clean up now" button. Returns the number of rows pruned.
#[tauri::command]
pub async fn prune_llm_traces_now(db: State<'_, DbState>) -> Result<usize, AppError> {
    let conn = db
        .get()
        .map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    let settings = get_trace_settings(&conn)?;
    let pruned = prune_old_traces(&conn, settings.retention_days)?;
    if pruned > 0 {
        let _ = maybe_vacuum(&conn, &settings);
    }
    Ok(pruned)
}
