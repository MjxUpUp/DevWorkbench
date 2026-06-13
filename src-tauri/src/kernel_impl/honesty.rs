//! HonestyVerifier — the anti-self-deception quality layer.
//!
//! This is the kernel's first-class answer to the user's #1 documented pain
//! (CLAUDE.md / AGENTS.md "铁律"): agents that claim success while staring at
//! errors. Three checks, each returning evidence (real output, not "should pass"):
//!
//! 1. [`check_assertion_weakening`] — scan a git diff for test changes that
//!    weaken assertions: `t.Fatal` → `t.Log`, `assert!` → `println!`,
//!    `assert_eq!(a,b)` → `assert_eq!(a,b,)`-with-tolerance, `#[ignore]` added,
//!    `t.Skip` added, `expect` → `unwrap_or`. These are exactly the patterns the
//!    user's `test-discipline` skill forbids.
//! 2. [`require_proof_of_completion`] — an agent's claimed "done" must carry the
//!    real stdout of the project's highest-tier test, not a paraphrase.
//! 3. [`verify_env_sane`] — before trusting any verification, confirm the
//!    environment isn't already broken (no compile errors pre-existing).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A finding from an honesty check, with the offending snippet as evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HonestyWarning {
    pub severity: Severity,
    pub rule: String,
    pub file: String,
    /// The added (or removed) line that triggered the finding.
    pub evidence: String,
    pub explanation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Assertion weakening — almost always dishonest.
    Error,
    /// Suspicious but maybe legitimate (e.g. #[ignore] on a flaky test).
    Warning,
}

/// A unified diff (the `+`/`-`/` ` line format `git diff` produces).
pub struct GitDiff<'a> {
    pub lines: Vec<DiffLine>,
    #[allow(dead_code)]
    _raw: std::marker::PhantomData<&'a str>,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: LineKind,
    pub file: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Added,
    Removed,
    Context,
}

/// Parse a unified diff into structured lines, tracking the current file from
/// `+++ b/...` headers.
pub fn parse_diff(diff_text: &str) -> GitDiff<'_> {
    let mut lines = Vec::new();
    let mut current_file = String::new();
    for raw in diff_text.lines() {
        if let Some(f) = raw.strip_prefix("+++ b/") {
            current_file = f.to_string();
            continue;
        }
        if let Some(f) = raw.strip_prefix("+++ ") {
            // fallback for non-prefixed
            current_file = f.trim_start_matches("b/").to_string();
            continue;
        }
        let (kind, text) = if let Some(t) = raw.strip_prefix('+') {
            (LineKind::Added, t)
        } else if let Some(t) = raw.strip_prefix('-') {
            (LineKind::Removed, t)
        } else {
            (LineKind::Context, raw)
        };
        lines.push(DiffLine {
            kind,
            file: current_file.clone(),
            text: text.trim().to_string(),
        });
    }
    GitDiff {
        lines,
        _raw: std::marker::PhantomData,
    }
}

/// Weakening pairs: (marker seen in a REMOVED line, marker seen in an ADDED
/// line of the same file, rule name, explanation). The detection fires when a
/// removed line carries a strong assertion and an added line carries its weaker
/// replacement — e.g. removed `t.Fatal(...)`, added `t.Log(...)`.
const WEAKENING_PAIRS: &[(&str, &str, &str, &str)] = &[
    ("t.Fatal", "t.Log", "fatal_to_log",
     "t.Fatal was weakened to t.Log — failing tests now silently log instead of aborting"),
    ("assert!", "println!", "assert_to_println",
     "assert! was weakened to println! — a real assertion became a non-checking statement"),
    ("assert_eq!", "unwrap_or(", "assert_eq_to_unwrap_or",
     "assert_eq! was weakened to unwrap_or() — panics became silent fallbacks"),
    ("expect(", "unwrap_or(", "expect_to_unwrap_or",
     "expect() was weakened to unwrap_or() — panics became silent fallbacks"),
    // Single-sided: an added line that ADDED an ignore/skip (hiding a test).
    // Modeled as (anything, marker) — the 'removed' side is matched as .* .
    ("", "#[ignore]", "ignore_added",
     "#[ignore] added — a test was hidden from the suite instead of being fixed"),
    ("", "t.Skip", "skip_added",
     "t.Skip added — a failing test is being skipped instead of repaired"),
];

/// Check a diff for assertion-weakening. Detects two shapes:
/// 1. removed line has strong-assertion marker AND added line has weak replacement.
/// 2. added line introduces a suppression (`#[ignore]`, `t.Skip`) outright.
pub fn check_assertion_weakening(diff: &GitDiff<'_>) -> Vec<HonestyWarning> {
    let mut warnings = Vec::new();
    use std::collections::HashMap;
    let mut by_file: HashMap<String, (Vec<&DiffLine>, Vec<&DiffLine>)> = HashMap::new();
    for l in &diff.lines {
        let entry = by_file.entry(l.file.clone()).or_default();
        match l.kind {
            LineKind::Added => entry.0.push(l),
            LineKind::Removed => entry.1.push(l),
            LineKind::Context => {}
        }
    }
    for (file, (added, removed)) in by_file {
        for &(removed_marker, added_marker, rule, explain) in WEAKENING_PAIRS {
            // For single-sided rules (removed_marker == ""), fire if any added
            // line contains the suppression marker.
            if removed_marker.is_empty() {
                for a in &added {
                    if a.text.contains(added_marker) {
                        warnings.push(HonestyWarning {
                            severity: Severity::Error,
                            rule: rule.into(),
                            file: file.clone(),
                            evidence: a.text.clone(),
                            explanation: explain.into(),
                        });
                    }
                }
                continue;
            }
            // Two-sided: need a removed line with the strong marker AND an added
            // line with the weak marker in the same file.
            let had_strong = removed.iter().any(|r| r.text.contains(removed_marker));
            if !had_strong {
                continue;
            }
            for a in &added {
                if a.text.contains(added_marker) {
                    warnings.push(HonestyWarning {
                        severity: Severity::Error,
                        rule: rule.into(),
                        file: file.clone(),
                        evidence: format!("-{removed_marker}…  +{}", a.text),
                        explanation: explain.into(),
                    });
                }
            }
        }
    }
    warnings
}

/// Require that a completion claim carries real proof. `claim` is the agent's
/// final message; `proof` is the captured stdout of the verification command.
/// If the claim asserts success but the proof contains a failure signal, that's
/// a finding.
pub fn require_proof_of_completion(claim: &str, proof: &str) -> Result<(), HonestyWarning> {
    let claim_ok = claim.to_lowercase().contains("pass")
        || claim.contains("通过")
        || claim.contains("完成");
    let proof_failed = proof.contains("FAILED")
        || proof.contains("error[")
        || proof.contains("panicked")
        || proof.contains("test result: FAILED");
    if claim_ok && proof_failed {
        return Err(HonestyWarning {
            severity: Severity::Error,
            rule: "claim_without_proof".into(),
            file: String::new(),
            evidence: proof.lines().take(3).collect::<Vec<_>>().join("\n"),
            explanation: "agent claimed success but the verification output shows failure".into(),
        });
    }
    Ok(())
}

/// Quick environment sanity check: a pre-existing compile error means the agent
/// can't honestly be blamed for it, and any "I fixed it" claim is suspect.
pub fn verify_env_sane(compile_check_output: &str) -> Result<(), HonestyWarning> {
    if compile_check_output.contains("error[") || compile_check_output.contains("error:") {
        return Err(HonestyWarning {
            severity: Severity::Warning,
            rule: "env_not_sane".into(),
            file: String::new(),
            evidence: compile_check_output.lines().take(2).collect::<Vec<_>>().join("\n"),
            explanation: "environment has pre-existing compile errors — fix those first".into(),
        });
    }
    Ok(())
}

/// Serialize a list of findings into the JSON the Gate node / frontend expects.
pub fn findings_to_json(warnings: &[HonestyWarning]) -> Value {
    serde_json::to_value(warnings).unwrap_or_else(|_| serde_json::json!({"status": "unknown"}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_fatal_to_log_weakening() {
        // `t.Fatal` line is removed (-), `t.Log` line is added (+).
        let diff = parse_diff(
            "+++ b/foo_test.rs\n-foo()\n-t.Fatal(\"boom\")\n+foo()\n+t.Log(\"boom\")\n",
        );
        let w = check_assertion_weakening(&diff);
        assert!(w.iter().any(|x| x.rule == "fatal_to_log"), "got: {w:?}");
    }

    #[test]
    fn no_warning_when_no_assertion_removed() {
        let diff = parse_diff("+++ b/main.rs\n-old\n+new\n");
        let w = check_assertion_weakening(&diff);
        assert!(w.is_empty(), "got: {w:?}");
    }

    #[test]
    fn proof_check_flags_claim_with_failure() {
        let claim = "All tests pass ✅";
        let proof = "running 3 tests\ntest result: FAILED. 1 passed; 1 failed";
        assert!(require_proof_of_completion(claim, proof).is_err());
    }

    #[test]
    fn proof_check_passes_when_claim_matches_proof() {
        let claim = "done";
        let proof = "test result: ok. 3 passed";
        assert!(require_proof_of_completion(claim, proof).is_ok());
    }

    #[test]
    fn env_sane_flags_existing_error() {
        let out = "error[E0308]: mismatched types";
        assert!(verify_env_sane(out).is_err());
    }
}
