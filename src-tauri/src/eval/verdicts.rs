//! `verdicts` table — the L1 verdict ledger. One row per gate verdict
//! (verify / honesty / forge) or circuit-breaker transition (TRIPPED / RESET)
//! emitted during an agent run or a platform eval. The `attribution` column is
//! where the anti-gaming stance lives (反刷分三原则): a gain with no verifiable
//! causal chain lands as `BRAKE` (unattributed = brake), not as a win. This
//! module is the persistence primitive only — *who* writes a verdict (executor
//! run_gate, circuit-breaker transition, eval scorer) is wired in the L1
//! integration step on top of this table.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// A persisted verdict row, surfaced to the frontend / eval panel. `report`
/// (the gate's detail JSON) is kept — unlike eval_runs' trajectory snapshot,
/// verdicts are small enough to ship whole, and the panel renders the report
/// inline (honesty findings / forge score / verify rubric verdict).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerdictRow {
    pub id: String,
    pub session_id: Option<String>,
    pub case_id: Option<String>,
    pub gate: String,
    pub verdict: String,
    pub attribution: Option<String>,
    pub report: Option<String>,
    pub commit_sha: Option<String>,
    pub created_at: String,
}

/// Input for `insert_verdict` — built at the call site (executor run_gate /
/// circuit-breaker transition / eval scorer) from the gate result.
/// `attribution` is `None` for events that carry no human-attributable cause
/// (circuit-breaker TRIPPED — a host going down is not "your work").
#[derive(Debug, Clone)]
pub struct NewVerdict {
    pub id: String,
    pub session_id: Option<String>,
    pub case_id: Option<String>,
    pub gate: String,
    pub verdict: String,
    pub attribution: Option<String>,
    pub report: Option<String>,
    pub commit_sha: Option<String>,
    pub created_at: String,
}

/// Persist one verdict.
pub fn insert_verdict(conn: &Connection, row: &NewVerdict) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO verdicts
            (id, session_id, case_id, gate, verdict, attribution, report, commit_sha, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            row.id,
            row.session_id,
            row.case_id,
            row.gate,
            row.verdict,
            row.attribution,
            row.report,
            row.commit_sha,
            row.created_at,
        ],
    )?;
    Ok(())
}

/// List verdicts, newest-first. Any of `session_id` / `gate` / `case_id` may
/// scope the query (AND-combined); all-None lists across everything. The panel
/// uses `gate` to slice by gate kind (verify / honesty / forge / circuit-breaker
/// / eval) and `case_id` to pull a replay run's verdicts.
pub fn list_verdicts(
    conn: &Connection,
    session_id: Option<&str>,
    gate: Option<&str>,
    case_id: Option<&str>,
    limit: i64,
) -> Result<Vec<VerdictRow>, AppError> {
    let mut sql = String::from(
        "SELECT id, session_id, case_id, gate, verdict, attribution, report, commit_sha, created_at
         FROM verdicts",
    );
    let mut clauses: Vec<&str> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(sid) = session_id {
        clauses.push("session_id = ?");
        params.push(Box::new(sid.to_string()));
    }
    if let Some(g) = gate {
        clauses.push("gate = ?");
        params.push(Box::new(g.to_string()));
    }
    if let Some(c) = case_id {
        clauses.push("case_id = ?");
        params.push(Box::new(c.to_string()));
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ?");
    params.push(Box::new(limit));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), map_row)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<VerdictRow> {
    Ok(VerdictRow {
        id: row.get(0)?,
        session_id: row.get(1)?,
        case_id: row.get(2)?,
        gate: row.get(3)?,
        verdict: row.get(4)?,
        attribution: row.get(5)?,
        report: row.get(6)?,
        commit_sha: row.get(7)?,
        created_at: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory DB with the verdicts table — mirrors the SCHEMA in db.rs.
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE verdicts (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                case_id TEXT,
                gate TEXT NOT NULL,
                verdict TEXT NOT NULL,
                attribution TEXT,
                report TEXT,
                commit_sha TEXT,
                created_at TEXT NOT NULL
            );
            CREATE INDEX idx_verdicts_session ON verdicts(session_id);
            CREATE INDEX idx_verdicts_gate ON verdicts(gate);
            CREATE INDEX idx_verdicts_created ON verdicts(created_at);",
        )
        .unwrap();
        conn
    }

    fn new_verdict(
        id: &str,
        session: &str,
        gate: &str,
        verdict: &str,
        attribution: Option<&str>,
        created: &str,
    ) -> NewVerdict {
        NewVerdict {
            id: id.into(),
            session_id: Some(session.into()),
            case_id: None,
            gate: gate.into(),
            verdict: verdict.into(),
            attribution: attribution.map(|s| s.into()),
            report: Some(r#"{"status":"ok"}"#.into()),
            commit_sha: Some("abc1234".into()),
            created_at: created.into(),
        }
    }

    #[test]
    fn insert_then_list_round_trip_newest_first() {
        let conn = test_conn();
        insert_verdict(
            &conn,
            &new_verdict("v1", "s1", "forge", "PASS", Some("CLEAR"), "2026-07-02T00:00:00Z"),
        )
        .unwrap();
        insert_verdict(
            &conn,
            &new_verdict(
                "v2",
                "s1",
                "honesty",
                "FAIL",
                Some("BRAKE"),
                "2026-07-02T00:00:01Z",
            ),
        )
        .unwrap();
        let rows = list_verdicts(&conn, Some("s1"), None, None, 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "v2", "newest first");
        assert_eq!(rows[0].gate, "honesty");
        assert_eq!(rows[0].verdict, "FAIL");
        assert_eq!(rows[0].attribution.as_deref(), Some("BRAKE"));
        assert_eq!(rows[0].report.as_deref(), Some(r#"{"status":"ok"}"#));
    }

    #[test]
    fn list_scopes_to_session_and_all() {
        let conn = test_conn();
        insert_verdict(
            &conn,
            &new_verdict("a", "s1", "forge", "PASS", Some("CLEAR"), "2026-07-02T00:00:00Z"),
        )
        .unwrap();
        insert_verdict(
            &conn,
            &new_verdict("b", "s2", "verify", "FAIL", None, "2026-07-02T00:00:01Z"),
        )
        .unwrap();
        assert_eq!(list_verdicts(&conn, Some("s1"), None, None, 10).unwrap().len(), 1);
        assert_eq!(list_verdicts(&conn, None, None, None, 10).unwrap().len(), 2);
    }

    #[test]
    fn attribution_nullable_for_circuit_events() {
        let conn = test_conn();
        // circuit-breaker TRIPPED has no human-attributable cause → attribution
        // is NULL (a host going down is not "your work"). This is the contract
        // the integration step relies on when persisting circuit events.
        insert_verdict(
            &conn,
            &new_verdict(
                "c",
                "s1",
                "circuit-breaker",
                "TRIPPED",
                None,
                "2026-07-02T00:00:00Z",
            ),
        )
        .unwrap();
        let rows = list_verdicts(&conn, Some("s1"), None, None, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].attribution.is_none(),
            "circuit events carry no attribution"
        );
        assert_eq!(rows[0].gate, "circuit-breaker");
        assert_eq!(rows[0].verdict, "TRIPPED");
    }

    #[test]
    fn brake_attribution_surfaces_in_list() {
        // 反刷分三原则: an unattributed gain is a BRAKE, not a win. The panel
        // filters on attribution to surface these, so the round-trip must
        // preserve the exact token.
        let conn = test_conn();
        insert_verdict(
            &conn,
            &new_verdict(
                "br",
                "s1",
                "verify",
                "PASS",
                Some("BRAKE"),
                "2026-07-02T00:00:00Z",
            ),
        )
        .unwrap();
        let rows = list_verdicts(&conn, Some("s1"), None, None, 10).unwrap();
        assert_eq!(rows[0].attribution.as_deref(), Some("BRAKE"));
    }

    #[test]
    fn list_filters_by_gate_and_case() {
        // The panel slices the ledger by gate kind and by case_id (a replay
        // run's verdicts). Both filters AND-combine with session_id.
        let conn = test_conn();
        insert_verdict(
            &conn,
            &new_verdict("v1", "s1", "forge", "PASS", Some("CLEAR"), "2026-07-02T00:00:00Z"),
        )
        .unwrap();
        insert_verdict(
            &conn,
            &new_verdict("v2", "s1", "verify", "FAIL", None, "2026-07-02T00:00:01Z"),
        )
        .unwrap();
        // gate filter: only the verify verdict.
        let verify_only = list_verdicts(&conn, None, Some("verify"), None, 10).unwrap();
        assert_eq!(verify_only.len(), 1);
        assert_eq!(verify_only[0].gate, "verify");
        // gate filter with no match → empty.
        assert!(list_verdicts(&conn, None, Some("honesty"), None, 10)
            .unwrap()
            .is_empty());
        // case_id filter: no verdict here carries a case_id (new_verdict sets
        // None) → empty, proving the case filter is applied, not ignored.
        assert!(list_verdicts(&conn, None, None, Some("case-99"), 10)
            .unwrap()
            .is_empty());
    }
}
