//! B7 trajectory-eval Tauri commands — score a session's reconstructed
//! trajectory against an optional reference, persist the run, and read the
//! daily regression trend.

use tauri::State;

use crate::activity;
use crate::db::DbState;
use crate::error::AppError;
use crate::eval::db as eval_db;
use crate::eval::extract;
use crate::eval::scoring::{self, Grade, Matcher, RubricInput};
use crate::trace::db::list_traces_for_session;

use crate::eval::cases as eval_cases;
use crate::eval::verdicts as verdicts_db;
use serde::Deserialize;

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

// ===========================================================================
// L1 verdict ledger + L2 eval cases — the anti-gaming eval panel's data plane.
// Verdicts (L1) are written by executor run_gate / circuit-breaker / the replay
// scorer; cases (L2) are the deterministic contracts replay/paired run against.
// ===========================================================================

/// List verdicts (the L1 ledger), newest-first. Any of `session_id` / `gate` /
/// `case_id` may scope the query (AND-combined). The panel slices by gate kind
/// (verify / honesty / forge / circuit-breaker / eval) and by a replay run's
/// `case_id`.
#[tauri::command]
pub async fn list_verdicts(
    db: State<'_, DbState>,
    session_id: Option<String>,
    gate: Option<String>,
    case_id: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<verdicts_db::VerdictRow>, AppError> {
    let conn = db.get().map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    verdicts_db::list_verdicts(
        &conn,
        session_id.as_deref(),
        gate.as_deref(),
        case_id.as_deref(),
        limit.unwrap_or(100),
    )
}

/// List eval cases. Approved only by default (`draft` excluded) so the replay /
/// paired paths only see reviewed contracts; pass `include_drafts = true` for
/// the case-editor view that shows pending trajectory-frozen drafts.
#[tauri::command]
pub async fn list_eval_cases(
    db: State<'_, DbState>,
    category: Option<String>,
    include_drafts: Option<bool>,
    limit: Option<i64>,
) -> Result<Vec<eval_cases::EvalCaseRow>, AppError> {
    let conn = db.get().map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    eval_cases::list_eval_cases(
        &conn,
        category.as_deref(),
        include_drafts.unwrap_or(false),
        limit.unwrap_or(50),
    )
}

/// Fetch one eval case by id — the replay driver loads its contract this way.
#[tauri::command]
pub async fn get_eval_case(
    db: State<'_, DbState>,
    id: String,
) -> Result<Option<eval_cases::EvalCaseRow>, AppError> {
    let conn = db.get().map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    eval_cases::get_eval_case(&conn, &id)
}

/// Approve a draft case — the only path by which a trajectory-frozen case
/// becomes eligible to anchor a paired replay (反刷分: 防 agent 自我背书 — the
/// agent's own past run can seed a case, but cannot ratify one). Returns rows
/// touched (0 = not found / already approved).
#[tauri::command]
pub async fn approve_eval_case(
    db: State<'_, DbState>,
    id: String,
) -> Result<usize, AppError> {
    let conn = db.get().map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    eval_cases::approve_draft(&conn, &id)
}

/// Frontend input for [`update_eval_case`] — the editable contract fields.
/// `input_prompt` is intentionally absent (C1: 对话记录只读).
#[derive(Debug, Deserialize)]
pub struct UpdateCaseInput {
    pub name: String,
    pub category: String,
    pub expected_steps_json: Option<String>,
    pub expected_output: Option<String>,
    pub expected_observables_json: Option<String>,
    pub negative_json: Option<String>,
}

/// Update a case's editable contract fields. `input_prompt` is never touched
/// (C1: 对话记录只读); `draft` only flips via `approve_eval_case`. Returns rows
/// touched (0 = not found).
#[tauri::command]
pub async fn update_eval_case(
    db: State<'_, DbState>,
    id: String,
    input: UpdateCaseInput,
) -> Result<usize, AppError> {
    let conn = db.get().map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    let update = eval_cases::CaseUpdate {
        name: input.name,
        category: input.category,
        expected_steps_json: input.expected_steps_json,
        expected_output: input.expected_output,
        expected_observables_json: input.expected_observables_json,
        negative_json: input.negative_json,
    };
    eval_cases::update_eval_case(&conn, &id, &update)
}

/// Frontend input for [`create_eval_case`] — everything except id / created_at
/// (server-generated) and the draft flag (defaults to false: a hand-authored
/// case is already reviewed).
#[derive(Debug, Deserialize)]
pub struct CreateCaseInput {
    pub name: String,
    pub category: String,
    pub input_prompt: String,
    pub expected_steps_json: Option<String>,
    pub expected_output: Option<String>,
    pub expected_observables_json: Option<String>,
    pub negative_json: Option<String>,
    pub source_session_id: Option<String>,
    pub commit_sha: Option<String>,
    /// Override the draft flag (default false). True only when seeding an
    /// un-reviewed case from a trajectory.
    pub draft: Option<bool>,
}

/// Create an eval case. Defaults to `draft = false` (a hand-authored case is
/// reviewed); pass `draft = true` to seed an un-reviewed case. Returns the new
/// case id. (The in-backend trajectory-seeding path is
/// `eval::cases::build_draft_case_from_trajectory`.)
#[tauri::command]
pub async fn create_eval_case(
    db: State<'_, DbState>,
    input: CreateCaseInput,
) -> Result<String, AppError> {
    let id = uuid::Uuid::new_v4().to_string();
    let row = eval_cases::NewEvalCase {
        id: id.clone(),
        name: input.name,
        category: input.category,
        input_prompt: input.input_prompt,
        expected_steps_json: input.expected_steps_json,
        expected_output: input.expected_output,
        expected_observables_json: input.expected_observables_json,
        negative_json: input.negative_json,
        source_session_id: input.source_session_id,
        commit_sha: input.commit_sha,
        draft: input.draft.unwrap_or(false),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let conn = db.get().map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    eval_cases::insert_eval_case(&conn, &row)?;
    Ok(id)
}

// ===========================================================================
// L3 replay trigger — the frontend's "运行回放" entry point. Loads a case,
// builds a Plan-mode ReactAgent (read-only sandbox), runs it one turn against
// the case's frozen `input_prompt`, reconstructs the trajectory from real LLM
// traces, scores it (反刷分 #1: pure sequence match, no LLM self-grading), and
// persists an `eval` verdict tied to the case. Needs a live provider key — the
// agent really runs.
// ===========================================================================

/// Drive one L3 replay run for a case. Returns the scored verdict (also
/// persisted as a `gate = "eval"` verdict row with the case_id set, so it shows
/// up in `list_verdicts`). The frontend's P4 "启动评测" / P5 single-replay view
/// both call this.
#[tauri::command]
pub async fn run_eval_replay(
    db: State<'_, DbState>,
    case_id: String,
    working_dir: String,
    model: Option<String>,
    matcher: Matcher,
) -> Result<crate::eval::replay::ReplayVerdict, AppError> {
    let conn = db.get().map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    let case = eval_cases::get_eval_case(&conn, &case_id)?
        .ok_or_else(|| AppError::NotFound(format!("eval case not found: {case_id}")))?;
    drop(conn);
    // Fresh replay session id — its LLM traces land under this id, and the
    // persisted verdict carries it for traceability.
    let session_id = uuid::Uuid::new_v4().to_string();
    let input = crate::eval::replay::ReplayInput {
        session_id,
        working_dir,
        model,
        // A plain agent replay admits installed skills (the default). The
        // platform-enablement eval toggles this off for its baseline run.
        enable_skills: true,
        // Persist the verdict under the `eval` gate so it lands in the case's
        // formal eval history (the default; enablement probes pass None).
        verdict_gate: Some("eval".to_string()),
    };
    crate::eval::replay::run_replay(input, &case, matcher, db.inner())
        .await
        .map_err(AppError::Internal)
}

/// Preview a session's reconstructed tool-call trajectory WITHOUT persisting —
/// the P3 "会话 → Case" wizard shows this so the user can curate the expected
/// steps before saving a draft case, and A1 renders the span tree from it.
/// Deterministic extraction (`extract_full` over real LLM traces + the
/// session's recorded file diff), no LLM, no write — so what the user sees is
/// exactly what a future replay would score against (反刷分 #1: the contract is
/// frozen from objective trace data, not the agent's say-so). Returns the rich
/// trajectory: steps + files changed + token usage + estimated cost + the span
/// tree (LLM call = parent, tool calls = children).
#[tauri::command]
pub async fn preview_session_trajectory(
    db: State<'_, DbState>,
    session_id: String,
) -> Result<extract::FullTrajectory, AppError> {
    let conn = db.get().map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    let traces = list_traces_for_session(&conn, &session_id)?;
    let files = activity::files_changed_for_session(&conn, &session_id)?;
    Ok(extract::extract_full(&traces, &files))
}

/// Score a session against its case contract on the 8-dimension AgentX
/// reliability rubric (P6). Assembles [`RubricInput`] entirely from
/// already-recorded facts: LLM traces → actual tool sequence + failed-step
/// count, the activity log → files the run touched, the case contract →
/// expected steps / negative (forbidden tools) / expected observables (file
/// subset), the verdict ledger → whether any human-gate approval fired. Returns
/// the weighted `Q_code` + per-dimension breakdown. Pure + deterministic
/// (反刷分 #1: no LLM judges its own reliability here).
///
/// `had_human_intervention` is read from the L1 ledger: any `gate =
/// "human-gate"` verdict (approve/reject/retry, persisted by
/// `resolve_human_gate_cmd`) for the session counts as an intervention and
/// trips the hard gate. A replay run (Plan sandbox, no human gate) has none, so
/// it scores `false` there — the honest "no human helped".
#[tauri::command]
pub async fn score_eval_rubric(
    db: State<'_, DbState>,
    session_id: String,
    case_id: String,
    matcher: Matcher,
) -> Result<scoring::RubricScore, AppError> {
    let conn = db.get().map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    let case = eval_cases::get_eval_case(&conn, &case_id)?
        .ok_or_else(|| AppError::NotFound(format!("eval case not found: {case_id}")))?;
    let traces = list_traces_for_session(&conn, &session_id)?;
    let steps = extract::extract_trajectory(&traces);
    let actual: Vec<&str> = extract::ToolStep::name_refs(&steps);

    let actual_files = activity::files_changed_for_session(&conn, &session_id)?;
    let actual_files_refs: Vec<&str> = actual_files.iter().map(String::as_str).collect();

    // expected_steps_json / negative_json are persisted as [{"name":"Read"}]
    // (ToolStep-shaped) by the P3 wizard, but a hand-authored ["Read"] is also
    // valid — parse_names takes both. expected_observables_json may carry file
    // paths as a string array; prose (non-JSON) → empty → file_change dim is
    // vacuously 1.0 (no file contract to check), the honest "unconstrained".
    let expected = parse_names(case.expected_steps_json.as_deref());
    let expected_refs: Option<Vec<&str>> = if expected.is_empty() {
        None
    } else {
        Some(expected.iter().map(String::as_str).collect())
    };
    let negative = parse_names(case.negative_json.as_deref());
    let negative_refs: Vec<&str> = negative.iter().map(String::as_str).collect();
    let expected_files = parse_names(case.expected_observables_json.as_deref());
    let expected_files_refs: Vec<&str> = expected_files.iter().map(String::as_str).collect();

    let failed_steps = steps
        .iter()
        .filter(|s| s.status.as_deref() == Some("error"))
        .count();

    // manual_intervention hard gate, scored from FACT: any human-gate verdict
    // (approve/reject/retry) persisted for this session counts as an
    // intervention — the run needed a human nudge. A replay (Plan sandbox, no
    // human gate) has none, so it stays false there; a real interactive session
    // in HumanGate mode records each decision via resolve_human_gate_cmd.
    let had_human_intervention = !verdicts_db::list_verdicts(
        &conn,
        Some(&session_id),
        Some("human-gate"),
        None,
        1,
    )?
    .is_empty();

    let input = RubricInput {
        actual: &actual,
        expected: expected_refs.as_deref(),
        matcher,
        negative: &negative_refs,
        expected_files: &expected_files_refs,
        actual_files: &actual_files_refs,
        failed_steps,
        had_human_intervention,
    };
    Ok(scoring::score_rubric(input))
}

/// Parse a JSON array of names that may be either `["read"]` (strings) or
/// `[{"name":"read"}]` (ToolStep-shaped objects). Non-array / unparseable /
/// null → empty. Used for expected_steps / negative / observables, which
/// hand-authoring and the P3 wizard emit in different shapes.
fn parse_names(json: Option<&str>) -> Vec<String> {
    let Some(s) = json else {
        return Vec::new();
    };
    let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(s) else {
        return Vec::new();
    };
    arr.into_iter()
        .filter_map(|v| {
            if let Some(s) = v.as_str() {
                Some(s.to_string())
            } else {
                v.get("name").and_then(|n| n.as_str()).map(String::from)
            }
        })
        .collect()
}

// ===========================================================================
// P4 platform-mechanism eval — exercise the kernel-compose DAG engine itself
// (routing / gate / skip / fail behavior) with a deterministic stub executor.
// No LLM: the verdict is a fact about the engine's GraphEvent sequence.
// ===========================================================================

/// Run a platform-mechanism case: compile the YAML workflow, drive it with a
/// stub executor (agent echoes input / gate passes), and compare the observed
/// node-start order + terminal outcome against `expect`. The P4 "平台-机制"
/// object calls this — closing the gap where the 3 platform objects were
/// gap-noted. Returns the verdict (also surfaced in the panel; a persisted
/// verdict variant is a follow-on).
#[tauri::command]
pub async fn eval_platform_mechanism(
    graph_yaml: String,
    input_json: serde_json::Value,
    expect: crate::eval::platform::MechanismExpect,
) -> Result<crate::eval::platform::MechanismVerdict, AppError> {
    crate::eval::platform::run_platform_mechanism(&graph_yaml, input_json, expect)
        .await
        .map_err(AppError::Internal)
}

// ===========================================================================
// P4 platform-e2e eval — exercise the full eval data-plane (persistence logic →
// in-memory DB → return shape) against a seeded database. No LLM, no browser:
// the verdict is a fact about the data contracts the frontend consumes. The
// browser-render layer is guarded by the playwright eval.spec.ts suite.
// ===========================================================================

/// Run a platform-e2e case: stand up an in-memory DB with the real eval schema,
/// seed it, and assert each set expectation (draft filtering, replay grade,
// verdict gate-count) against the real persistence/logic functions. Deterministic
/// — no LLM, no provider, no browser. The P4 "平台-e2e" object calls this.
#[tauri::command]
pub async fn eval_platform_e2e(
    seed: crate::eval::platform_e2e::E2ESeed,
    expect: crate::eval::platform_e2e::E2EExpect,
) -> Result<crate::eval::platform_e2e::E2EVerdict, AppError> {
    crate::eval::platform_e2e::run_platform_e2e(seed, expect)
        .map_err(AppError::Internal)
}

// ===========================================================================
// P4 platform-enablement eval — does enabling a DW feature (skills) actually
// improve the agent's tool-choice? Runs the case twice (feature OFF vs ON) and
// diffs the trajectories via L4 compare_paired. Needs a live provider key (two
// real agent runs); the verdict core is compare_paired (unit-tested). 反刷分:
// a gain that doesn't close the gap to expected is BRAKE, not CLEAR.
// ===========================================================================

/// Drive one platform-enablement run: replay with skills OFF, replay with skills
/// ON, diff via compare_paired, persist a `gate = "enablement"` verdict. Needs a
/// live provider key (the agent really runs twice). The P4 "平台-加持" object.
#[tauri::command]
pub async fn run_eval_enablement(
    db: State<'_, DbState>,
    case_id: String,
    working_dir: String,
    model: Option<String>,
    matcher: Matcher,
) -> Result<crate::eval::platform_enablement::EnablementVerdict, AppError> {
    let conn = db.get().map_err(|e| AppError::Internal(format!("db lock: {e}")))?;
    let case = eval_cases::get_eval_case(&conn, &case_id)?
        .ok_or_else(|| AppError::NotFound(format!("eval case not found: {case_id}")))?;
    drop(conn);
    crate::eval::platform_enablement::run_eval_enablement(
        db.inner(),
        &case,
        working_dir,
        model,
        matcher,
    )
    .await
    .map_err(AppError::Internal)
}
