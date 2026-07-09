//! Platform-enablement eval (P4): the 反刷分 hardest case — does enabling a DW
//! feature actually IMPROVE the agent's tool-choice, or does it inflate the
//! score without a verifiable causal chain? Runs the SAME case under two configs
//! (feature OFF baseline vs feature ON) and diffs the trajectories via L4
//! [`compare_paired`]. A gain is CLEAR only if the ON trajectory demonstrably
//! closed the gap to the expected contract; otherwise BRAKE (unattributed gain
//! = brake, not a win).
//!
//! The toggled feature is **installed skills** (a real DW capability): OFF run
//! registers no skills (`skill_filter = Some(&[])`), ON run registers all
//! (`None`). If skills help, the ON trajectory should pick up expected steps the
//! OFF run missed; if they don't, the verdict is NoChange (honest: no signal),
//! never a fabricated gain.
//!
//! Needs a live LLM (two real agent runs) — like `run_replay`, the driver
//! compiles and runs when a provider key is present; the pure verdict core
//! (`compare_paired`) is unit-tested in [`crate::eval::paired`]. No faking.

use serde::{Deserialize, Serialize};

use crate::db::DbState;
use crate::eval::cases::EvalCaseRow;
use crate::eval::extract;
use crate::eval::paired::{compare_paired, PairedOutcome};
use crate::eval::replay::{parse_steps, run_replay, ReplayInput};
use crate::eval::scoring::Matcher;
use crate::trace::db::list_traces_for_session;

/// Which DW feature the enablement run toggles. Only `skills` today; the enum
/// leaves room to add MCP toggles without reshaping the wire contract.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EnablementFeature {
    Skills,
}

/// The verdict on whether enabling a feature improved the agent. Mirrors L4's
/// `PairedVerdict` plus the two raw scores so the UI can show the delta.
#[derive(Debug, Clone, Serialize)]
pub struct EnablementVerdict {
    pub feature: EnablementFeature,
    /// off→on outcome (Improvement / Regression / NoChange).
    pub outcome: PairedOutcome,
    /// CLEAR = attributed gain (ON closed the gap to expected); BRAKE = gain
    /// without a verifiable chain; None = no gain to attribute.
    pub attribution: Option<String>,
    pub off_score: f64,
    pub on_score: f64,
    pub reason: String,
}

/// Drive one platform-enablement run for a case: replay with the feature OFF,
/// replay with it ON, diff the two trajectories via [`compare_paired`]. Needs a
/// live provider key (the agent really runs twice). Persists a
/// `gate = "enablement"` verdict tied to the case so the result shows in the L1
/// ledger. The OFF/ON trajectories are reconstructed from each run's LLM traces
/// (客观事实: rebuilt from real response bodies, not the agent's say-so).
pub async fn run_eval_enablement(
    db: &DbState,
    case: &EvalCaseRow,
    working_dir: String,
    model: Option<String>,
    matcher: Matcher,
) -> Result<EnablementVerdict, String> {
    let feature = EnablementFeature::Skills;

    // OFF run: skills disabled (empty allow-list). verdict_gate = None: this is
    // an experimental baseline probe, NOT the case's formal eval — its FAIL must
    // not pollute the case's eval history. Only the汇总 enablement verdict below
    // lands in the ledger (反刷分 ledger hygiene).
    let off_sid = uuid::Uuid::new_v4().to_string();
    let off = run_replay(
        ReplayInput {
            session_id: off_sid.clone(),
            working_dir: working_dir.clone(),
            model: model.clone(),
            enable_skills: false,
            verdict_gate: None,
        },
        case,
        matcher,
        db,
    )
    .await?;
    let off_actual = trajectory_for(db, &off_sid).await?;

    // ON run: skills enabled (all admitted). Same None — the汇总 verdict carries
    // the signal; the ON run's per-step verdict would just double-count.
    let on_sid = uuid::Uuid::new_v4().to_string();
    let on = run_replay(
        ReplayInput {
            session_id: on_sid.clone(),
            working_dir: working_dir.clone(),
            model: model.clone(),
            enable_skills: true,
            verdict_gate: None,
        },
        case,
        matcher,
        db,
    )
    .await?;
    let on_actual = trajectory_for(db, &on_sid).await?;

    let expected = parse_steps(case.expected_steps_json.as_deref());
    let paired = compare_paired(
        &off,
        &on,
        &off_actual,
        &on_actual,
        if expected.is_empty() { None } else { Some(&expected) },
    );

    // Persist an `enablement` verdict tied to the case (best-effort). The ON
    // session is the "after" run — anchor the row to it for traceability.
    let conn = db.get().map_err(|e| format!("db lock: {e}"))?;
    let outcome_wire = serde_json::to_value(paired.outcome)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| format!("{:?}", paired.outcome));
    let report = serde_json::json!({
        "feature": serde_json::to_value(feature).unwrap_or(serde_json::Value::Null),
        "off_score": off.score,
        "on_score": on.score,
        // Mirror the wire (snake_case) form, not Debug's PascalCase, so the
        // report JSON matches the EnablementVerdict.outcome the UI renders.
        "outcome": outcome_wire,
        "reason": paired.reason,
    });
    let row = crate::eval::verdicts::NewVerdict {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: Some(on_sid),
        case_id: Some(case.id.clone()),
        gate: "enablement".to_string(),
        verdict: outcome_wire,
        attribution: paired.attribution.clone(),
        report: serde_json::to_string(&report).ok(),
        commit_sha: case.commit_sha.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(e) = crate::eval::verdicts::insert_verdict(&conn, &row) {
        log::warn!("[enablement] verdict persist failed for case {}: {e}", case.id);
    }

    Ok(EnablementVerdict {
        feature,
        outcome: paired.outcome,
        attribution: paired.attribution,
        off_score: off.score,
        on_score: on.score,
        reason: paired.reason,
    })
}

/// Reconstruct a run's tool-call trajectory from its persisted LLM traces — the
/// same extraction `run_replay` uses internally, surfaced here so the enablement
/// driver can feed both trajectories to `compare_paired` without changing
/// `run_replay`'s return shape.
async fn trajectory_for(db: &DbState, session_id: &str) -> Result<Vec<extract::ToolStep>, String> {
    let conn = db.get().map_err(|e| format!("db lock: {e}"))?;
    let traces = list_traces_for_session(&conn, session_id)
        .map_err(|e| format!("list traces for {session_id}: {e}"))?;
    Ok(extract::extract_trajectory(&traces))
}
