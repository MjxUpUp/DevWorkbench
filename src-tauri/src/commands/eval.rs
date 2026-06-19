//! B7 trajectory-eval Tauri commands — score a session's reconstructed
//! trajectory against an optional reference, persist the run, and read the
//! daily regression trend.

use tauri::State;

use crate::db::DbState;
use crate::error::AppError;
use crate::eval::db as eval_db;
use crate::eval::extract;
use crate::eval::scoring::{self, Grade, Matcher};
use crate::trace::db::list_traces_for_session;

fn matcher_str(m: Matcher) -> &'static str {
    match m {
        Matcher::ExactMatch => "exact_match",
        Matcher::InOrder => "in_order",
        Matcher::AnyOrder => "any_order",
    }
}

fn grade_str(g: Grade) -> &'static str {
    match g {
        Grade::Optimal => "optimal",
        Grade::Suboptimal => "suboptimal",
        Grade::Incorrect => "incorrect",
    }
}

/// Score a session's reconstructed tool-call trajectory against an optional
/// reference (`matcher` controls strictness) and persist the run. An empty
/// trajectory (a session that made no traced tool calls) still scores —
/// `Incorrect` at 0.0 — and is recorded, so the trend curve reflects real
/// coverage rather than silently dropping no-op sessions. Returns the stored
/// row.
#[tauri::command]
pub async fn eval_run_session(
    db: State<'_, DbState>,
    session_id: String,
    reference: Option<Vec<String>>,
    matcher: Matcher,
) -> Result<eval_db::EvalRunRow, AppError> {
    let conn = db.get().map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    let traces = list_traces_for_session(&conn, &session_id)?;
    let steps = extract::extract_trajectory(&traces);
    let actual = extract::ToolStep::name_refs(&steps);
    let reference_refs: Option<Vec<&str>> = reference
        .as_ref()
        .map(|v| v.iter().map(String::as_str).collect());
    let evaluated = scoring::score(&actual, reference_refs.as_deref(), matcher);

    let trajectory_json = serde_json::to_string(&actual).ok();
    let reference_json = reference
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_default());

    let conversation_id = traces.first().and_then(|t| t.conversation_id.clone());
    let created_at = chrono::Utc::now().to_rfc3339();
    let matcher_s = matcher_str(matcher).to_string();
    let grade_s = grade_str(evaluated.grade).to_string();
    let steps_n = steps.len() as i64;
    let row = eval_db::NewEvalRun {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: Some(session_id.clone()),
        conversation_id: conversation_id.clone(),
        matcher: matcher_s.clone(),
        score: evaluated.score,
        grade: grade_s.clone(),
        steps: steps_n,
        trajectory_json,
        reference_json,
        created_at: created_at.clone(),
    };
    eval_db::insert_eval_run(&conn, &row)?;

    Ok(eval_db::EvalRunRow {
        id: row.id,
        session_id: Some(session_id),
        conversation_id,
        matcher: matcher_s,
        score: evaluated.score,
        grade: grade_s,
        steps: steps_n,
        created_at,
    })
}

/// List eval runs, newest-first. Scope to a session when `session_id` is set.
#[tauri::command]
pub async fn list_eval_runs(
    db: State<'_, DbState>,
    session_id: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<eval_db::EvalRunRow>, AppError> {
    let conn = db.get().map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    eval_db::list_eval_runs(&conn, session_id.as_deref(), limit.unwrap_or(50))
}

/// Daily regression curve over the last `days` days (default 30). Buckets by
/// UTC date, ASC, so the chart reads left-to-right.
#[tauri::command]
pub async fn eval_trend(
    db: State<'_, DbState>,
    days: Option<i64>,
) -> Result<Vec<eval_db::TrendPoint>, AppError> {
    let conn = db.get().map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    eval_db::eval_trend(&conn, days.unwrap_or(30))
}
