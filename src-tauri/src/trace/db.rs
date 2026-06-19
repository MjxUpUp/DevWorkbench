//! Row type + INSERT/SELECT for the `llm_traces` table. Mirrors
//! `crate::cost::agentfare` (the cost_records write/read helper).

use rusqlite::Connection;
use serde::Serialize;

use crate::error::AppError;

/// One persisted LLM call trace row. Serialized as-is to the frontend by the
/// `list_llm_traces` command (snake_case fields align with the TS type).
#[derive(Debug, Clone, Serialize)]
pub struct LlmTraceRow {
    pub id: String,
    pub session_id: Option<String>,
    pub conversation_id: Option<String>,
    pub model: String,
    pub base_url: String,
    pub status_code: Option<i64>,
    pub error_kind: Option<String>,
    pub req_body: String,
    pub resp_body: Option<String>,
    pub latency_ms: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub created_at: String,
}

/// Persist one trace row. Fire-and-forget from `DbTraceSink::record_llm_call`
/// (on a blocking thread); errors bubble to the caller which logs them.
pub fn insert_llm_trace(conn: &Connection, row: &LlmTraceRow) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO llm_traces
            (id, session_id, conversation_id, model, base_url, status_code, error_kind,
             req_body, resp_body, latency_ms, input_tokens, output_tokens, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        rusqlite::params![
            row.id,
            row.session_id,
            row.conversation_id,
            row.model,
            row.base_url,
            row.status_code,
            row.error_kind,
            row.req_body,
            row.resp_body,
            row.latency_ms,
            row.input_tokens,
            row.output_tokens,
            row.created_at,
        ],
    )?;
    Ok(())
}

/// Load all traces for a session, oldest-first (the order they happened). Empty
/// vec (not an error) when the session made no LLM calls (e.g. failed before
/// the first request, or a non-kernel agent).
pub fn list_traces_for_session(
    conn: &Connection,
    session_id: &str,
) -> Result<Vec<LlmTraceRow>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, conversation_id, model, base_url, status_code, error_kind,
                req_body, resp_body, latency_ms, input_tokens, output_tokens, created_at
         FROM llm_traces
         WHERE session_id = ?1
         ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map([session_id], |row| {
        Ok(LlmTraceRow {
            id: row.get(0)?,
            session_id: row.get(1)?,
            conversation_id: row.get(2)?,
            model: row.get(3)?,
            base_url: row.get(4)?,
            status_code: row.get(5)?,
            error_kind: row.get(6)?,
            req_body: row.get(7)?,
            resp_body: row.get(8)?,
            latency_ms: row.get(9)?,
            input_tokens: row.get(10)?,
            output_tokens: row.get(11)?,
            created_at: row.get(12)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory DB with the llm_traces table — mirrors the DDL in db.rs SCHEMA
    /// (duplicated here to avoid reaching into the private SCHEMA constant,
    /// same pattern as cost/agentfare.rs).
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE llm_traces (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                conversation_id TEXT,
                model TEXT NOT NULL,
                base_url TEXT NOT NULL,
                status_code INTEGER,
                error_kind TEXT,
                req_body TEXT,
                resp_body TEXT,
                latency_ms INTEGER,
                input_tokens INTEGER,
                output_tokens INTEGER,
                created_at TEXT NOT NULL
            );",
        )
        .unwrap();
        conn
    }

    fn sample(id: &str, session: &str, created: &str) -> LlmTraceRow {
        LlmTraceRow {
            id: id.into(),
            session_id: Some(session.into()),
            conversation_id: None,
            model: "glm-4.6".into(),
            base_url: "https://x".into(),
            status_code: Some(400),
            error_kind: Some("non_2xx".into()),
            req_body: "{\"model\":\"glm-4.6\"}".into(),
            resp_body: Some("invalid".into()),
            latency_ms: Some(8),
            input_tokens: None,
            output_tokens: None,
            created_at: created.into(),
        }
    }

    #[test]
    fn insert_then_list_round_trip_oldest_first() {
        let conn = test_conn();
        // Insert out of order; list must return ASC by created_at (call order).
        insert_llm_trace(&conn, &sample("late", "s1", "2026-06-19T00:28:31Z")).unwrap();
        insert_llm_trace(&conn, &sample("early", "s1", "2026-06-19T00:28:30Z")).unwrap();
        let rows = list_traces_for_session(&conn, "s1").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "early", "must be ASC by created_at");
        assert_eq!(rows[1].id, "late");
        assert_eq!(rows[0].status_code, Some(400));
        assert_eq!(rows[0].error_kind.as_deref(), Some("non_2xx"));
    }

    #[test]
    fn list_filters_by_session_and_empty_is_ok() {
        let conn = test_conn();
        insert_llm_trace(&conn, &sample("a", "s1", "2026-06-19T00:00:00Z")).unwrap();
        insert_llm_trace(&conn, &sample("b", "s2", "2026-06-19T00:00:01Z")).unwrap();
        assert_eq!(list_traces_for_session(&conn, "s1").unwrap().len(), 1);
        assert_eq!(list_traces_for_session(&conn, "s2").unwrap().len(), 1);
        // Unknown session → empty vec, not an error.
        assert_eq!(list_traces_for_session(&conn, "missing").unwrap().len(), 0);
    }

    #[test]
    fn insert_success_shape_with_nulls_succeeds() {
        // A clean 2xx call: status present, but error_kind/resp_body NULL.
        let conn = test_conn();
        let mut row = sample("ok", "s1", "2026-06-19T00:00:00Z");
        row.status_code = Some(200);
        row.error_kind = None;
        row.resp_body = None;
        insert_llm_trace(&conn, &row).unwrap();
    }
}
