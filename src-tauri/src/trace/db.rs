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
    /// B3: request-send → first response signal (time-to-first-byte). NULL when
    /// the call never reached a first byte (pure network failure) or for
    /// pre-v18 rows. Drives the "slow to start" diagnosis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<i64>,
    /// B3: first-byte → completion (output/stream duration). NULL when there
    /// was no streaming phase (e.g. a headers-only non_2xx) or pre-v18 rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_ms: Option<i64>,
    pub created_at: String,
}

/// Persist one trace row. Fire-and-forget from `DbTraceSink::record_llm_call`
/// (on a blocking thread); errors bubble to the caller which logs them.
pub fn insert_llm_trace(conn: &Connection, row: &LlmTraceRow) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO llm_traces
            (id, session_id, conversation_id, model, base_url, status_code, error_kind,
             req_body, resp_body, latency_ms, input_tokens, output_tokens, ttfb_ms, stream_ms,
             created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
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
            row.ttfb_ms,
            row.stream_ms,
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
                req_body, resp_body, latency_ms, input_tokens, output_tokens, ttfb_ms, stream_ms,
                created_at
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
            ttfb_ms: row.get(12)?,
            stream_ms: row.get(13)?,
            created_at: row.get(14)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Trace retention settings — the single `trace_settings` row, surfaced to the
/// frontend. `retention_days` NULL/<=0 = infinite (the default, per the
/// 2026-06-19 trace observability research — Phoenix's infinite-by-default
/// semantics); a positive N prunes traces older than N days. `last_vacuum_at`
/// throttles VACUUM to weekly and is shown in the settings UI.
#[derive(Debug, Clone, Serialize)]
pub struct TraceSettings {
    pub retention_days: Option<i64>,
    pub last_vacuum_at: Option<String>,
}

/// Read the single trace_settings row (always present — seeded by SCHEMA and the
/// v15 migration). retention_days NULL = infinite.
pub fn get_trace_settings(conn: &Connection) -> Result<TraceSettings, AppError> {
    conn.query_row(
        "SELECT retention_days, last_vacuum_at FROM trace_settings WHERE id = 1",
        [],
        |r| {
            Ok(TraceSettings {
                retention_days: r.get(0)?,
                last_vacuum_at: r.get(1)?,
            })
        },
    )
    .map_err(|e| AppError::Internal(format!("trace_settings read failed: {e}")))
}

/// Set retention_days (NULL = infinite) and stamp updated_at. Does NOT prune;
/// the caller prunes immediately after so the new policy takes effect now.
pub fn set_trace_retention(conn: &Connection, days: Option<i64>) -> Result<(), AppError> {
    conn.execute(
        "UPDATE trace_settings SET retention_days = ?1, updated_at = ?2 WHERE id = 1",
        rusqlite::params![days, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Delete traces older than `retention_days` days. None/<=0 = infinite (no-op,
/// returns 0). Uses a UTC cutoff so the string comparison against the UTC
/// `created_at` (written by `DbTraceSink`) is a true time ordering — mixing
/// timezones here would mis-delete. Backed by `idx_llm_traces_created`. Returns
/// the row count deleted.
pub fn prune_old_traces(conn: &Connection, retention_days: Option<i64>) -> Result<usize, AppError> {
    let Some(days) = retention_days.filter(|d| *d > 0) else {
        return Ok(0);
    };
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days);
    let cutoff_str = cutoff.to_rfc3339();
    let deleted = conn.execute(
        "DELETE FROM llm_traces WHERE created_at < ?1",
        [&cutoff_str],
    )?;
    Ok(deleted)
}

/// Run VACUUM when `last_vacuum_at` is missing or older than 7 days, then stamp
/// it. Skips otherwise. SQLite does not reclaim disk after DELETE without
/// VACUUM; throttling to weekly avoids the rewrite cost on every startup.
/// Returns true iff a VACUUM ran.
pub fn maybe_vacuum(conn: &Connection, settings: &TraceSettings) -> Result<bool, AppError> {
    let due = match settings.last_vacuum_at.as_deref() {
        None => true,
        Some(ts) => match chrono::DateTime::parse_from_rfc3339(ts) {
            Ok(prev) => chrono::Utc::now().signed_duration_since(prev) > chrono::Duration::days(7),
            Err(_) => true, // unparseable stamp → treat as due (safe, just vacuums)
        },
    };
    if !due {
        return Ok(false);
    }
    conn.execute_batch("VACUUM")?;
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE trace_settings SET last_vacuum_at = ?1, updated_at = ?1 WHERE id = 1",
        [&now],
    )?;
    Ok(true)
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
                ttfb_ms INTEGER,
                stream_ms INTEGER,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_llm_traces_created ON llm_traces(created_at);
            CREATE TABLE IF NOT EXISTS trace_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                retention_days INTEGER,
                last_vacuum_at TEXT,
                updated_at TEXT NOT NULL
            );
            INSERT INTO trace_settings (id, retention_days, last_vacuum_at, updated_at)
            VALUES (1, NULL, '2026-06-19T00:00:00Z', '2026-06-19T00:00:00Z');",
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
            ttfb_ms: None,
            stream_ms: None,
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
        // The INSERT path must tolerate NULL optional fields (a network error
        // row, for instance, has no resp_body). Independent of the 2xx policy.
        let conn = test_conn();
        let mut row = sample("ok", "s1", "2026-06-19T00:00:00Z");
        row.status_code = Some(200);
        row.error_kind = None;
        row.resp_body = None;
        insert_llm_trace(&conn, &row).unwrap();
    }

    #[test]
    fn insert_then_list_round_trips_timing_breakdown() {
        // B3: the ttfb_ms / stream_ms columns must survive INSERT → SELECT so
        // TraceView can show the per-phase timing split (time-to-first-byte vs
        // output/stream duration), not just total latency.
        let conn = test_conn();
        let mut row = sample("timed", "s1", "2026-06-19T00:00:00Z");
        row.status_code = Some(200);
        row.error_kind = None;
        row.latency_ms = Some(5_000);
        row.ttfb_ms = Some(1_200);
        row.stream_ms = Some(3_800);
        insert_llm_trace(&conn, &row).unwrap();

        let rows = list_traces_for_session(&conn, "s1").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ttfb_ms, Some(1_200), "ttfb_ms must round-trip");
        assert_eq!(rows[0].stream_ms, Some(3_800), "stream_ms must round-trip");
        assert_eq!(rows[0].latency_ms, Some(5_000));
    }

    #[test]
    fn timing_columns_default_null_for_legacy_shape() {
        // A row written with ttfb/stream still None (legacy / pure network
        // failure) must round-trip as None, not 0 — None is the honest "this
        // phase never happened" signal the TimingChecker relies on.
        let conn = test_conn();
        insert_llm_trace(&conn, &sample("netfail", "s1", "2026-06-19T00:00:00Z")).unwrap();
        let rows = list_traces_for_session(&conn, "s1").unwrap();
        assert_eq!(rows[0].ttfb_ms, None);
        assert_eq!(rows[0].stream_ms, None);
    }

    #[test]
    fn insert_2xx_persists_full_resp_body() {
        // A clean 2xx now stores the FULL wire response body (truncated on the
        // Rust side), symmetric with the error path — the 2026-06-19 trace
        // observability research found "2xx stores NULL" to be an industry
        // outlier. The INSERT + round-trip must carry it, not just the NULL case.
        let conn = test_conn();
        let mut row = sample("ok2xx", "s1", "2026-06-19T00:00:00Z");
        row.status_code = Some(200);
        row.error_kind = None;
        row.resp_body = Some(r#"{"content":[{"type":"text","text":"hi"}]}"#.to_string());
        insert_llm_trace(&conn, &row).unwrap();
        let rows = list_traces_for_session(&conn, "s1").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status_code, Some(200));
        assert!(rows[0].error_kind.is_none());
        assert!(
            rows[0].resp_body.as_deref().unwrap_or("").contains("hi"),
            "2xx resp_body must round-trip, got: {:?}",
            rows[0].resp_body
        );
    }

    #[test]
    fn prune_old_traces_deletes_past_retention_keeps_recent() {
        let conn = test_conn();
        let two_days_ago = (chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339();
        let now = chrono::Utc::now().to_rfc3339();
        insert_llm_trace(&conn, &sample("old", "s1", &two_days_ago)).unwrap();
        insert_llm_trace(&conn, &sample("fresh", "s1", &now)).unwrap();

        let deleted = prune_old_traces(&conn, Some(1)).unwrap();
        assert_eq!(
            deleted, 1,
            "the 2-day-old trace is past the 1-day retention"
        );

        let rows = list_traces_for_session(&conn, "s1").unwrap();
        assert_eq!(rows.len(), 1, "the recent trace is kept");
        assert_eq!(rows[0].id, "fresh");
    }

    #[test]
    fn prune_old_traces_infinite_when_null_or_nonpositive() {
        let conn = test_conn();
        let ancient = (chrono::Utc::now() - chrono::Duration::days(365)).to_rfc3339();
        insert_llm_trace(&conn, &sample("ancient", "s1", &ancient)).unwrap();
        // NULL / 0 / negative retention = infinite → nothing deleted.
        assert_eq!(prune_old_traces(&conn, None).unwrap(), 0);
        assert_eq!(prune_old_traces(&conn, Some(0)).unwrap(), 0);
        assert_eq!(prune_old_traces(&conn, Some(-5)).unwrap(), 0);
        assert_eq!(
            list_traces_for_session(&conn, "s1").unwrap().len(),
            1,
            "infinite retention keeps even a year-old trace"
        );
    }

    #[test]
    fn trace_settings_get_set_round_trip() {
        let conn = test_conn();
        // test_conn seeds the default row: NULL retention = infinite.
        let s = get_trace_settings(&conn).unwrap();
        assert_eq!(s.retention_days, None, "default retention is infinite");
        assert_eq!(s.last_vacuum_at.as_deref(), Some("2026-06-19T00:00:00Z"));

        set_trace_retention(&conn, Some(30)).unwrap();
        let s = get_trace_settings(&conn).unwrap();
        assert_eq!(s.retention_days, Some(30), "retention persisted");

        set_trace_retention(&conn, None).unwrap();
        let s = get_trace_settings(&conn).unwrap();
        assert_eq!(s.retention_days, None, "NULL resets to infinite");
    }

    #[test]
    fn maybe_vacuum_runs_when_due_then_throttled_within_window() {
        let conn = test_conn();
        // Force a due state: clear last_vacuum_at (NULL = never vacuumed).
        conn.execute(
            "UPDATE trace_settings SET last_vacuum_at = NULL WHERE id = 1",
            [],
        )
        .unwrap();
        let due = get_trace_settings(&conn).unwrap();
        assert!(
            maybe_vacuum(&conn, &due).unwrap(),
            "NULL last_vacuum_at → VACUUM runs"
        );

        let after = get_trace_settings(&conn).unwrap();
        assert!(
            after.last_vacuum_at.is_some(),
            "last_vacuum_at stamped after VACUUM"
        );

        // Immediately again → throttled (just stamped, well within 7 days).
        let not_due = get_trace_settings(&conn).unwrap();
        assert!(
            !maybe_vacuum(&conn, &not_due).unwrap(),
            "throttled within the 7-day window"
        );
    }
}
