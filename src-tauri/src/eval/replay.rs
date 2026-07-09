//! L3 replay — drive a fresh ReactAgent against an eval case's `input_prompt`
//! in a read-only sandbox, collect the resulting tool-call trajectory, score it
//! against the case's frozen `expected_steps`, and persist a verdict tied to the
//! case (`case_id`). This is the deterministic core of the eval panel: a case is
//! a fixed contract, the agent re-runs against it, and the score is a pure
//! sequence match — not an LLM's opinion of its own output.
//!
//! 反刷分三原则 mapped here:
//! - **客观事实代码判** — `score_replay` is a pure function over tool-name
//!   sequences (`scoring::score`); no LLM judges the trajectory. The LLM only
//!   *runs* as the agent under test, it never grades itself.
//! - **因果归因** — a PASS carries `attribution = CLEAR` only when the
//!   trajectory actually matches the contract; a PASS by luck (or by a forbidden
//!   shortcut) is downgraded below.
//! - **配对回放** — `run_replay` is the single-step primitive; L4 paired
//!   comparison calls it twice (old vs new platform version) and diffs.
//!
//! Safety fence: replay runs the agent in `PermissionMode::Plan`, which blocks
//! Bash/Write at the hook layer while allowing Read/Glob/Grep. So an agent under
//! evaluation CANNOT alter the workspace — its score reflects tool-selection,
//! not side-effects it landed. Exec-output quality is the L1 gate verdicts' job
//! (verify/honesty/forge), not replay's.

use serde::{Deserialize, Serialize};

use crate::eval::extract::ToolStep;
use crate::eval::scoring::{score, EvalScore, Grade, Matcher};

/// The verdict for one replayed case — what the replay driver persists as a
/// `gate = "eval"` verdict row (with `case_id` set), and what L4 paired-replay
/// diffs between two platform versions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplayVerdict {
    /// Numeric trajectory score in `[0.0, 1.0]` (0.0 when a negative is hit).
    pub score: f64,
    /// Three-state rubric grade. `Incorrect` on any negative counter-example.
    pub grade: Grade,
    /// Coarse ledger verdict: PASS for optimal/suboptimal, FAIL for incorrect
    /// or negative-hit.
    pub verdict: String,
    /// Anti-gaming attribution (反刷分): CLEAR only on a clean PASS; None
    /// otherwise (FAIL is the brake signal, not a gain needing attribution).
    pub attribution: Option<String>,
    /// Did the trajectory hit any step listed in the case's `negative_json`?
    /// A hit OVERRULES the score → FAIL, the guard against right-steps-but-
    /// wrong-outcome刷分 (e.g. used Bash to force a result a Read would have
    /// answered).
    pub negative_violated: bool,
    /// Human-readable reason string — goes into the verdict `report` so a FAIL
    /// is self-explaining in the UI (why it failed: negative hit vs missing step).
    pub reason: String,
}

/// Score a replayed trajectory against a case's deterministic contract.
///
/// Pure function — the entire反刷分 #1 (客观事实代码判) lives here. Inputs are
/// the agent's actual tool-call sequence, the case's frozen expected sequence
/// (None/empty = no step contract, score the trajectory on its own), the matcher
/// strictness, and the negative counter-examples (steps that must NOT appear).
///
/// Scoring order (matters):
/// 1. **Negative hit → instant FAIL.** A forbidden step appearing anywhere in the
///    actual trajectory makes the whole replay Incorrect, regardless of how well
///    the rest matches. This is the anti-gaming guard: an agent that reaches the
///    "right" tools but via a forbidden shortcut (Bash instead of Read) must not
///    score well.
/// 2. **Otherwise** delegate to `scoring::score` (exact / in-order / any-order).
/// 3. **Attribution**: a PASS (optimal/suboptimal) is CLEAR — the trajectory
///    itself is the verifiable causal chain. A FAIL leaves attribution None —
///    FAIL is itself the brake, not a gain to attribute. (BRAKE is reserved for
///    L4: an unattributed gain of the NEW version over the old.)
pub fn score_replay(
    actual: &[ToolStep],
    expected: Option<&[ToolStep]>,
    matcher: Matcher,
    negative: &[ToolStep],
) -> ReplayVerdict {
    let actual_names = ToolStep::name_refs(actual);
    let neg_names: Vec<&str> = negative.iter().map(|n| n.name.as_str()).collect();

    // #1 anti-gaming: a negative counter-example hit OVERRULES everything.
    let hit = neg_names
        .iter()
        .find(|n| actual_names.contains(n))
        .copied();
    if let Some(bad) = hit {
        return ReplayVerdict {
            score: 0.0,
            grade: Grade::Incorrect,
            verdict: "FAIL".into(),
            attribution: None,
            negative_violated: true,
            reason: format!(
                "negative counter-example hit: agent used forbidden step '{bad}' \
                 (anti-gaming: right-steps-wrong-outcome guard)"
            ),
        };
    }

    // #2 objective trajectory match — pure sequence comparison, no LLM.
    let expected_names: Option<Vec<&str>> = expected.map(ToolStep::name_refs);
    let ref_slice: Option<&[&str]> = expected_names.as_deref();
    let EvalScore { score: s, grade } = score(&actual_names, ref_slice, matcher);

    // #3 attribution: CLEAR only on a clean PASS.
    let (verdict, attribution) = match grade {
        Grade::Optimal | Grade::Suboptimal => ("PASS", Some("CLEAR")),
        Grade::Incorrect => ("FAIL", None),
    };
    ReplayVerdict {
        score: s,
        grade,
        verdict: verdict.into(),
        attribution: attribution.map(String::from),
        negative_violated: false,
        reason: format!("trajectory grade {grade:?} (matcher {matcher:?})"),
    }
}

/// Decode a case's `expected_steps_json` / `negative_json` into typed steps.
/// Malformed JSON yields an empty vec (defensive — a corrupt row never panics
/// the replay; it just scores as "no contract" / "no negatives").
pub fn parse_steps(json: Option<&str>) -> Vec<ToolStep> {
    json.and_then(|s| serde_json::from_str::<Vec<ToolStep>>(s).ok())
        .unwrap_or_default()
}

/// Input for [`run_replay`] — the case contract + where/how to run it. The
/// driver builds a Plan-mode ReactAgent (read-only sandbox), runs one turn
/// against `case.input_prompt`, pulls the session's LLM traces, extracts the
/// trajectory, scores it, and persists an `eval` verdict tied to `case.id`.
///
/// NOTE: `run_replay` itself needs a live LLM + a built agent, so it is not
/// unit-testable in-process (the app_lib test binary hits the pre-existing
/// 0xc0000139 GUI-DLL load failure on Windows; the react loop needs a real
/// provider key). Its deterministic core — [`score_replay`] — IS unit-tested
/// below; `run_replay` is verified via an example bin / CI mac-linux (the same
/// pattern used by build_react_agent's live-wiring smoke test).
pub struct ReplayInput {
    /// The replay session id — verdicts get `session_id = Some(this)` so a
    /// replay run is traceable back to its session (and its traces).
    pub session_id: String,
    /// The working directory under test. Read-only during replay (Plan mode);
    /// the agent explores but cannot alter it.
    pub working_dir: String,
    /// Model id to run under (None = the user's configured default).
    pub model: Option<String>,
    /// Whether installed skills are admitted for this run. The platform-
    /// enablement eval toggles this OFF vs ON and diffs the trajectories; a
    /// plain agent replay leaves it `true` (the default — skills available).
    pub enable_skills: bool,
    /// Which gate label to persist the verdict under, or `None` to skip the
    /// verdict write entirely. A normal replay run uses `Some("eval")` so the
    /// verdict lands in the case's eval history. The platform-enablement driver
    /// passes `None` for both its OFF/ON runs — those are experimental probes,
    /// not the case's formal eval; only the single汇总 `enablement` verdict it
    /// writes itself should land in the ledger (反刷分 ledger hygiene: don't let
    /// a baseline-probe FAIL pollute the case's real eval PASS/FAIL signal).
    pub verdict_gate: Option<String>,
}

/// Drive one replay turn end-to-end:
/// 1. build a Plan-mode ReactAgent (read-only sandbox — Bash/Write blocked at
///    the hook layer, Read/Glob/Grep allowed),
/// 2. run it one turn against the case's `input_prompt`,
/// 3. pull the session's LLM traces → reconstruct the trajectory (客观事实,
///    rebuilt from real response bodies, not the agent's self-report),
/// 4. [`score_replay`] against the case's frozen contract,
/// 5. persist an `eval` verdict tied to `case.id`.
///
/// Needs a live LLM (the agent really runs) + a tokio runtime, so it is NOT
/// unit-testable in-process (the app_lib test binary hits 0xc0000139; the react
/// loop needs a real provider key). The deterministic core — [`score_replay`] —
/// IS unit-tested; this driver is verified via an example bin / CI mac-linux,
/// the same pattern as `build_react_agent`'s live-wiring smoke test.
pub async fn run_replay(
    input: ReplayInput,
    case: &crate::eval::cases::EvalCaseRow,
    matcher: crate::eval::scoring::Matcher,
    db: &crate::db::DbState,
) -> Result<ReplayVerdict, String> {
    use futures::StreamExt;
    // `agent.run` is the `kernel_core::Agent` trait method — needs the trait
    // in scope (ReactAgent impls it, but the method isn't inherent).
    use kernel_core::Agent;

    // 1. Plan-mode agent — the workspace is fenced, the agent can't alter it.
    //    No MCP: replay isolates the agent down to its built-in
    //    read tools + (optionally) skills so the trajectory is deterministic
    //    w.r.t. the install, not whatever servers happened to be enabled. The
    //    platform-enablement eval flips `enable_skills` to measure the skill
    //    feature's effect on the trajectory.
    let skill_filter: Option<&[String]> = if input.enable_skills {
        None // all installed skills admitted
    } else {
        Some(&[]) // empty allow-list → no skills registered
    };
    let agent = crate::kernel_impl::executor::build_react_agent(
        input.model.as_deref(),
        None,                       // mcp — isolated for determinism
        &input.working_dir,
        None,                       // conversation_id — ad-hoc replay turn
        Vec::new(),                 // history — fresh single turn
        Some(db.clone()),
        crate::kernel_impl::hooks::PermissionMode::Plan,
        None,                       // task_ref
        Some(input.session_id.as_str()),
        skill_filter,               // skill_filter — toggled by enable_skills
        None,                       // mcp_filter
        None,                       // app — single-agent replay, no compaction archive
        None,                       // compaction_blocks
        None,                       // approval
    )?;

    // 2. Run one turn against the case's frozen input prompt.
    let agent_input = kernel_core::AgentInput {
        prompt: case.input_prompt.clone(),
        working_dir: Some(input.working_dir.clone()),
        model: None,
        resume_from: None,
    };
    let mut stream = agent.run(agent_input).map_err(|e| format!("{e}"))?;
    // Drain to completion — events are discarded; we only need the run to FINISH
    // so its LLM calls land in llm_traces under session_id for step 3.
    while let Some(_ev) = stream.next().await {}

    // 3. Reconstruct the trajectory from the session's real response bodies.
    let conn = db.get().map_err(|e| format!("db lock: {e}"))?;
    let traces = crate::trace::db::list_traces_for_session(&conn, &input.session_id)
        .map_err(|e| format!("list traces: {e}"))?;
    let actual = crate::eval::extract::extract_trajectory(&traces);

    // 4. Score against the case's deterministic contract (反刷分 #1).
    let expected = parse_steps(case.expected_steps_json.as_deref());
    let negative = parse_steps(case.negative_json.as_deref());
    let verdict = score_replay(&actual, Some(&expected), matcher, &negative);

    // 5. Persist an `eval` verdict tied to the case. Best-effort — a DB write
    //    failure is logged, never blocks returning the verdict. Skipped entirely
    //    when `verdict_gate` is None (platform-enablement's OFF/ON probes — those
    //    are not the case's formal eval run, so they must NOT land in its eval
    //    history; the enablement driver writes its own single汇总 verdict).
    if let Some(gate) = &input.verdict_gate {
        let report = serde_json::json!({
            "score": verdict.score,
            "grade": format!("{:?}", verdict.grade),
            "reason": verdict.reason,
            "negative_violated": verdict.negative_violated,
            "matcher": format!("{:?}", matcher),
            "actual_steps": actual.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
        });
        let row = crate::eval::verdicts::NewVerdict {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: Some(input.session_id.clone()),
            case_id: Some(case.id.clone()),
            gate: gate.clone(),
            verdict: verdict.verdict.clone(),
            attribution: verdict.attribution.clone(),
            report: serde_json::to_string(&report).ok(),
            // Anchor to the case's version (the contract under test), not the
            // replay run's incidental HEAD.
            commit_sha: case.commit_sha.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        if let Err(e) = crate::eval::verdicts::insert_verdict(&conn, &row) {
            log::warn!("[replay] verdict persist failed for case {}: {e}", case.id);
        }
    }

    Ok(verdict)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(name: &str) -> ToolStep {
        ToolStep {
            name: name.into(),
            status: None,
        }
    }
    fn steps(names: &[&str]) -> Vec<ToolStep> {
        names.iter().map(|n| step(n)).collect()
    }

    #[test]
    fn exact_match_optimal_is_pass_clear() {
        // Actual == expected, element for element → Optimal, PASS, CLEAR.
        let v = score_replay(
            &steps(&["read", "grep", "edit"]),
            Some(&steps(&["read", "grep", "edit"])),
            Matcher::ExactMatch,
            &[],
        );
        assert_eq!(v.verdict, "PASS");
        assert_eq!(v.attribution.as_deref(), Some("CLEAR"));
        assert_eq!(v.grade, Grade::Optimal);
        assert!((v.score - 1.0).abs() < 1e-9);
        assert!(!v.negative_violated);
    }

    #[test]
    fn missing_step_is_incorrect_fail_no_attribution() {
        // Agent skipped 'grep' → Incorrect under ExactMatch, FAIL, no attribution.
        let v = score_replay(
            &steps(&["read", "edit"]),
            Some(&steps(&["read", "grep", "edit"])),
            Matcher::ExactMatch,
            &[],
        );
        assert_eq!(v.verdict, "FAIL");
        assert_eq!(v.grade, Grade::Incorrect);
        assert!(v.attribution.is_none(), "FAIL carries no attribution");
    }

    #[test]
    fn negative_hit_overrules_a_passing_score() {
        // 反刷分 guard: trajectory matches expected (would be Optimal/PASS), BUT
        // the agent used 'bash' which the case lists as a forbidden shortcut.
        // The negative hit OVERRULES → FAIL, score 0, Incorrect — no matter how
        // good the rest of the trajectory looks.
        let v = score_replay(
            &steps(&["read", "bash", "grep"]),
            Some(&steps(&["read", "grep"])),
            Matcher::InOrder,
            &steps(&["bash"]),
        );
        assert_eq!(v.verdict, "FAIL", "negative hit must overrule the match");
        assert_eq!(v.grade, Grade::Incorrect);
        assert!((v.score - 0.0).abs() < 1e-9);
        assert!(v.negative_violated);
        assert!(v.reason.contains("bash"), "reason names the forbidden step");
    }

    #[test]
    fn negative_not_hit_does_not_affect_a_clean_pass() {
        // 'write' is forbidden and absent → the negative is a no-op; the clean
        // match stands as PASS/CLEAR.
        let v = score_replay(
            &steps(&["read", "grep"]),
            Some(&steps(&["read", "grep"])),
            Matcher::ExactMatch,
            &steps(&["write", "bash"]),
        );
        assert_eq!(v.verdict, "PASS");
        assert_eq!(v.attribution.as_deref(), Some("CLEAR"));
        assert!(!v.negative_violated);
    }

    #[test]
    fn any_order_tolerates_extra_redundant_steps_as_suboptimal() {
        // Expected tools all present but with an extra redundant call → under
        // AnyOrder that's Suboptimal (correct tools, redundant steps), still PASS.
        let v = score_replay(
            &steps(&["read", "read", "grep"]),
            Some(&steps(&["read", "grep"])),
            Matcher::AnyOrder,
            &[],
        );
        assert_eq!(v.grade, Grade::Suboptimal);
        assert_eq!(v.verdict, "PASS", "suboptimal is still a pass");
        assert_eq!(v.attribution.as_deref(), Some("CLEAR"));
    }

    #[test]
    fn no_expected_contract_scores_trajectory_only_and_never_negatives() {
        // expected=None: no step contract to match, so the grade comes from the
        // bare trajectory (None reference → scoring treats it as a pass-by-
        // having-run). A negative hit still fires.
        let v = score_replay(&steps(&["read", "grep"]), None, Matcher::ExactMatch, &[]);
        assert_ne!(v.verdict, "FAIL", "no negative + ran → not a fail");
        // But a negative still overrules even with no contract:
        let v2 = score_replay(&steps(&["read", "bash"]), None, Matcher::ExactMatch, &steps(&["bash"]));
        assert_eq!(v2.verdict, "FAIL");
        assert!(v2.negative_violated);
    }

    #[test]
    fn parse_steps_round_trips_and_is_defensive() {
        // Valid JSON → decoded steps.
        let s = parse_steps(Some(r#"[{"name":"read"},{"name":"grep"}]"#));
        assert_eq!(s.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["read", "grep"]);
        // Malformed JSON → empty (never panics).
        assert!(parse_steps(Some("not json")).is_empty());
        // None → empty.
        assert!(parse_steps(None).is_empty());
    }
}
