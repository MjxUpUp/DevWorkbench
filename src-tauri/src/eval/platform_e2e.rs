//! Platform-e2e eval (P4): exercise the full eval DATA-PLANE — real persistence
//! logic → in-memory DB → return shape — against a seeded database, asserting
//! the objective facts the frontend renders (draft filtering, replay scoring,
//! verdict-ledger gate queries). No LLM, no browser: the verdict is a fact
//! about the persistence + logic + wire-contract layer (反刷分 #1: 客观事实).
//!
//! What this is and isn't: it closes the loop the IPC layer sits on — the SAME
//! `list_eval_cases` / `score_replay` / `list_verdicts` functions the Tauri
//! commands wrap and the frontend consumes, run against a real (in-memory)
//! SQLite with the real migrated schema. It does NOT spin up a browser; the
//! browser-render layer that consumes this data is guarded by the playwright
//! `eval.spec.ts` suite, not duplicated here. A platform-e2e PASS therefore
//! means: the data the frontend is wired to is shaped and filtered correctly
//! end-to-end through the persistence layer.
//!
//! Distinct from `platform` (engine mechanics) and agent eval (LLM trajectory):
//! here the platform's own data contracts are the subject, with the agent
//! stubbed out (a frozen `actual_steps` the case ships).

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::eval::cases::{insert_eval_case, list_eval_cases, NewEvalCase};
use crate::eval::extract::ToolStep;
use crate::eval::replay::{parse_steps, score_replay};
use crate::eval::scoring::{Grade, Matcher};
use crate::eval::verdicts::{insert_verdict, list_verdicts, NewVerdict};
use crate::migrate::{migrate_v20_to_v21, migrate_v21_to_v22};

/// A case row to seed into `eval_cases` for the run (the contract under test).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct E2ESeedCase {
    pub id: String,
    pub name: String,
    pub category: String,
    pub input_prompt: String,
    #[serde(default)]
    pub expected_steps_json: Option<String>,
    #[serde(default)]
    pub negative_json: Option<String>,
    /// draft=true seeds an un-reviewed case (must be excluded from the approved
    /// list, present in include_drafts).
    #[serde(default)]
    pub draft: bool,
}

/// A verdict row to seed into the ledger (for the gate-count check).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct E2ESeedVerdict {
    pub gate: String,
    pub verdict: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub case_id: Option<String>,
}

/// What gets seeded before the assertions run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct E2ESeed {
    #[serde(default)]
    pub cases: Vec<E2ESeedCase>,
    #[serde(default)]
    pub verdicts: Vec<E2ESeedVerdict>,
}

/// A replay-scoring assertion: load case `case_id`'s frozen contract from the
/// seeded DB, score the stubbed `actual_steps` against it, and require the
/// resulting grade. Exercises score_replay wired to a DB-loaded contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct E2EReplayExpect {
    pub case_id: String,
    pub actual_steps: Vec<String>,
    #[serde(default = "default_matcher")]
    pub matcher: Matcher,
    pub expected_grade: Grade,
}

fn default_matcher() -> Matcher {
    Matcher::ExactMatch
}

/// The deterministic contract a platform-e2e case asserts. Each `Option` is a
/// "don't care" when None — only the set fields are checked.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct E2EExpect {
    /// `list_eval_cases(include_drafts=false)` length (approved-only count).
    #[serde(default)]
    pub approved_case_count: Option<usize>,
    /// `list_eval_cases(include_drafts=true)` length (total seeded count).
    #[serde(default)]
    pub total_case_count: Option<usize>,
    /// Count verdicts for this gate (`list_verdicts(gate=…)` length).
    #[serde(default)]
    pub verdict_count_for_gate: Option<(String, usize)>,
    /// A replay-scoring assertion (score_replay grade on a stubbed trajectory).
    #[serde(default)]
    pub replay: Option<E2EReplayExpect>,
}

/// One check's outcome — surfaced so the UI can show which contract dimension
/// passed/failed (not just an opaque pass bool).
#[derive(Debug, Clone, Serialize)]
pub struct E2ECheck {
    pub name: String,
    pub pass: bool,
    pub detail: String,
}

/// The verdict a platform-e2e run produces. `pass` requires every set
/// expectation to match the data plane's observed behavior.
#[derive(Debug, Clone, Serialize)]
pub struct E2EVerdict {
    pub pass: bool,
    pub checks: Vec<E2ECheck>,
    pub mismatches: Vec<String>,
}

/// Run a platform-e2e case: stand up a fresh in-memory DB with the real eval
/// schema (the same migrations the app runs), seed it, and assert each set
/// expectation against the real persistence/logic functions. Pure data-plane —
/// no LLM, no provider, no browser. Returns the verdict with per-check detail.
pub fn run_platform_e2e(seed: E2ESeed, expect: E2EExpect) -> Result<E2EVerdict, String> {
    let conn = Connection::open_in_memory().map_err(|e| format!("open in-memory db: {e}"))?;
    // The migrations assume the base `schema_version` table exists — on a real
    // DB, DbState::open creates it via the static SCHEMA before any migration
    // runs. A bare in-memory connection has NO tables, so each migration's
    // trailing `INSERT INTO schema_version (version, applied_at) ...` would fail
    // and the whole driver would return Err on every call. Pre-create it here
    // (matching db.rs exactly) so the migrations run identically to app startup.
    // Without this the 4 unit tests below would panic on mac/linux CI (which
    // runs dev-workbench's tests, unlike Windows's 0xc0000139-blocked exe).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (\
            version INTEGER PRIMARY KEY,\
            applied_at TEXT NOT NULL\
        );",
    )
    .map_err(|e| format!("create schema_version: {e}"))?;
    // Real schema — the same CREATE TABLE the migrations run at app startup.
    migrate_v20_to_v21(&conn).map_err(|e| format!("verdicts schema: {e}"))?;
    migrate_v21_to_v22(&conn).map_err(|e| format!("eval_cases schema: {e}"))?;

    // Seed cases.
    for c in &seed.cases {
        insert_eval_case(
            &conn,
            &NewEvalCase {
                id: c.id.clone(),
                name: c.name.clone(),
                category: c.category.clone(),
                input_prompt: c.input_prompt.clone(),
                expected_steps_json: c.expected_steps_json.clone(),
                expected_output: None,
                expected_observables_json: None,
                negative_json: c.negative_json.clone(),
                source_session_id: None,
                commit_sha: None,
                draft: c.draft,
                created_at: "2026-07-03T00:00:00Z".to_string(),
            },
        )
        .map_err(|e| format!("seed case {}: {e}", c.id))?;
    }
    // Seed verdicts.
    for v in &seed.verdicts {
        insert_verdict(
            &conn,
            &NewVerdict {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: v.session_id.clone(),
                case_id: v.case_id.clone(),
                gate: v.gate.clone(),
                verdict: v.verdict.clone(),
                attribution: None,
                report: None,
                commit_sha: None,
                created_at: "2026-07-03T00:00:00Z".to_string(),
            },
        )
        .map_err(|e| format!("seed verdict: {e}"))?;
    }

    let mut checks: Vec<E2ECheck> = Vec::new();

    if let Some(want) = expect.approved_case_count {
        let approved = list_eval_cases(&conn, None, false, 1000)
            .map_err(|e| format!("list approved cases: {e}"))?;
        let got = approved.len();
        let pass = got == want;
        checks.push(E2ECheck {
            name: "approved_case_count (draft excluded)".into(),
            pass,
            detail: format!("got {got}, want {want}"),
        });
    }

    if let Some(want) = expect.total_case_count {
        let total = list_eval_cases(&conn, None, true, 1000)
            .map_err(|e| format!("list all cases: {e}"))?;
        let got = total.len();
        let pass = got == want;
        checks.push(E2ECheck {
            name: "total_case_count (include_drafts)".into(),
            pass,
            detail: format!("got {got}, want {want}"),
        });
    }

    if let Some((gate, want)) = expect.verdict_count_for_gate {
        let rows = list_verdicts(&conn, None, Some(&gate), None, 1000)
            .map_err(|e| format!("list verdicts for gate {gate}: {e}"))?;
        let got = rows.len();
        let pass = got == want;
        checks.push(E2ECheck {
            name: format!("verdict_count_for_gate({gate})"),
            pass,
            detail: format!("got {got}, want {want}"),
        });
    }

    if let Some(r) = expect.replay {
        // Load the case's frozen contract from the seeded DB (the same path
        // run_eval_replay takes), score the stubbed trajectory, compare grade.
        let case = crate::eval::cases::get_eval_case(&conn, &r.case_id)
            .map_err(|e| format!("load case {}: {e}", r.case_id))?
            .ok_or_else(|| format!("seed missing case {}", r.case_id))?;
        let actual: Vec<ToolStep> = r
            .actual_steps
            .iter()
            .map(|n| ToolStep {
                name: n.clone(),
                status: None,
            })
            .collect();
        let expected = parse_steps(case.expected_steps_json.as_deref());
        let negative = parse_steps(case.negative_json.as_deref());
        let v = score_replay(&actual, Some(&expected), r.matcher, &negative);
        let pass = v.grade == r.expected_grade;
        checks.push(E2ECheck {
            name: format!("replay grade (case {})", r.case_id),
            pass,
            detail: format!("got {:?}, want {:?}", v.grade, r.expected_grade),
        });
    }

    let mismatches: Vec<String> = checks
        .iter()
        .filter(|c| !c.pass)
        .map(|c| format!("{}: {}", c.name, c.detail))
        .collect();
    Ok(E2EVerdict {
        pass: mismatches.is_empty(),
        checks,
        mismatches,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(id: &str, draft: bool) -> E2ESeedCase {
        E2ESeedCase {
            id: id.into(),
            name: format!("case {id}"),
            category: "agent".into(),
            input_prompt: "do thing".into(),
            expected_steps_json: Some(r#"[{"name":"read"},{"name":"edit"}]"#.into()),
            negative_json: Some(r#"[{"name":"bash"}]"#.into()),
            draft,
        }
    }

    #[test]
    fn data_plane_passes_when_counts_and_grade_match() {
        // 2 approved + 1 draft seeded; expect 2 approved, 3 total; 1 eval-gate
        // verdict; replay [read,edit] on the case → Optimal.
        let seed = E2ESeed {
            cases: vec![case("c1", false), case("c2", false), case("c3", true)],
            verdicts: vec![E2ESeedVerdict {
                gate: "eval".into(),
                verdict: "PASS".into(),
                session_id: None,
                case_id: Some("c1".into()),
            }],
        };
        let expect = E2EExpect {
            approved_case_count: Some(2),
            total_case_count: Some(3),
            verdict_count_for_gate: Some(("eval".into(), 1)),
            replay: Some(E2EReplayExpect {
                case_id: "c1".into(),
                actual_steps: vec!["read".into(), "edit".into()],
                matcher: Matcher::ExactMatch,
                expected_grade: Grade::Optimal,
            }),
        };
        let v = run_platform_e2e(seed, expect).expect("e2e runs");
        assert!(v.pass, "all checks should pass: {:?}", v.mismatches);
        assert_eq!(v.checks.len(), 4);
    }

    #[test]
    fn draft_filter_mismatch_fails_the_check() {
        // The draft is supposed to be excluded from approved; if the contract
        // expects it counted (wrong), the check fails — proving the draft filter
        // is really exercised, not stubbed.
        let seed = E2ESeed {
            cases: vec![case("c1", false), case("c2", true)],
            verdicts: vec![],
        };
        let expect = E2EExpect {
            approved_case_count: Some(2), // wrong: draft c2 must be excluded
            total_case_count: None,
            verdict_count_for_gate: None,
            replay: None,
        };
        let v = run_platform_e2e(seed, expect).expect("e2e runs");
        assert!(!v.pass, "wrong approved count must fail");
        assert!(v.mismatches.iter().any(|m| m.contains("approved_case_count")));
    }

    #[test]
    fn negative_hit_replay_scores_incorrect() {
        // 反刷分 guard wired through the data plane: a trajectory that hits the
        // forbidden tool scores Incorrect regardless of the rest.
        let seed = E2ESeed {
            cases: vec![case("c1", false)],
            verdicts: vec![],
        };
        let expect = E2EExpect {
            approved_case_count: None,
            total_case_count: None,
            verdict_count_for_gate: None,
            replay: Some(E2EReplayExpect {
                case_id: "c1".into(),
                actual_steps: vec!["read".into(), "bash".into(), "edit".into()],
                matcher: Matcher::InOrder,
                expected_grade: Grade::Incorrect,
            }),
        };
        let v = run_platform_e2e(seed, expect).expect("e2e runs");
        assert!(v.pass, "negative hit → Incorrect: {:?}", v.mismatches);
    }

    #[test]
    fn none_expectations_are_all_dont_care_so_empty_seed_passes() {
        // No expectations set → nothing to fail → vacuous pass. Guards against a
        // future change that invents a default-failing check.
        let v = run_platform_e2e(E2ESeed::default(), E2EExpect::default()).expect("e2e runs");
        assert!(v.pass);
        assert!(v.checks.is_empty());
    }
}
