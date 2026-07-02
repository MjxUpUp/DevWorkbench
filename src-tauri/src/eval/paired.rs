//! L4 paired comparison — the anti-gaming gate on platform evolution. A
//! strategy / prompt / model change is NOT judged by its raw score; it is judged
//! by diffing new vs old on the SAME batch of cases. This is the SGPO paired-
//! replay discipline (AgentX): a regression blocks merge, and a gain must show
//! a verifiable causal chain or it lands as BRAKE (反刷分 #2 因果归因:
//! unattributed gain = brake, not a win).
//!
//! The causal-chain test is deliberately mechanical: a real improvement should
//! close the gap between the agent's trajectory and the case's `expected_steps`
//! (it picked up a step it was missing, or dropped a redundant one). If the new
//! version scores better WITHOUT closing that gap — e.g. it swapped one set of
//! tools for another and happened to match under a loose matcher — the gain is
//! unattributed, and反刷分 demands BRAKE, not CLEAR. A gain with no `expected`
//! contract to diff against is BRAKE by default (there's no chain to verify).
//!
//! Like [`crate::eval::replay`], the pure core ([`compare_paired`]) is unit-
//! tested here; the orchestration that runs two replays (old commit vs new) is
//! the driver's job, verified via example / CI.

use serde::{Deserialize, Serialize};

use crate::eval::extract::ToolStep;
use crate::eval::replay::ReplayVerdict;

/// How a case fared under the new platform version vs the old.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairedOutcome {
    /// New version passes where old failed — a real gain (needs attribution).
    Improvement,
    /// New version fails where old passed — a regression. Blocks merge.
    Regression,
    /// Same verdict both sides — no signal about this change either way.
    NoChange,
}

/// The verdict on one case's old-vs-new diff. `attribution` encodes the反刷分
/// stance: CLEAR for an attributed gain, BRAKE for an unattributed one, None
/// when there's no gain to attribute (regression / no-change).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairedVerdict {
    pub outcome: PairedOutcome,
    /// CLEAR = gain with a verifiable trajectory improvement; BRAKE = gain
    /// without one (or with no expected contract to verify against); None =
    /// regression / no-change (no gain to attribute).
    pub attribution: Option<String>,
    pub reason: String,
}

/// Count how many `expected` steps are MISSING from `actual` (set membership,
/// order-agnostic — the gap-to-contract measure). Zero when `actual` covers
/// every expected tool.
fn missing_expected(actual: &[ToolStep], expected: &[ToolStep]) -> usize {
    let actual_names: std::collections::HashSet<&str> =
        actual.iter().map(|s| s.name.as_str()).collect();
    expected
        .iter()
        .filter(|e| !actual_names.contains(e.name.as_str()))
        .count()
}

/// Diff one case's old-vs-new replay verdicts. Pure function — the entire反刷分
/// #2 (因果归因) for paired replay lives here.
///
/// - **Regression** (old PASS → new FAIL): blocks merge. No attribution (a
///   regression is a brake signal, not a gain).
/// - **NoChange**: no signal. No attribution.
/// - **Improvement** (old FAIL → new PASS): the gain is attributed CLEAR only
///   when the new trajectory demonstrably closed the gap to `expected`
///   (`missing_expected` strictly decreased). Otherwise BRAKE — a gain with no
///   verifiable cause. With no `expected` contract, BRAKE by default (there's no
///   chain to check).
pub fn compare_paired(
    old: &ReplayVerdict,
    new: &ReplayVerdict,
    old_actual: &[ToolStep],
    new_actual: &[ToolStep],
    expected: Option<&[ToolStep]>,
) -> PairedVerdict {
    let outcome = match (old.verdict.as_str(), new.verdict.as_str()) {
        ("PASS", "FAIL") => PairedOutcome::Regression,
        ("FAIL", "PASS") => PairedOutcome::Improvement,
        _ => PairedOutcome::NoChange,
    };

    match outcome {
        PairedOutcome::Regression => PairedVerdict {
            outcome,
            attribution: None,
            reason: "regression: case passed on the old version, fails on the new — blocks merge"
                .into(),
        },
        PairedOutcome::NoChange => PairedVerdict {
            outcome,
            attribution: None,
            reason: "no change between versions on this case".into(),
        },
        PairedOutcome::Improvement => {
            // 反刷分 #2: a gain MUST show a verifiable causal chain. The chain
            // we can verify mechanically is "the new trajectory closed the gap
            // to the expected contract". No contract ⇒ no chain to verify ⇒ BRAKE.
            match expected {
                None => PairedVerdict {
                    outcome,
                    attribution: Some("BRAKE".into()),
                    reason:
                        "improvement but no expected contract to attribute the gain to — \
                         unattributed gain = BRAKE (反刷分)"
                            .into(),
                },
                Some(exp) => {
                    let old_miss = missing_expected(old_actual, exp);
                    let new_miss = missing_expected(new_actual, exp);
                    if new_miss < old_miss {
                        PairedVerdict {
                            outcome,
                            attribution: Some("CLEAR".into()),
                            reason: format!(
                                "gain attributed: new trajectory closed the gap to expected \
                                 ({old_miss} → {new_miss} missing steps)"
                            ),
                        }
                    } else {
                        PairedVerdict {
                            outcome,
                            attribution: Some("BRAKE".into()),
                            reason: format!(
                                "improvement but the new trajectory did NOT close the gap to \
                                 expected ({old_miss} → {new_miss} missing) — unattributed gain = \
                                 BRAKE (反刷分)"
                            ),
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::replay::score_replay;
    use crate::eval::scoring::Matcher;

    fn steps(names: &[&str]) -> Vec<ToolStep> {
        names
            .iter()
            .map(|n| ToolStep {
                name: n.to_string(),
                status: None,
            })
            .collect()
    }

    fn verdict(pass: bool) -> ReplayVerdict {
        ReplayVerdict {
            score: if pass { 1.0 } else { 0.0 },
            grade: if pass {
                crate::eval::scoring::Grade::Optimal
            } else {
                crate::eval::scoring::Grade::Incorrect
            },
            verdict: if pass { "PASS".into() } else { "FAIL".into() },
            attribution: if pass { Some("CLEAR".into()) } else { None },
            negative_violated: false,
            reason: "stub".into(),
        }
    }

    #[test]
    fn regression_blocks_merge_with_no_attribution() {
        // Old passed, new fails → Regression, no attribution (it's a brake,
        // not a gain).
        let v = compare_paired(&verdict(true), &verdict(false), &[], &[], None);
        assert_eq!(v.outcome, PairedOutcome::Regression);
        assert!(v.attribution.is_none());
    }

    #[test]
    fn no_change_carries_no_signal() {
        let v = compare_paired(&verdict(true), &verdict(true), &[], &[], None);
        assert_eq!(v.outcome, PairedOutcome::NoChange);
        assert!(v.attribution.is_none());
    }

    #[test]
    fn improvement_without_expected_contract_is_brake() {
        // 反刷分: a gain with no expected contract to verify the causal chain
        // against → BRAKE (unattributed gain), NOT CLEAR. The agent can't claim
        // a win it can't show the work for.
        let v = compare_paired(&verdict(false), &verdict(true), &[], &[], None);
        assert_eq!(v.outcome, PairedOutcome::Improvement);
        assert_eq!(v.attribution.as_deref(), Some("BRAKE"));
    }

    #[test]
    fn improvement_that_closes_the_gap_is_clear() {
        // Old missed 'grep' (1 missing); new picked it up (0 missing). The gain
        // is attributed to a verifiable trajectory improvement → CLEAR.
        let expected = steps(&["read", "grep"]);
        let old_actual = steps(&["read"]); // missing grep
        let new_actual = steps(&["read", "grep"]); // complete
        let v = compare_paired(
            &verdict(false),
            &verdict(true),
            &old_actual,
            &new_actual,
            Some(&expected),
        );
        assert_eq!(v.outcome, PairedOutcome::Improvement);
        assert_eq!(v.attribution.as_deref(), Some("CLEAR"));
        assert!(v.reason.contains("1 → 0"), "reason reports the gap closure: {}", v.reason);
    }

    #[test]
    fn improvement_that_does_not_close_the_gap_is_brake() {
        // 反刷分核心: new version passes where old failed, BUT its trajectory
        // did NOT get closer to expected (still missing the same step) — the
        // gain is unattributed (likely matcher luck / LLM variance), so BRAKE,
        // not CLEAR. This is the exact刷分 this layer exists to catch.
        let expected = steps(&["read", "grep"]);
        // Both old and new miss 'grep' — but new passes anyway? Construct the
        // verdicts directly to simulate a loose-matcher pass without gap closure.
        let old_actual = steps(&["read", "edit"]);
        let new_actual = steps(&["read", "edit"]); // same gap, no improvement
        let v = compare_paired(
            &verdict(false),
            &verdict(true),
            &old_actual,
            &new_actual,
            Some(&expected),
        );
        assert_eq!(v.outcome, PairedOutcome::Improvement);
        assert_eq!(
            v.attribution.as_deref(),
            Some("BRAKE"),
            "a gain that didn't close the gap is unattributed = BRAKE"
        );
    }

    #[test]
    fn end_to_end_score_then_compare_attributes_clear_on_real_gap_closure() {
        // Wire score_replay → compare_paired on real trajectories, not stubs:
        // old ran [read] (Incorrect under ExactMatch vs [read,grep,edit]), new
        // ran [read,grep,edit] (Optimal). The improvement closed the gap → CLEAR.
        let expected = steps(&["read", "grep", "edit"]);
        let old_actual = steps(&["read"]);
        let new_actual = steps(&["read", "grep", "edit"]);
        let old = score_replay(&old_actual, Some(&expected), Matcher::ExactMatch, &[]);
        let new = score_replay(&new_actual, Some(&expected), Matcher::ExactMatch, &[]);
        assert_eq!(old.verdict, "FAIL");
        assert_eq!(new.verdict, "PASS");
        let paired = compare_paired(&old, &new, &old_actual, &new_actual, Some(&expected));
        assert_eq!(paired.outcome, PairedOutcome::Improvement);
        assert_eq!(paired.attribution.as_deref(), Some("CLEAR"));
    }
}
