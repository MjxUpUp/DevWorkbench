//! Gate evaluators — the host-injected logic a `Gate` node runs.
//!
//! `GateNode` is declarative (`gate: String` + `config: Value`); the actual
//! evaluation lives in the host `Executor::run_gate`. The deterministic gates
//! (`forge` quality gates, `honesty` static-diff audit) are host-side because
//! they touch the filesystem / external tools. The `"verify"` gate's logic,
//! however, is PURE over a `ChatModel` (no fs, no db, no AppHandle) — so it
//! lives HERE in the crate layer where it is unit-testable with a stub model,
//! rather than buried in the app crate (whose test binary can't load on every
//! platform). The host `run_gate` builds the model + calls [`verify_via_review`].

use kernel_core::{ChatModel, Message, ModelOptions};
use serde_json::{Value, json};

/// Read-only adversarial review — the evaluator-optimizer pattern: ask a
/// `ChatModel` whether `claim` (the upstream node's output) meets the criteria
/// in `prompt`, returning a gate report `{gate, passed, report}`.
///
/// Pure over the model so it is unit-testable with a stub `ChatModel` (the host
/// builds the real provider-backed model and hands it in). The verdict contract:
/// the reviewer's FIRST output line must be exactly `VERDICT: PASS` /
/// `VERDICT: FAIL`; models that ignore the contract fall back to
/// [`parse_verdict`]'s keyword heuristic. Orthogonal to the static gates
/// (`honesty` diff audit, `forge` quality gates) — this is semantic
/// cross-verification the deterministic checks cannot do.
///
/// The reviewer receives NO tools (a single `generate()`), so it cannot mutate
/// the project — it only judges.
pub async fn verify_via_review(
    chat: &dyn ChatModel,
    claim: &str,
    prompt: &str,
) -> Result<Value, String> {
    let messages = vec![
        Message::system(
            "You are a strict, adversarial reviewer. Verify the work product against the \
             given criteria. Your FIRST output line MUST be exactly 'VERDICT: PASS' or \
             'VERDICT: FAIL' (nothing else on that line), followed by your review. PASS \
             ONLY if the work fully meets the criteria — any real defect, gap, or \
             unverified claim is FAIL. Do not be generous.",
        ),
        Message::user(format!("{prompt}\n\n--- Work product to verify ---\n{claim}")),
    ];
    let resp = chat
        .generate(&messages, &ModelOptions::default())
        .await
        .map_err(|e| format!("verify review generate: {e}"))?;
    let report = resp.content.trim().to_string();
    let passed = parse_verdict(&report);
    Ok(json!({"gate": "verify", "passed": passed, "report": report}))
}

/// Extract a pass/fail verdict from a reviewer report.
///
/// Contract: the FIRST line must be EXACTLY `VERDICT: PASS` / `VERDICT: FAIL`.
/// A line that merely CONTAINS the token (e.g. `"The work has defects. VERDICT:
/// PASS"`) is a *violated* contract — we do NOT trust the embedded PASS; we fall
/// through to the keyword scan, where the `defect` marker correctly yields FAIL.
/// This is adversarial: a contract violation must never paper over a fail
/// marker.
///
/// Even a CLEAN `VERDICT: PASS` first line is overridden if the report body
/// later names a defect/fail marker — a reviewer hedging "PASS … but one
/// defect" is a conflicting signal, judged FAIL. With no clean contract line,
/// fall back to keyword presence on the whole report. Default FAIL — an
/// unreadable or ambiguous report never passes the work by default.
pub fn parse_verdict(report: &str) -> bool {
    let has_fail_marker = |text: &str| {
        let l = text.to_ascii_lowercase();
        l.contains("fail") || l.contains("defect") || l.contains("不通过") || l.contains("缺陷")
    };
    if let Some(first) = report.lines().next() {
        // Exact match only (trim whitespace + trailing sentence punctuation).
        // `contains` here would let "...defects. VERDICT: PASS" sneak through.
        let l = first
            .trim()
            .trim_end_matches(['.', '。', '!', '？', '!', '?'])
            .to_ascii_lowercase();
        if l == "verdict: pass" {
            // A clean PASS is still overridden by any fail marker in the body.
            return !has_fail_marker(report);
        }
        if l == "verdict: fail" {
            return false;
        }
        // Anything else on the first line = violated contract → fall through.
    }
    let l = report.to_ascii_lowercase();
    let pass = l.contains("pass") || l.contains("通过");
    pass && !has_fail_marker(&l)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_core::{Error, MessageStream, Role};

    /// Stub ChatModel returning a canned assistant message — isolates
    /// verify_via_review's prompt assembly + verdict parsing from any LLM.
    struct StubReviewer {
        reply: String,
    }

    #[async_trait::async_trait]
    impl ChatModel for StubReviewer {
        async fn generate(
            &self,
            _messages: &[Message],
            _opts: &ModelOptions,
        ) -> Result<Message, Error> {
            Ok(Message::assistant(self.reply.clone()))
        }
        fn stream(
            &self,
            _messages: &[Message],
            _opts: &ModelOptions,
        ) -> Result<MessageStream, Error> {
            Err(Error::Unsupported("stub reviewer has no stream".into()))
        }
    }

    #[test]
    fn parse_verdict_first_line_contract_wins() {
        assert!(parse_verdict("VERDICT: PASS\nlooks good"));
        assert!(!parse_verdict("VERDICT: FAIL\nfound a bug"));
    }

    #[test]
    fn parse_verdict_case_insensitive() {
        assert!(parse_verdict("verdict: pass\nok"));
        assert!(!parse_verdict("Verdict: Fail\nnope"));
    }

    #[test]
    fn parse_verdict_keyword_fallback_pass() {
        // No verdict line → keyword heuristic: explicit pass, no fail marker.
        assert!(parse_verdict("The implementation is correct and passes review."));
    }

    #[test]
    fn parse_verdict_keyword_fallback_fail_dominates() {
        // A fail/defect marker present → FAIL even if "pass" appears too.
        assert!(!parse_verdict("This passes most checks but has one defect."));
        assert!(!parse_verdict("大致通过，但存在缺陷。"));
    }

    #[test]
    fn parse_verdict_defaults_to_fail_when_ambiguous() {
        // Adversarial default: empty/unreadable → FAIL, never pass-by-default.
        assert!(!parse_verdict(""));
        assert!(!parse_verdict("一些无法判断的文字"));
    }

    #[tokio::test]
    async fn verify_via_review_pass_wraps_report() {
        let stub = StubReviewer {
            reply: "VERDICT: PASS\nAll criteria met.".into(),
        };
        let v = verify_via_review(&stub, "the work", "check it").await.unwrap();
        assert_eq!(v["gate"], "verify");
        assert_eq!(v["passed"], true);
        assert!(v["report"].as_str().unwrap().contains("All criteria met."));
    }

    #[tokio::test]
    async fn verify_via_review_fail_propagates() {
        let stub = StubReviewer {
            reply: "VERDICT: FAIL\nMissing tests.".into(),
        };
        let v = verify_via_review(&stub, "the work", "check it").await.unwrap();
        assert_eq!(v["passed"], false);
        assert!(v["report"].as_str().unwrap().contains("Missing tests."));
    }

    #[tokio::test]
    async fn verify_via_review_sends_claim_and_rubric_to_reviewer() {
        // The reviewer must actually SEE the work product + rubric, else the
        // gate silently reviews nothing. Record what generate() receives.
        use std::sync::{Arc, Mutex};
        struct Recording {
            received: Arc<Mutex<Vec<Message>>>,
            reply: String,
        }
        #[async_trait::async_trait]
        impl ChatModel for Recording {
            async fn generate(
                &self,
                messages: &[Message],
                _opts: &ModelOptions,
            ) -> Result<Message, Error> {
                *self.received.lock().unwrap() = messages.to_vec();
                Ok(Message::assistant(self.reply.clone()))
            }
            fn stream(
                &self,
                _m: &[Message],
                _o: &ModelOptions,
            ) -> Result<MessageStream, Error> {
                Err(Error::Unsupported("no stream".into()))
            }
        }
        let received = Arc::new(Mutex::new(Vec::new()));
        let rec = Recording {
            received: received.clone(),
            reply: "VERDICT: PASS".into(),
        };
        verify_via_review(&rec, "WORK_PRODUCT_42", "RUBRIC_99")
            .await
            .unwrap();
        let msgs = received.lock().unwrap().clone();
        assert_eq!(msgs.len(), 2, "expected system + user message");
        assert_eq!(msgs[0].role, Role::System);
        let user = msgs[1].content.as_str();
        assert!(user.contains("WORK_PRODUCT_42"), "claim must reach reviewer");
        assert!(user.contains("RUBRIC_99"), "rubric must reach reviewer");
    }

    #[test]
    fn parse_verdict_violated_contract_does_not_trust_embedded_pass() {
        // First line merely CONTAINS "VERDICT: PASS" but leads with a defect
        // admission — a violated contract must NOT paper over the fail marker.
        // Falls through to keyword scan → "defect" → FAIL.
        assert!(!parse_verdict("The work has defects. VERDICT: PASS"));
    }

    #[test]
    fn parse_verdict_body_fail_marker_overrides_clean_pass_line() {
        // A clean "VERDICT: PASS" first line followed by a defect admission in
        // the body is a conflicting signal → FAIL (adversarial).
        assert!(!parse_verdict("VERDICT: PASS\nhowever, one defect remains."));
        // A clean PASS with no fail marker in the body still passes.
        assert!(parse_verdict("VERDICT: PASS\nSolid implementation."));
    }

    #[tokio::test]
    async fn verify_via_review_propagates_reviewer_error() {
        // The main production failure mode: reviewer model 401 / rate-limited /
        // network error. The gate must surface that as an Err, not swallow it
        // into a default verdict (silently pass/fail on an unavailable reviewer).
        struct FailingReviewer;
        #[async_trait::async_trait]
        impl ChatModel for FailingReviewer {
            async fn generate(
                &self,
                _messages: &[Message],
                _opts: &ModelOptions,
            ) -> Result<Message, Error> {
                Err(Error::Network("reviewer 429 rate limited".into()))
            }
            fn stream(
                &self,
                _m: &[Message],
                _o: &ModelOptions,
            ) -> Result<MessageStream, Error> {
                Err(Error::Unsupported("no stream".into()))
            }
        }
        let err = verify_via_review(&FailingReviewer, "the work", "check it")
            .await
            .expect_err("reviewer error must propagate");
        assert!(err.contains("verify review generate"), "got: {err}");
        assert!(err.contains("429"), "underlying cause preserved, got: {err}");
    }
}
