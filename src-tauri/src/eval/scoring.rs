//! B7 trajectory scoring — produce a numeric score in `[0.0, 1.0]` and a
//! three-state grade for an agent's tool-call trajectory, optionally compared
//! against a reference (expected) trajectory.
//!
//! Mirrors the OpenAI Agents SDK `samples/python/06-evaluate/
//! trajectory-evaluation` rubric. A trajectory is the ordered sequence of tool
//! calls an agent made; the rubric grades HOW the agent worked, not just the
//! final answer:
//! - **Optimal** — right tools in the right order, no redundant steps.
//! - **Suboptimal** — correct tools used, but with extra or redundant steps.
//! - **Incorrect** — missing expected tools (wrong tools / wrong path).
//!
//! Three matchers cover how strictly the actual sequence must follow the
//! reference: `ExactMatch` (element-for-element), `InOrder` (reference is an
//! ordered subsequence, gaps allowed), `AnyOrder` (reference tools all present,
//! ignoring order/count). Pure functions over `&[&str]` tool-name sequences so
//! the logic is exhaustively testable without any DB or wire payload.

use serde::{Deserialize, Serialize};

/// How strictly the actual trajectory must follow the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Matcher {
    /// `actual == reference`, element for element, in order.
    ExactMatch,
    /// `reference` is an ordered subsequence of `actual` (gaps allowed).
    InOrder,
    /// Every `reference` tool appears in `actual`, ignoring order and count.
    AnyOrder,
}

/// Three-state rubric grade (OpenAI sample optimal / suboptimal / incorrect).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grade {
    /// Right tools in the right order, no redundant steps.
    Optimal,
    /// Correct tools used, but with extra or redundant steps.
    Suboptimal,
    /// Missing expected tools — wrong path.
    Incorrect,
}

/// A scored trajectory: numeric score in `[0.0, 1.0]` + three-state grade.
#[derive(Debug, Clone, Serialize)]
pub struct EvalScore {
    pub score: f64,
    pub grade: Grade,
}

/// Does `actual` satisfy `reference` under `matcher`? True when every expected
/// tool is found according to the matcher's strictness.
fn matches(actual: &[&str], reference: &[&str], matcher: Matcher) -> bool {
    match matcher {
        Matcher::ExactMatch => actual == reference,
        Matcher::InOrder => is_subsequence(reference, actual),
        Matcher::AnyOrder => reference.iter().all(|r| actual.contains(r)),
    }
}

/// Ordered-subsequence check: `needle` appears in `haystack` in the same
/// relative order (gaps allowed). An empty `needle` is trivially a subsequence.
fn is_subsequence(needle: &[&str], haystack: &[&str]) -> bool {
    let mut it = haystack.iter();
    for n in needle {
        if !it.any(|h| h == n) {
            return false;
        }
    }
    true
}

/// Fraction of `reference`'s tools present in `actual` (count-agnostic). An
/// empty reference is no constraint at all → 1.0.
fn coverage(actual: &[&str], reference: &[&str]) -> f64 {
    if reference.is_empty() {
        return 1.0;
    }
    let present = reference.iter().filter(|r| actual.contains(r)).count();
    present as f64 / reference.len() as f64
}

/// Score an actual tool-name sequence against an optional reference. With a
/// reference: a clean match with no extra steps is `Optimal`; a match with
/// extras is `Suboptimal`; missing-tools is `Incorrect`. Without a reference:
/// a heuristic on the actual sequence alone judges redundancy (an empty
/// trajectory is `Incorrect`; back-to-back identical calls are `Suboptimal`).
pub fn score(actual: &[&str], reference: Option<&[&str]>, matcher: Matcher) -> EvalScore {
    match reference.filter(|r| !r.is_empty()) {
        None => heuristic(actual),
        Some(reference) => score_against_reference(actual, reference, matcher),
    }
}

fn score_against_reference(actual: &[&str], reference: &[&str], matcher: Matcher) -> EvalScore {
    if matches(actual, reference, matcher) {
        // All expected tools present under the matcher. Redundant extra steps
        // → Suboptimal (correct path, inefficient); tight fit → Optimal.
        if actual.len() <= reference.len() {
            EvalScore {
                score: 1.0,
                grade: Grade::Optimal,
            }
        } else {
            let ratio = reference.len() as f64 / actual.len() as f64;
            EvalScore {
                score: ratio,
                grade: Grade::Suboptimal,
            }
        }
    } else {
        // Missing expected tools → Incorrect, scaled by how much was covered.
        let cov = coverage(actual, reference);
        EvalScore {
            score: cov * 0.49,
            grade: Grade::Incorrect,
        }
    }
}

/// Reference-free heuristic: empty trajectory is `Incorrect`; back-to-back
/// identical calls (the canonical redundant step) push to `Suboptimal`; else
/// `Optimal`.
fn heuristic(actual: &[&str]) -> EvalScore {
    if actual.is_empty() {
        return EvalScore {
            score: 0.0,
            grade: Grade::Incorrect,
        };
    }
    if has_redundant(actual) {
        let unique = {
            let mut s = actual.to_vec();
            s.sort_unstable();
            s.dedup();
            s.len()
        };
        let ratio = unique as f64 / actual.len() as f64;
        EvalScore {
            score: ratio,
            grade: Grade::Suboptimal,
        }
    } else {
        EvalScore {
            score: 1.0,
            grade: Grade::Optimal,
        }
    }
}

/// True iff any tool is called twice in a row (the canonical "redundant step").
fn has_redundant(actual: &[&str]) -> bool {
    actual.windows(2).any(|w| w[0] == w[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ExactMatch ----

    #[test]
    fn exact_match_equal_is_optimal() {
        let s = score(&["read", "grep", "edit"], Some(&["read", "grep", "edit"]), Matcher::ExactMatch);
        assert_eq!(s.grade, Grade::Optimal);
        assert!((s.score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn exact_match_extra_step_is_suboptimal() {
        let s = score(&["read", "grep", "edit", "bash"], Some(&["read", "grep", "edit"]), Matcher::ExactMatch);
        // ExactMatch fails (actual != reference) but... here actual has the
        // reference as a prefix + extra. ExactMatch is element-equality, so a
        // longer actual does NOT match → Incorrect, NOT Suboptimal. This is
        // intentional: ExactMatch demands a tight fit; extras mean the path
        // diverged. Use InOrder to tolerate extras.
        assert_eq!(s.grade, Grade::Incorrect);
    }

    #[test]
    fn exact_match_missing_tool_is_incorrect() {
        let s = score(&["read", "grep"], Some(&["read", "grep", "edit"]), Matcher::ExactMatch);
        assert_eq!(s.grade, Grade::Incorrect);
        // 2/3 covered → 0.49 * 2/3 ≈ 0.327, within (0, 0.49].
        assert!(s.score > 0.0 && s.score <= 0.49);
    }

    // ---- InOrder ----

    #[test]
    fn in_order_subsequence_with_gaps_is_optimal() {
        // reference appears in order within actual, with extra steps between.
        // No extra → Optimal; with extra steps it's Suboptimal (redundant).
        let s = score(&["read", "bash", "grep"], Some(&["read", "grep"]), Matcher::InOrder);
        assert_eq!(s.grade, Grade::Suboptimal, "extra 'bash' step → suboptimal");
        assert!(s.score < 1.0);
    }

    #[test]
    fn in_order_tight_fit_is_optimal() {
        let s = score(&["read", "grep"], Some(&["read", "grep"]), Matcher::InOrder);
        assert_eq!(s.grade, Grade::Optimal);
    }

    #[test]
    fn in_order_wrong_order_is_incorrect() {
        // reference wants read→grep, actual did grep→read: not an ordered
        // subsequence → Incorrect.
        let s = score(&["grep", "read"], Some(&["read", "grep"]), Matcher::InOrder);
        assert_eq!(s.grade, Grade::Incorrect);
    }

    // ---- AnyOrder ----

    #[test]
    fn any_order_present_ignores_order_and_count() {
        // All reference tools present (read, edit), reordered, with extras →
        // Suboptimal (matched, but redundant 'grep' + duplicate 'read').
        let s = score(
            &["grep", "read", "read", "edit"],
            Some(&["read", "edit"]),
            Matcher::AnyOrder,
        );
        assert_eq!(s.grade, Grade::Suboptimal);
        assert!(s.score < 1.0);
    }

    #[test]
    fn any_order_tight_fit_is_optimal() {
        let s = score(&["edit", "read"], Some(&["read", "edit"]), Matcher::AnyOrder);
        assert_eq!(s.grade, Grade::Optimal);
    }

    #[test]
    fn any_order_missing_tool_is_incorrect() {
        let s = score(&["read"], Some(&["read", "edit"]), Matcher::AnyOrder);
        assert_eq!(s.grade, Grade::Incorrect);
        assert!(s.score <= 0.49);
    }

    // ---- reference-free heuristic ----

    #[test]
    fn heuristic_empty_is_incorrect() {
        let s = score(&[], None, Matcher::ExactMatch);
        assert_eq!(s.grade, Grade::Incorrect);
        assert_eq!(s.score, 0.0);
    }

    #[test]
    fn heuristic_no_repeats_is_optimal() {
        let s = score(&["read", "grep", "edit"], None, Matcher::ExactMatch);
        assert_eq!(s.grade, Grade::Optimal);
    }

    #[test]
    fn heuristic_consecutive_repeat_is_suboptimal() {
        // 'read' called twice in a row → redundant → Suboptimal.
        let s = score(&["read", "read", "grep"], None, Matcher::ExactMatch);
        assert_eq!(s.grade, Grade::Suboptimal);
        // unique{read,grep}=2 / actual=3 → 0.667.
        assert!((s.score - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn empty_reference_treated_as_heuristic() {
        // Some(&[]) is no constraint → same as None → heuristic path.
        let s = score(&["read"], Some(&[]), Matcher::ExactMatch);
        assert_eq!(s.grade, Grade::Optimal);
    }

    // ---- serde wire format ----

    #[test]
    fn matcher_and_grade_serde_snake_case() {
        let m = serde_json::to_string(&Matcher::ExactMatch).unwrap();
        assert_eq!(m, "\"exact_match\"");
        let g = serde_json::to_string(&Grade::Suboptimal).unwrap();
        assert_eq!(g, "\"suboptimal\"");
        let m2: Matcher = serde_json::from_str("\"in_order\"").unwrap();
        assert_eq!(m2, Matcher::InOrder);
        let g2: Grade = serde_json::from_str("\"incorrect\"").unwrap();
        assert_eq!(g2, Grade::Incorrect);
    }

    // ---- subsequence helper ----

    #[test]
    fn is_subsequence_handles_gaps_and_empty() {
        assert!(is_subsequence(&[], &["a"]));
        assert!(is_subsequence(&["a", "c"], &["a", "b", "c"]));
        assert!(!is_subsequence(&["c", "a"], &["a", "b", "c"]));
        assert!(!is_subsequence(&["x"], &["a", "b"]));
    }
}
