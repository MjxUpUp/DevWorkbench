//! L2 eval cases — the deterministic contract a replay (L3) or paired
//! comparison (L4) runs against. 反刷分三原则 #1 (客观事实代码判): the
//! `expected_steps_json` / `expected_observables_json` fields are deterministic
//! facts frozen from a real past run, NOT LLM-generated; an LLM is used only to
//! *judge* `expected_output`, never to invent the target. That separation is the
//! whole point — a deterministic target can't be gamed by a fluent wrong answer.
//!
//! A case built straight off a trajectory ([`build_draft_case_from_trajectory]])
//! is a `draft`: its `expected_steps` are frozen verbatim from
//! [`extract::extract_trajectory`], and `draft = true` blocks it from anchoring a
//! paired replay until an independent review flips it ([`approve_draft`]). This
//! stops an agent from self-certifying "whatever I did = the answer" — the most
//! direct刷分 attack on an eval suite. `negative_json` carries counter-examples
//! (steps/output that must NOT happen), the guard against right-steps-but-
//! wrong-outcome刷分.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::eval::extract::extract_trajectory;
use crate::trace::db::LlmTraceRow;

/// A persisted eval case, surfaced to the frontend / replay driver. Mirrors the
/// row shape; `draft` is decoded from the SQL INTEGER (0/1) to a bool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalCaseRow {
    pub id: String,
    pub name: String,
    pub category: String,
    pub input_prompt: String,
    pub expected_steps_json: Option<String>,
    pub expected_output: Option<String>,
    pub expected_observables_json: Option<String>,
    pub negative_json: Option<String>,
    pub source_session_id: Option<String>,
    pub commit_sha: Option<String>,
    pub draft: bool,
    pub created_at: String,
}

/// Input for [`insert_eval_case`] — the full row. Built either by hand (a
/// reviewed case) or by [`build_draft_case_from_trajectory`] (a draft awaiting
/// review).
#[derive(Debug, Clone)]
pub struct NewEvalCase {
    pub id: String,
    pub name: String,
    pub category: String,
    pub input_prompt: String,
    pub expected_steps_json: Option<String>,
    pub expected_output: Option<String>,
    pub expected_observables_json: Option<String>,
    pub negative_json: Option<String>,
    pub source_session_id: Option<String>,
    pub commit_sha: Option<String>,
    pub draft: bool,
    pub created_at: String,
}

/// Persist one eval case.
pub fn insert_eval_case(conn: &Connection, row: &NewEvalCase) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO eval_cases
            (id, name, category, input_prompt, expected_steps_json, expected_output,
             expected_observables_json, negative_json, source_session_id, commit_sha,
             draft, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            row.id,
            row.name,
            row.category,
            row.input_prompt,
            row.expected_steps_json,
            row.expected_output,
            row.expected_observables_json,
            row.negative_json,
            row.source_session_id,
            row.commit_sha,
            row.draft,
            row.created_at,
        ],
    )?;
    Ok(())
}

/// Fetch a single case by id (used by the replay driver to load its contract).
pub fn get_eval_case(conn: &Connection, id: &str) -> Result<Option<EvalCaseRow>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, category, input_prompt, expected_steps_json, expected_output,
                expected_observables_json, negative_json, source_session_id, commit_sha,
                draft, created_at
         FROM eval_cases WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![id], map_row)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// List cases, newest-first. Scope to a category when `category` is Some. By
/// default a draft (un-reviewed) case is EXCLUDED — the replay/paired paths must
/// only see approved contracts; pass `include_drafts = true` for the case-editor
/// view that needs to show pending drafts.
pub fn list_eval_cases(
    conn: &Connection,
    category: Option<&str>,
    include_drafts: bool,
    limit: i64,
) -> Result<Vec<EvalCaseRow>, AppError> {
    let mut sql = String::from(
        "SELECT id, name, category, input_prompt, expected_steps_json, expected_output,
                expected_observables_json, negative_json, source_session_id, commit_sha,
                draft, created_at
         FROM eval_cases",
    );
    // Build the WHERE clause and its bound params in lock-step so the `?`
    // placeholders line up with the params vec regardless of which filters are
    // active. `draft IS 0` is parameterless (a literal in the SQL).
    let mut clauses: Vec<&str> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(cat) = category {
        clauses.push("category = ?");
        params.push(Box::new(cat.to_string()));
    }
    if !include_drafts {
        clauses.push("draft IS 0");
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

/// Approve a draft case — flips `draft` to 0 (approved). The only path by which
/// a trajectory-frozen case becomes eligible to anchor a paired replay. Returns
/// the number of rows touched (0 = no such case / already approved).
pub fn approve_draft(conn: &Connection, id: &str) -> Result<usize, AppError> {
    conn.execute(
        "UPDATE eval_cases SET draft = 0 WHERE id = ?1 AND draft IS 1",
        rusqlite::params![id],
    )
    .map_err(AppError::from)
}

/// Build a DRAFT case from a real session's trajectory. The deterministic
/// `expected_steps` are frozen verbatim from [`extract_trajectory`] (客观事实 —
/// the tools the agent actually called, in order); the LLM-judged target fields
/// (`expected_output` / `expected_observables` / `negative`) are left NULL for
/// the reviewer to fill, and `draft = true` blocks the case from anchoring a
/// paired replay until [`approve_draft`] runs. This is the anti-self-certification
/// gate: the agent's own past run can seed a case, but cannot ratify one.
pub fn build_draft_case_from_trajectory(
    name: &str,
    category: &str,
    input_prompt: &str,
    traces: &[LlmTraceRow],
    source_session_id: Option<&str>,
    commit_sha: Option<&str>,
    created_at: &str,
) -> NewEvalCase {
    let steps = extract_trajectory(traces);
    // Freeze the ordered tool-call sequence verbatim. An empty trajectory (no
    // tool calls — e.g. a plain-text turn) yields None, not "[]": a case with
    // no deterministic step contract is meaningless as a replay target, so the
    // caller should skip rather than persist a vacuous case.
    let expected_steps_json = if steps.is_empty() {
        None
    } else {
        serde_json::to_string(&steps).ok()
    };
    NewEvalCase {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.to_string(),
        category: category.to_string(),
        input_prompt: input_prompt.to_string(),
        expected_steps_json,
        // LLM-judged / observable targets are filled by the independent review,
        // never auto-derived from the run being frozen.
        expected_output: None,
        expected_observables_json: None,
        negative_json: None,
        source_session_id: source_session_id.map(String::from),
        commit_sha: commit_sha.map(String::from),
        draft: true,
        created_at: created_at.to_string(),
    }
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EvalCaseRow> {
    Ok(EvalCaseRow {
        id: row.get(0)?,
        name: row.get(1)?,
        category: row.get(2)?,
        input_prompt: row.get(3)?,
        expected_steps_json: row.get(4)?,
        expected_output: row.get(5)?,
        expected_observables_json: row.get(6)?,
        negative_json: row.get(7)?,
        source_session_id: row.get(8)?,
        commit_sha: row.get(9)?,
        draft: row.get(10)?,
        created_at: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory DB with the eval_cases table — mirrors the SCHEMA in db.rs.
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE eval_cases (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                category TEXT NOT NULL,
                input_prompt TEXT NOT NULL,
                expected_steps_json TEXT,
                expected_output TEXT,
                expected_observables_json TEXT,
                negative_json TEXT,
                source_session_id TEXT,
                commit_sha TEXT,
                draft INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL
            );
            CREATE INDEX idx_eval_cases_category ON eval_cases(category);
            CREATE INDEX idx_eval_cases_draft ON eval_cases(draft);
            CREATE INDEX idx_eval_cases_created ON eval_cases(created_at);",
        )
        .unwrap();
        conn
    }

    fn new_case(id: &str, name: &str, category: &str, draft: bool, created: &str) -> NewEvalCase {
        NewEvalCase {
            id: id.into(),
            name: name.into(),
            category: category.into(),
            input_prompt: "do the thing".into(),
            expected_steps_json: Some(r#"[{"name":"read"}]"#.into()),
            expected_output: None,
            expected_observables_json: None,
            negative_json: None,
            source_session_id: Some("sess-orig".into()),
            commit_sha: None,
            draft,
            created_at: created.into(),
        }
    }

    #[test]
    fn insert_then_get_round_trip_decodes_draft_bool() {
        let conn = test_conn();
        insert_eval_case(&conn, &new_case("c1", "add-auth", "agent", true, "2026-07-02T00:00:00Z"))
            .unwrap();
        let row = get_eval_case(&conn, "c1").unwrap().expect("case exists");
        assert_eq!(row.name, "add-auth");
        assert_eq!(row.category, "agent");
        assert_eq!(row.expected_steps_json.as_deref(), Some(r#"[{"name":"read"}]"#));
        assert!(row.draft, "draft round-trips as true (SQL INTEGER 1 → bool)");
        // Round-trips a missing case too.
        assert!(get_eval_case(&conn, "nope").unwrap().is_none());
    }

    #[test]
    fn list_defaults_exclude_drafts_so_replay_only_sees_approved() {
        let conn = test_conn();
        insert_eval_case(&conn, &new_case("draft", "d", "agent", true, "2026-07-02T00:00:00Z"))
            .unwrap();
        insert_eval_case(&conn, &new_case("approved", "a", "agent", false, "2026-07-02T00:00:01Z"))
            .unwrap();
        // Default (include_drafts=false): only the approved case surfaces.
        let view = list_eval_cases(&conn, None, false, 10).unwrap();
        assert_eq!(view.len(), 1, "draft excluded from the replay view");
        assert_eq!(view[0].id, "approved");
        // Editor view (include_drafts=true): both.
        let editor = list_eval_cases(&conn, None, true, 10).unwrap();
        assert_eq!(editor.len(), 2);
        assert_eq!(editor[0].id, "approved", "newest first");
    }

    #[test]
    fn list_filters_by_category() {
        let conn = test_conn();
        insert_eval_case(&conn, &new_case("a1", "n", "agent", false, "2026-07-02T00:00:00Z")).unwrap();
        insert_eval_case(
            &conn,
            &new_case("p1", "n", "platform-mechanism", false, "2026-07-02T00:00:01Z"),
        )
        .unwrap();
        let agent_only = list_eval_cases(&conn, Some("agent"), false, 10).unwrap();
        assert_eq!(agent_only.len(), 1);
        assert_eq!(agent_only[0].id, "a1");
    }

    #[test]
    fn approve_draft_flips_to_approved_and_is_idempotent() {
        let conn = test_conn();
        insert_eval_case(&conn, &new_case("c", "n", "agent", true, "2026-07-02T00:00:00Z")).unwrap();
        assert_eq!(approve_draft(&conn, "c").unwrap(), 1, "one row flipped");
        assert!(!get_eval_case(&conn, "c").unwrap().unwrap().draft);
        // Re-approving an already-approved case is a no-op (0 rows), not an error.
        assert_eq!(approve_draft(&conn, "c").unwrap(), 0);
    }

    #[test]
    fn build_draft_case_freezes_trajectory_and_marks_draft() {
        // A real Anthropic-shape trace carrying one tool_use → frozen as
        // expected_steps, draft=true, LLM-judged targets NULL.
        let body = r#"{"content":[{"type":"tool_use","name":"read","id":"1"}]}"#;
        let trace = LlmTraceRow {
            id: "t1".into(),
            session_id: Some("s1".into()),
            conversation_id: None,
            model: "glm-4.6".into(),
            base_url: "https://x".into(),
            status_code: Some(200),
            error_kind: None,
            req_body: "{}".into(),
            resp_body: Some(body.into()),
            latency_ms: None,
            input_tokens: None,
            output_tokens: None,
            ttfb_ms: None,
            stream_ms: None,
            created_at: "2026-07-02T00:00:00Z".into(),
        };
        let case = build_draft_case_from_trajectory(
            "frozen",
            "agent",
            "do the thing",
            &[trace],
            Some("s1"),
            None,
            "2026-07-02T00:00:01Z",
        );
        assert!(case.draft, "trajectory-frozen case is a draft until reviewed");
        assert_eq!(case.source_session_id.as_deref(), Some("s1"));
        assert!(case.expected_steps_json.is_some());
        assert!(
            case.expected_steps_json.as_deref().unwrap().contains("read"),
            "frozen verbatim from the trajectory: {}",
            case.expected_steps_json.as_deref().unwrap()
        );
        // LLM-judged / observable targets are NOT auto-derived — left for review.
        assert!(case.expected_output.is_none());
        assert!(case.expected_observables_json.is_none());
        assert!(case.negative_json.is_none());
    }

    #[test]
    fn build_draft_case_with_empty_trajectory_yields_no_step_contract() {
        // A turn with no tool calls (plain text) carries no deterministic step
        // contract — build returns None for expected_steps so the caller can
        // skip rather than persist a vacuous case.
        let trace = LlmTraceRow {
            id: "t1".into(),
            session_id: None,
            conversation_id: None,
            model: "glm-4.6".into(),
            base_url: "https://x".into(),
            status_code: Some(200),
            error_kind: None,
            req_body: "{}".into(),
            resp_body: Some(r#"{"content":[{"type":"text","text":"hi"}]}"#.into()),
            latency_ms: None,
            input_tokens: None,
            output_tokens: None,
            ttfb_ms: None,
            stream_ms: None,
            created_at: "2026-07-02T00:00:00Z".into(),
        };
        let case = build_draft_case_from_trajectory(
            "empty", "agent", "hi", &[trace], None, None, "2026-07-02T00:00:01Z",
        );
        assert!(case.expected_steps_json.is_none());
    }
}
