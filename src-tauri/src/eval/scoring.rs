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

// ─────────────────────────────────────────────────────────────────────────────
// AgentX-style 8-dimension reliability rubric (P6).
//
// The 3-state `score()` above judges the trajectory as a whole; this rubric
// scores reliability across 8 orthogonal dimensions and rolls them into a
// weighted Q_code. "评测 = 可靠地完成，不是完成" — a pass that needed a human
// nudge, or that hallucinated a forbidden action, is not reliable even if the
// final tool sequence matched.
//
// Every dimension is a DETERMINISTIC derivation from facts the trace/recording
// already carry (anti-gaming 客观事实原则): tool names, error statuses, the
// negative list, expected files, whether a human approval fired. No LLM judges
// its own reliability here. `manual_intervention` is a hard gate — any human
// nudge zeros Q_code (a reliable run needs no rescuing).
// ─────────────────────────────────────────────────────────────────────────────

/// One scored dimension of the reliability rubric. `score ∈ [0,1]` is the
/// normalized dimension score (1.0 = ideal); `val` is the human-readable raw
/// (e.g. "2 次", "3/4", "=0 命中"). `hard` marks hard-gate dimensions whose
/// failure zeros Q_code regardless of other dims.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RubricDim {
    /// Stable machine key (snake_case) — wire + indexing.
    pub key: String,
    /// Human label (zh) shown in the rubric card.
    pub label: String,
    /// Normalized score ∈ [0.0, 1.0].
    pub score: f64,
    /// Raw value string (count / ratio / status).
    pub val: String,
    /// Hard-gate dimension (failure zeros Q_code).
    #[serde(default)]
    pub hard: bool,
}

/// The full 8-dimension reliability verdict. `q_code` is the weighted roll-up
/// (0.0 if any hard gate tripped); `hard_gate_triggered` names which gate.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RubricScore {
    pub dims: Vec<RubricDim>,
    /// Weighted reliability score ∈ [0.0, 1.0]. 0.0 if a hard gate tripped.
    pub q_code: f64,
    /// True iff a hard-gate dim scored 0 (manual intervention).
    pub hard_gate_triggered: bool,
}

/// Inputs to the rubric. All borrowed — the rubric is a pure function over
/// already-recorded facts, no I/O, no LLM.
pub struct RubricInput<'a> {
    /// Actual tool-call sequence (ordered).
    pub actual: &'a [&'a str],
    /// Expected tool sequence (the case contract). None/empty → unconstrained.
    pub expected: Option<&'a [&'a str]>,
    pub matcher: Matcher,
    /// Forbidden tools (the case negative list). Touching any → hallucination 0.
    pub negative: &'a [&'a str],
    /// Expected file paths (the case's expected_observables, file subset).
    pub expected_files: &'a [&'a str],
    /// Files the run actually touched.
    pub actual_files: &'a [&'a str],
    /// Count of steps whose status was "error" (failed tool calls).
    pub failed_steps: usize,
    /// True iff a human approval/intervention fired during the run (hard gate).
    pub had_human_intervention: bool,
}

/// Read-before-modify tools (investigate, then act). Used by the harness-pattern
/// dimension: a reliable run inspects before it changes.
const READ_TOOLS: &[&str] = &["read", "glob", "grep", "ls"];
const MODIFY_TOOLS: &[&str] = &["edit", "write", "bash", "patch", "multiedit"];

/// Weights for the 7 non-gate dimensions (attribute hallucination is the
/// heaviest — S-tier in AgentX). `manual_intervention` is excluded from the
/// weighted sum: it's a hard gate, not a soft contributor. Weights sum to 1.0.
const W_TOOL: f64 = 0.15;
const W_HALLUC: f64 = 0.30;
const W_LOOP: f64 = 0.10;
const W_DRYRUN: f64 = 0.10;
const W_HARNESS: f64 = 0.10;
const W_DSL: f64 = 0.10;
const W_FILES: f64 = 0.15;

/// Score a run across the 8 AgentX reliability dimensions and roll up to a
/// weighted Q_code. Pure + deterministic — see [`RubricInput`] for the facts.
pub fn score_rubric(input: RubricInput) -> RubricScore {
    let total = input.actual.len();
    let neg_hits = input
        .actual
        .iter()
        .filter(|t| input.negative.iter().any(|n| n.eq_ignore_ascii_case(t)))
        .count();

    // 1. 工具选择准确率 — the trajectory score itself (reference match).
    let tool_choice = score(input.actual, input.expected, input.matcher).score;

    // 2. attribute hallucination (S-tier, heaviest) — touching a forbidden tool
    //    is the strongest hallucination signal available. 0 if any hit, else 1.
    let halluc = if input.negative.is_empty() {
        1.0 // no negative contract → can't detect hallucination by this signal
    } else if neg_hits > 0 {
        0.0
    } else {
        1.0
    };

    // 3. correctness-loop 迭代 — back-to-back identical calls = retry churn.
    let retries = total.saturating_sub(1).min(
        input
            .actual
            .windows(2)
            .filter(|w| w[0].eq_ignore_ascii_case(w[1]))
            .count(),
    );
    let loop_score = if total == 0 {
        0.0
    } else {
        1.0 - (retries as f64 / total as f64)
    };

    // 4. manual intervention (HARD GATE) — any human nudge → Q=0.
    let manual = if input.had_human_intervention { 0.0 } else { 1.0 };

    // 5. dryrun pass — fraction of steps that didn't error.
    let ok_steps = total.saturating_sub(input.failed_steps);
    let dryrun = if total == 0 {
        0.0
    } else {
        ok_steps as f64 / total as f64
    };

    // 6. harness-pattern — did the run investigate (read) before modifying?
    let harness = harness_pattern_score(input.actual);

    // 7. DSL / declarative adherence — structural coverage of expected steps.
    let dsl = coverage(input.actual, input.expected.unwrap_or(&[]));

    // 8. 文件变更符合预期 — expected file coverage in actual touched files.
    let files = coverage_lower(input.actual_files, input.expected_files);

    let dims = vec![
        RubricDim { key: "tool_choice".into(), label: "工具选择准确率".into(), score: tool_choice, val: fmt_pct(tool_choice), hard: false },
        RubricDim { key: "attr_hallucination".into(), label: "attribute hallucination".into(), score: halluc, val: if neg_hits > 0 { format!("命中 {neg_hits}") } else { "无".into() }, hard: false },
        RubricDim { key: "correctness_loop".into(), label: "correctness-loop 迭代".into(), score: loop_score, val: format!("{retries} 次"), hard: false },
        RubricDim { key: "manual_intervention".into(), label: "manual intervention ⚠硬门".into(), score: manual, val: if input.had_human_intervention { "=0 命中".into() } else { "无".into() }, hard: true },
        RubricDim { key: "dryrun_pass".into(), label: "dryrun pass".into(), score: dryrun, val: format!("{ok_steps}/{total}"), hard: false },
        RubricDim { key: "harness_pattern".into(), label: "harness-pattern".into(), score: harness, val: fmt_pct(harness), hard: false },
        RubricDim { key: "dsl".into(), label: "DSL 声明符合".into(), score: dsl, val: fmt_pct(dsl), hard: false },
        RubricDim { key: "file_change".into(), label: "文件变更符合预期".into(), score: files, val: fmt_pct(files), hard: false },
    ];

    let hard_gate_triggered = input.had_human_intervention;
    let q_code = if hard_gate_triggered {
        0.0
    } else {
        W_TOOL * tool_choice
            + W_HALLUC * halluc
            + W_LOOP * loop_score
            + W_DRYRUN * dryrun
            + W_HARNESS * harness
            + W_DSL * dsl
            + W_FILES * files
    };

    RubricScore {
        dims,
        q_code,
        hard_gate_triggered,
    }
}

/// harness-pattern: 1.0 if a read tool precedes the first modify tool
/// (investigate-then-act); 0.5 if it modifies without prior read; 0.0 if it
/// never reads at all yet still modifies (blind change).
fn harness_pattern_score(actual: &[&str]) -> f64 {
    let first_read = actual.iter().position(|t| is_in(t, READ_TOOLS));
    let first_modify = actual.iter().position(|t| is_in(t, MODIFY_TOOLS));
    match (first_read, first_modify) {
        (None, None) => 1.0,        // pure read or empty — nothing wrong
        (Some(_), None) => 1.0,     // only investigated — fine
        (None, Some(_)) => 0.0,     // modified blind, no read
        (Some(r), Some(m)) => {
            if r < m {
                1.0
            } else {
                0.5 // modified before reading back — half credit
            }
        }
    }
}

fn is_in(tool: &str, set: &[&str]) -> bool {
    set.iter().any(|s| s.eq_ignore_ascii_case(tool))
}

/// Like [`coverage`] but lower-cased + path-separator agnostic for file paths.
fn coverage_lower(actual: &[&str], reference: &[&str]) -> f64 {
    if reference.is_empty() {
        return 1.0;
    }
    let actual_lc: Vec<String> = actual.iter().map(|a| a.to_ascii_lowercase()).collect();
    let present = reference
        .iter()
        .filter(|r| actual_lc.iter().any(|a| a == &r.to_ascii_lowercase()))
        .count();
    present as f64 / reference.len() as f64
}

fn fmt_pct(x: f64) -> String {
    format!("{:.2}", x)
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

    // ---- 8-dim rubric (P6) ----

    fn dim<'a>(rs: &'a RubricScore, key: &str) -> &'a RubricDim {
        rs.dims.iter().find(|d| d.key == key).expect("dim exists")
    }

    fn clean_run() -> RubricInput<'static> {
        RubricInput {
            actual: &["read", "grep", "edit"],
            expected: Some(&["read", "grep", "edit"]),
            matcher: Matcher::ExactMatch,
            negative: &["bash"],
            expected_files: &["blocksview.tsx"],
            actual_files: &["blocksview.tsx"],
            failed_steps: 0,
            had_human_intervention: false,
        }
    }

    #[test]
    fn rubric_clean_run_aces_all_dims_and_q_above_threshold() {
        let rs = score_rubric(clean_run());
        assert!(!rs.hard_gate_triggered);
        for d in &rs.dims {
            assert!(d.score > 0.99, "{} should be ~1.0, got {}", d.key, d.score);
        }
        assert!(rs.q_code > 0.99, "clean run Q should be ~1.0, got {}", rs.q_code);
    }

    #[test]
    fn rubric_manual_intervention_is_hard_gate_zeroing_q() {
        let mut inp = clean_run();
        inp.had_human_intervention = true;
        let rs = score_rubric(inp);
        assert!(rs.hard_gate_triggered);
        assert_eq!(rs.q_code, 0.0);
        assert_eq!(dim(&rs, "manual_intervention").score, 0.0);
        assert_eq!(dim(&rs, "manual_intervention").val, "=0 命中");
    }

    #[test]
    fn rubric_forbidden_tool_zeros_hallucination_s_tier() {
        let mut inp = clean_run();
        inp.actual = &["read", "bash", "edit"]; // touched forbidden bash
        let rs = score_rubric(inp);
        let h = dim(&rs, "attr_hallucination");
        assert_eq!(h.score, 0.0);
        assert_eq!(h.val, "命中 1");
        // hallucination is the heaviest weight → Q drops materially even though
        // tool_choice still matches under InOrder-ish... here ExactMatch fails
        // because bash inserted, so tool_choice also < 1. Q must be well below 1.
        assert!(rs.q_code < 0.7);
    }

    #[test]
    fn rubric_retry_churn_lowers_correctness_loop() {
        let mut inp = clean_run();
        inp.actual = &["read", "read", "read", "edit"]; // 2 back-to-back repeats
        let rs = score_rubric(RubricInput { actual: inp.actual, ..clean_run() });
        let l = dim(&rs, "correctness_loop");
        assert_eq!(l.val, "2 次");
        assert!(l.score < 1.0);
    }

    #[test]
    fn rubric_failed_steps_lower_dryrun() {
        let mut inp = clean_run();
        inp.failed_steps = 1; // 1 of 3 failed
        let rs = score_rubric(inp);
        let d = dim(&rs, "dryrun_pass");
        assert_eq!(d.val, "2/3");
        assert!((d.score - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn rubric_blind_modify_zeros_harness_pattern() {
        let mut inp = clean_run();
        inp.actual = &["edit", "bash"]; // modify without any read
        let rs = score_rubric(RubricInput { actual: inp.actual, ..clean_run() });
        assert!((dim(&rs, "harness_pattern").score - 0.0).abs() < 1e-9);
    }

    #[test]
    fn rubric_missing_file_zeros_file_change_coverage() {
        let mut inp = clean_run();
        inp.actual_files = &["other.ts"]; // expected blocksview.tsx not touched
        let rs = score_rubric(inp);
        assert_eq!(dim(&rs, "file_change").score, 0.0);
    }

    #[test]
    fn rubric_no_negative_means_hallucination_unconstrained_one() {
        let mut inp = clean_run();
        inp.negative = &[];
        let rs = score_rubric(inp);
        // No negative contract → hallucination signal can't fire → 1.0 (honest:
        // we can't claim hallucination detection without a contract).
        assert_eq!(dim(&rs, "attr_hallucination").score, 1.0);
    }

    #[test]
    fn rubric_weights_sum_to_one_so_clean_q_is_one() {
        // Sanity: weights must sum to 1.0 so a perfect run is exactly Q=1.0.
        let sum = W_TOOL + W_HALLUC + W_LOOP + W_DRYRUN + W_HARNESS + W_DSL + W_FILES;
        assert!((sum - 1.0).abs() < 1e-9, "weights sum to {sum}, expected 1.0");
    }
}
