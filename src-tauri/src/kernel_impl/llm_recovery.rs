//! LLM error classification + retry policy for tool-call recovery (C7).
//!
//! Ported from deer-flow's `LLMErrorHandlingMiddleware` (the semantics, not the
//! LangGraph middleware plumbing): the ReactAgent run loop and ChatModel
//! classify a model error into [`Retryable`](LlmErrorKind::Retryable) vs
//! [`Fatal`](LlmErrorKind::Fatal), retry transient errors with exponential
//! backoff, and degrade to a graceful Done instead of bubbling the error up to
//! kill the whole agent — the "LLM rate-limit / provider 400" pain point.
//!
//! The functions here are pure and unit-tested directly. The ChatModel wires
//! them into the send path (retry transient send failures); the run loop turns
//! a terminal Fatal into a degraded Done with an honest, specific message.

use std::time::Duration;

use kernel_core::Error;

/// Maximum total attempts (first try + retries). Matches deer-flow
/// `retry_max_attempts = 3`.
pub const MAX_ATTEMPTS: u32 = 3;

/// Base backoff before the first retry. Subsequent retries double, capped at
/// [`RETRY_CAP`].
pub const RETRY_BASE: Duration = Duration::from_millis(1000);

/// Upper bound on a single retry delay (deer-flow `retry_cap_delay_ms = 8000`).
pub const RETRY_CAP: Duration = Duration::from_millis(8000);

/// Whether an error deserves another attempt, and — if not — why not, so the
/// run loop can surface an honest, specific message instead of a raw stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmErrorKind {
    /// Transient (network/timeout/5xx/429/busy) — worth retrying with backoff.
    Retryable,
    /// Permanent — stop and degrade.
    Fatal(FatalReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatalReason {
    /// Upstream circuit breaker is open — retrying now is futile, the breaker
    /// already decided the endpoint is down. The run loop must not re-record
    /// this as a failure (the breaker drove the decision).
    Circuit,
    /// Account out of quota / billing blocked — user action needed.
    Quota,
    /// Monthly cost budget reached (v1.2 T10 hard limit) — the agent halts
    /// before spending another turn rather than burning past the cap.
    Budget,
    /// Bad/missing credentials — user action needed.
    Auth,
    /// Anything else not worth retrying (4xx other than 429, decode errors...).
    Generic,
}

/// HTTP status codes treated as retriable. Kept in sync with
/// `cost::circuit_breaker::should_failover` so retry policy and circuit policy
/// agree on what "transient" means.
const RETRIABLE_STATUS: &[u16] = &[408, 425, 429, 500, 502, 503, 504];

const QUOTA_PATTERNS: &[&str] = &[
    "insufficient_quota",
    "quota",
    "billing",
    "credit",
    "payment",
    "余额不足",
    "超出限额",
    "额度不足",
    "欠费",
];

const AUTH_PATTERNS: &[&str] = &[
    "unauthorized",
    "invalid api key",
    "invalid_api_key",
    "forbidden",
    "permission denied",
    "access denied",
    "authentication",
    "无权",
    "未授权",
];

const BUSY_PATTERNS: &[&str] = &[
    "server busy",
    "temporarily unavailable",
    "try again later",
    "please retry",
    "please try again",
    "overloaded",
    "high demand",
    "rate limit",
    "负载较高",
    "服务繁忙",
    "稍后重试",
    "请稍后重试",
];

fn matches_any(lowered: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| lowered.contains(p))
}

/// Pull the trailing HTTP status out of the ChatModel's `Model("LLM ... failed:
/// <status>")` messages. Returns None for non-status messages (circuit open,
/// decode errors, etc.). reqwest's `StatusCode` Display starts with the 3-digit
/// code, so we parse the leading digits of the tail after the last "failed:".
fn extract_status(msg: &str) -> Option<u16> {
    let tail = msg.rsplit("failed:").next()?.trim();
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits
        .parse::<u16>()
        .ok()
        .filter(|&s| (100..600).contains(&s))
}

/// Classify a kernel_core::Error produced by the model layer.
pub fn classify_llm_error(err: &Error) -> LlmErrorKind {
    let msg = err.to_string();
    let lowered = msg.to_ascii_lowercase();

    // Circuit-open is the breaker's verdict — never retry, never re-record.
    if lowered.contains("circuit open") {
        return LlmErrorKind::Fatal(FatalReason::Circuit);
    }
    // Quota / auth are user-actionable and permanent for this run.
    if matches_any(&lowered, QUOTA_PATTERNS) {
        return LlmErrorKind::Fatal(FatalReason::Quota);
    }
    if matches_any(&lowered, AUTH_PATTERNS) {
        return LlmErrorKind::Fatal(FatalReason::Auth);
    }
    // Network blips (connect/timeout/read) are always transient.
    if matches!(err, Error::Network(_)) {
        return LlmErrorKind::Retryable;
    }
    // Status-bearing model errors: retry the transient codes, fail the rest.
    if let Some(status) = extract_status(&msg) {
        if RETRIABLE_STATUS.contains(&status) {
            return LlmErrorKind::Retryable;
        }
        return LlmErrorKind::Fatal(FatalReason::Generic);
    }
    // No status, but provider said it's busy/overloaded → retry.
    if matches_any(&lowered, BUSY_PATTERNS) {
        return LlmErrorKind::Retryable;
    }
    LlmErrorKind::Fatal(FatalReason::Generic)
}

/// Backoff before the (attempt+1)-th request. Exponential from [`RETRY_BASE`],
/// capped at [`RETRY_CAP`]. `attempt` is 1-based: the delay before the 2nd
/// request overall.
pub fn retry_delay(attempt: u32) -> Duration {
    let exp = attempt.saturating_sub(1).min(6); // cap shift to avoid overflow
    let scaled = RETRY_BASE
        .checked_mul(2u32.saturating_pow(exp))
        .unwrap_or(RETRY_CAP);
    scaled.min(RETRY_CAP)
}

/// Should the caller retry given the current attempt count (1-based)? True when
/// the error is Retryable AND we haven't reached [`MAX_ATTEMPTS`].
pub fn should_retry(err: &Error, attempt: u32) -> bool {
    matches!(classify_llm_error(err), LlmErrorKind::Retryable) && attempt < MAX_ATTEMPTS
}

/// Honest, specific user-facing copy for a fatal failure — what the run loop
/// puts in the degraded Done's `output_summary`.
pub fn fatal_user_message(reason: FatalReason) -> &'static str {
    match reason {
        FatalReason::Circuit => {
            "The model provider is currently unavailable (circuit breaker engaged after repeated failures). Please wait a moment and try again."
        }
        FatalReason::Quota => {
            "The model provider rejected the request: account out of quota or billing unavailable. Please check your provider account and retry."
        }
        FatalReason::Budget => {
            "Monthly cost budget reached — the agent halted before exceeding the spending cap. Raise the budget in Settings \u{2192} Cost to continue."
        }
        FatalReason::Auth => {
            "The model provider rejected the request: authentication failed. Please check your API key in Settings \u{2192} Providers and retry."
        }
        FatalReason::Generic => {
            "The model request failed and could not be recovered after retries. Please rephrase or retry."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_error_is_retryable() {
        assert_eq!(
            classify_llm_error(&Error::Network("connection reset".into())),
            LlmErrorKind::Retryable
        );
    }

    #[test]
    fn circuit_open_is_fatal_circuit() {
        assert_eq!(
            classify_llm_error(&Error::Model("upstream circuit open: https://x".into())),
            LlmErrorKind::Fatal(FatalReason::Circuit)
        );
    }

    #[test]
    fn rate_limit_429_is_retryable() {
        assert_eq!(
            classify_llm_error(&Error::Model(
                "LLM stream failed: 429 Too Many Requests".into()
            )),
            LlmErrorKind::Retryable
        );
    }

    #[test]
    fn server_503_is_retryable() {
        assert_eq!(
            classify_llm_error(&Error::Model("LLM call failed: 503".into())),
            LlmErrorKind::Retryable
        );
    }

    #[test]
    fn client_400_is_fatal_generic() {
        assert_eq!(
            classify_llm_error(&Error::Model("LLM call failed: 400 Bad Request".into())),
            LlmErrorKind::Fatal(FatalReason::Generic)
        );
    }

    #[test]
    fn auth_401_is_fatal_auth() {
        assert_eq!(
            classify_llm_error(&Error::Model("LLM call failed: 401 unauthorized".into())),
            LlmErrorKind::Fatal(FatalReason::Auth)
        );
    }

    #[test]
    fn quota_message_is_fatal_quota() {
        assert_eq!(
            classify_llm_error(&Error::Model("insufficient_quota: billing issue".into())),
            LlmErrorKind::Fatal(FatalReason::Quota)
        );
    }

    #[test]
    fn busy_pattern_is_retryable() {
        assert_eq!(
            classify_llm_error(&Error::Model("server busy, try again later".into())),
            LlmErrorKind::Retryable
        );
    }

    #[test]
    fn decode_error_is_fatal_generic() {
        assert_eq!(
            classify_llm_error(&Error::Model("decode: bad json".into())),
            LlmErrorKind::Fatal(FatalReason::Generic)
        );
    }

    #[test]
    fn retry_delay_grows_then_caps() {
        assert_eq!(retry_delay(1), Duration::from_millis(1000));
        assert_eq!(retry_delay(2), Duration::from_millis(2000));
        assert_eq!(retry_delay(3), Duration::from_millis(4000));
        assert_eq!(retry_delay(10), Duration::from_millis(8000)); // capped
    }

    #[test]
    fn should_retry_respects_max_attempts() {
        // Retryable + early attempt → retry.
        assert!(should_retry(&Error::Network("x".into()), 1));
        assert!(should_retry(&Error::Network("x".into()), 2));
        // Retryable but at MAX_ATTEMPTS → stop.
        assert!(!should_retry(&Error::Network("x".into()), MAX_ATTEMPTS));
        // Fatal → never retry regardless of attempt.
        assert!(!should_retry(&Error::Model("circuit open".into()), 1));
    }

    #[test]
    fn fatal_messages_are_distinct_and_specific() {
        assert!(fatal_user_message(FatalReason::Quota).contains("quota"));
        assert!(fatal_user_message(FatalReason::Budget).contains("budget"));
        assert!(fatal_user_message(FatalReason::Auth).contains("API key"));
        assert!(fatal_user_message(FatalReason::Circuit).contains("circuit breaker"));
        assert!(fatal_user_message(FatalReason::Generic).contains("retries"));
    }
}
