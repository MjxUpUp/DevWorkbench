//! `eval_runs` table — persisted trajectory-eval results, one row per scored
//! session. Backs the B7 quality-trend view: `eval_trend` GROUP BY date gives
//! the regression curve. The full trajectory snapshot (`trajectory_json`) and
//! the reference used (`reference_json`) are stored for replay/debugging, but
//! the trend query only needs `score` + `created_at`.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// A persisted eval run, surfaced to the frontend. Drops the bulky JSON
/// snapshots (the trend/list views don't need them).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalRunRow {
    pub id: String,
    pub session_id: Option<String>,
    pub conversation_id: Option<String>,
    pub matcher: String,
    pub score: f64,
    pub grade: String,
    pub steps: i64,
    pub created_at: String,
}

/// Input for `insert_eval_run` — the full row including trajectory/reference
/// snapshots (built by the command layer from the scoring result).
#[derive(Debug, Clone)]
pub struct NewEvalRun {
    pub id: String,
    pub session_id: Option<String>,
    pub conversation_id: Option<String>,
    pub matcher: String,
    pub score: f64,
    pub grade: String,
    pub steps: i64,
    pub trajectory_json: Option<String>,
    pub reference_json: Option<String>,
    pub created_at: String,
}

/// One bucket on the regression curve: a UTC day, its mean score, and how many
/// runs landed in it.
#[derive(Debug, Clone, Serialize)]
pub struct TrendPoint {
    pub date: String,
    pub avg_score: f64,
    pub count: i64,
}

/// Persist one eval run.
pub fn insert_eval_run(conn: &Connection, row: &NewEvalRun) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO eval_runs
            (id, session_id, conversation_id, matcher, score, grade, steps,
             trajectory_json, reference_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            row.id,
            row.session_id,
            row.conversation_id,
            row.matcher,
            row.score,
            row.grade,
            row.steps,
            row.trajectory_json,
            row.reference_json,
            row.created_at,
        ],
    )?;
    Ok(())
}

/// List eval runs, newest-first. Scope to a session when `session_id` is Some.
pub fn list_eval_runs(
    conn: &Connection,
    session_id: Option<&str>,
    limit: i64,
) -> Result<Vec<EvalRunRow>, AppError> {
    let mut out = Vec::new();
    if let Some(sid) = session_id {
        let mut stmt = conn.prepare(
            "SELECT id, session_id, conversation_id, matcher, score, grade, steps, created_at
             FROM eval_runs WHERE session_id = ?1 ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![sid, limit], map_row)?;
        for r in rows {
            out.push(r?);
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, session_id, conversation_id, matcher, score, grade, steps, created_at
             FROM eval_runs ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![limit], map_row)?;
        for r in rows {
            out.push(r?);
        }
    }
    Ok(out)
}

/// Daily regression curve over the last `days` days. Buckets by UTC date of
/// `created_at`; returns ASC by date so the chart reads left-to-right.
pub fn eval_trend(conn: &Connection, days: i64) -> Result<Vec<TrendPoint>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT substr(created_at, 1, 10) AS d, AVG(score), COUNT(*)
         FROM eval_runs
         WHERE created_at >= strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?1)
         GROUP BY d
         ORDER BY d ASC",
    )?;
    let rows = stmt.query_map(rusqlite::params![format!("-{days} days")], |row| {
        Ok(TrendPoint {
            date: row.get(0)?,
            avg_score: row.get(1)?,
            count: row.get(2)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EvalRunRow> {
    Ok(EvalRunRow {
        id: row.get(0)?,
        session_id: row.get(1)?,
        conversation_id: row.get(2)?,
        matcher: row.get(3)?,
        score: row.get(4)?,
        grade: row.get(5)?,
        steps: row.get(6)?,
        created_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory DB with the eval_runs table — mirrors the SCHEMA in db.rs.
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE eval_runs (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                conversation_id TEXT,
                matcher TEXT NOT NULL,
                score REAL NOT NULL,
                grade TEXT NOT NULL,
                steps INTEGER NOT NULL,
                trajectory_json TEXT,
                reference_json TEXT,
                created_at TEXT NOT NULL
            );
            CREATE INDEX idx_eval_runs_session ON eval_runs(session_id);
            CREATE INDEX idx_eval_runs_created ON eval_runs(created_at);",
        )
        .unwrap();
        conn
    }

    fn new_run(id: &str, session: &str, score: f64, grade: &str, created: &str) -> NewEvalRun {
        NewEvalRun {
            id: id.into(),
            session_id: Some(session.into()),
            conversation_id: None,
            matcher: "exact_match".into(),
            score,
            grade: grade.into(),
            steps: 3,
            trajectory_json: Some(r#"["read","grep"]"#.into()),
            reference_json: None,
            created_at: created.into(),
        }
    }

    #[test]
    fn insert_then_list_round_trip_newest_first() {
        let conn = test_conn();
        insert_eval_run(&conn, &new_run("r1", "s1", 1.0, "optimal", "2026-06-19T00:00:00Z")).unwrap();
        insert_eval_run(&conn, &new_run("r2", "s1", 0.5, "suboptimal", "2026-06-20T00:00:00Z")).unwrap();
        let rows = list_eval_runs(&conn, Some("s1"), 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "r2", "newest first");
        assert_eq!(rows[0].grade, "suboptimal");
        assert!((rows[0].score - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn list_scopes_to_session_and_all() {
        let conn = test_conn();
        insert_eval_run(&conn, &new_run("a", "s1", 1.0, "optimal", "2026-06-19T00:00:00Z")).unwrap();
        insert_eval_run(&conn, &new_run("b", "s2", 0.0, "incorrect", "2026-06-19T00:00:01Z")).unwrap();
        assert_eq!(list_eval_runs(&conn, Some("s1"), 10).unwrap().len(), 1);
        assert_eq!(list_eval_runs(&conn, None, 10).unwrap().len(), 2);
    }

    #[test]
    fn trend_buckets_by_utc_date_asc() {
        let conn = test_conn();
        let now = chrono::Utc::now().to_rfc3339();
        // Two runs today (one optimal, one incorrect) → one bucket, avg = 0.5.
        insert_eval_run(&conn, &new_run("t1", "s1", 1.0, "optimal", &now)).unwrap();
        insert_eval_run(&conn, &new_run("t2", "s1", 0.0, "incorrect", &now)).unwrap();
        let trend = eval_trend(&conn, 7).unwrap();
        assert_eq!(trend.len(), 1, "both runs are today → one bucket");
        assert_eq!(trend[0].count, 2);
        assert!((trend[0].avg_score - 0.5).abs() < 1e-9);
    }

    #[test]
    fn trend_excludes_runs_outside_window() {
        let conn = test_conn();
        let ancient = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        insert_eval_run(&conn, &new_run("old", "s1", 1.0, "optimal", &ancient)).unwrap();
        let trend = eval_trend(&conn, 7).unwrap();
        assert!(trend.is_empty(), "30-day-old run excluded from 7-day window");
    }
}
