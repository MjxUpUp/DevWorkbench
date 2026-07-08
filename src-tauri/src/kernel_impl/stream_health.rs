//! Stream Health — classification + degradation policy for LLM stream failures.
//!
//! ## Why this exists
//!
//! Before this module, `llm_traces.error_kind` for stream failures stored only
//! coarse labels (`"stream"`, `"idle"`). That made the failure-mode dashboard
//! useless: when session 64388195 in conversation cfa53764 was truncated, we
//! couldn't tell whether it was a model thinking timeout, a proxy buffer
//! exhaustion, or a TCP reset. Without a root-cause label, every retry strategy
//! is guesswork.
//!
//! ## What this module provides
//!
//! 1. `RootCause` — enumerated failure mode. Five kinds based on the signal
//!    we can observe at the stream layer (status_code + chunk timing +
//!    bytes-before-EOF). These map 1:1 to what the upstream API / proxy /
//!    network can tell us at the byte boundary.
//! 2. `classify_stream_failure` — pure function that maps (status_code, secs
//!    since last byte, bytes received so far, was_message_stop_seen) to a
//!    `RootCause`. Used in `anthropic_chat_model.rs` to label `error_kind`
//!    more precisely.
//! 3. `degrade_policy` — maps `RootCause` to a recommended retry / degrade
//!    action. The actual retry loop lives in `llm_recovery.rs`; this module
//!    only owns the policy table.
//!
//! ## Root cause taxonomy
//!
//! - `ModelTimeout` — chunk gap > `STREAM_IDLE_TIMEOUT_SECS` (90s) while bytes
//!   were arriving before. The model went silent mid-stream (thinking stall,
//!   or upstream rate-limit). Retries with same prompt usually succeed.
//! - `NetworkReset` — TCP connection died mid-stream (bytes were flowing then
//!   EOF without `message_stop`). Often a proxy or load-balancer idle-kill.
//!   Retry without resume is fine.
//! - `ProxyBufferFull` — many bytes arrived then suddenly EOF. The upstream
//!   buffer (nginx/Cloudflare) overflowed. Retry with shorter output target
//!   (`max_tokens` lower).
//! - `ContextExhausted` — request was rejected before streaming started
//!   (4xx status) AND the body indicates token overflow. Surface as final
//!   failure — retry won't help.
//! - `PrematureEof` — bytes received but `message_stop` never arrived AND
//!   the chunk gap was well under `STREAM_IDLE_TIMEOUT_SECS`. Most likely
//!   the upstream cut the stream without warning. Retry without resume.

/// Stream failure root cause. Categorizes the symptom at the byte boundary;
/// does NOT diagnose the underlying system fault (which is opaque from here).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RootCause {
    /// Chunk gap > idle timeout while bytes were previously arriving. Model
    /// went silent mid-stream (thinking stall or upstream rate-limit).
    ModelTimeout,
    /// TCP connection died mid-stream. Common: proxy / LB idle-kill or
    /// network reset. Retry with same prompt usually succeeds.
    NetworkReset,
    /// Many bytes received then sudden EOF without `message_stop`. Upstream
    /// proxy buffer overflowed mid-flight. Retry with shorter output target.
    ProxyBufferFull,
    /// Request rejected pre-stream (4xx) AND the body indicates token overflow.
    /// Final failure — retry with same prompt fails identically.
    ContextExhausted,
    /// Bytes received but `message_stop` never arrived AND the chunk gap was
    /// well under the idle timeout. Upstream cut the stream cleanly without
    /// a closing event. Retry without resume is fine.
    PrematureEof,
}

impl RootCause {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ModelTimeout => "model_timeout",
            Self::NetworkReset => "network_reset",
            Self::ProxyBufferFull => "proxy_buffer_full",
            Self::ContextExhausted => "context_exhausted",
            Self::PrematureEof => "premature_eof",
        }
    }

    /// Whether the failure is worth retrying with the same prompt. Read by
    /// `llm_recovery.rs` to decide between retry and degrade. Kept here so
    /// the policy table is in one place.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::ModelTimeout => true,
            Self::NetworkReset => true,
            Self::ProxyBufferFull => true,
            Self::ContextExhausted => false,
            Self::PrematureEof => true,
        }
    }

    /// Suggested output cap (max_tokens) reduction when retrying. Returns
    /// `None` if no reduction is recommended. Used by the retry loop to
    /// shrink the next attempt's `max_tokens` parameter.
    pub fn suggested_max_tokens_reduction(&self) -> Option<u32> {
        match self {
            // Buffer overflow — retry shorter so the upstream proxy is happy.
            Self::ProxyBufferFull => Some(2048),
            _ => None,
        }
    }
}

/// Inputs to `classify_stream_failure`. All fields are best-effort
/// observations — the classifier prefers the most specific cause that
/// the signals support.
#[derive(Clone, Copy, Debug)]
pub struct StreamSignals {
    /// HTTP status code. `None` if the request never got a response
    /// (DNS failure, connection refused, etc).
    pub status_code: Option<u16>,
    /// Seconds elapsed between the last byte received and EOF / error.
    /// `None` if EOF happened immediately (no bytes ever arrived).
    pub idle_secs: Option<u64>,
    /// Total bytes received before EOF / error. Used to distinguish a
    /// mid-stream cut from a pre-stream failure.
    pub bytes_received: usize,
    /// Whether the LLM emitted a `message_stop` SSE event before EOF. When
    /// `false`, the stream ended without a proper closing event — the
    /// classic "truncated stream" symptom.
    pub saw_message_stop: bool,
    /// Optional pre-classified context-exhausted hint, set when the
    /// `error_kind` field already knew (e.g. Anthropic returned 400 with
    /// "prompt is too long"). Avoids re-classifying at the byte layer.
    pub context_overflow_hint: bool,
}

/// Classify a stream failure into a `RootCause` from observed signals. Pure.
///
/// The order of checks matters: the classifier applies the most specific
/// signal first (context overflow pre-stream → `ContextExhausted`) and
/// falls back to the byte-gap heuristic for in-flight failures.
pub fn classify_stream_failure(s: &StreamSignals) -> RootCause {
    // Pre-stream rejection with overflow hint → context exhausted. Retry
    // with same prompt fails identically, so the retry loop will skip.
    if s.context_overflow_hint {
        return RootCause::ContextExhausted;
    }
    // 4xx with no overflow hint: still treat as final — server rejected
    // the request, retry won't change anything.
    if let Some(code) = s.status_code {
        if (400..500).contains(&code) {
            return RootCause::ContextExhausted;
        }
    }

    // No bytes ever arrived AND a 5xx → server-side issue, classify as
    // network reset (the client never got to see the stream).
    if s.bytes_received == 0 {
        return RootCause::NetworkReset;
    }

    // Bytes were flowing. Now distinguish:
    let idle = s.idle_secs.unwrap_or(0);
    if !s.saw_message_stop && idle >= 60 {
        // Long silence after partial output → model thinking stall.
        return RootCause::ModelTimeout;
    }
    if !s.saw_message_stop && idle < 5 && s.bytes_received > 8192 {
        // Many bytes then sudden EOF with no warning → proxy buffer overflow.
        return RootCause::ProxyBufferFull;
    }
    if !s.saw_message_stop {
        // Generic premature EOF. Most likely a clean upstream cut.
        return RootCause::PrematureEof;
    }
    // saw_message_stop but stream still errored → almost impossible,
    // but treat as PrematureEof for safety.
    RootCause::PrematureEof
}

/// Suggested degrade action when retries are exhausted. Mirrors the
/// pre-P2 behavior (return a degraded final answer instead of an error)
/// but adds explicit policy per root cause. Pure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DegradeAction {
    /// Return what we have with a "[truncated]" marker so the user sees
    /// the partial output and can decide to retry manually.
    ReturnPartial,
    /// Surface the error to the user — retrying won't help, and the
    /// partial output is misleading.
    FailFinal,
    /// Same as FailFinal, but with a hint about what to change (e.g.
    /// shorten the prompt or switch model).
    FailWithHint(&'static str),
}

pub fn degrade_policy(cause: RootCause, emitted_any_output: bool) -> DegradeAction {
    if !emitted_any_output {
        // Nothing to show — fail cleanly.
        return match cause {
            RootCause::ContextExhausted => {
                DegradeAction::FailWithHint("prompt too long — shorten or compact")
            }
            _ => DegradeAction::FailFinal,
        };
    }
    // Partial output exists — let the user see it.
    match cause {
        RootCause::ContextExhausted => DegradeAction::ReturnPartial,
        _ => DegradeAction::ReturnPartial,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals(
        status: Option<u16>,
        idle_secs: Option<u64>,
        bytes: usize,
        stop: bool,
        hint: bool,
    ) -> StreamSignals {
        StreamSignals {
            status_code: status,
            idle_secs,
            bytes_received: bytes,
            saw_message_stop: stop,
            context_overflow_hint: hint,
        }
    }

    #[test]
    fn context_overflow_hint_wins() {
        let s = signals(Some(400), None, 0, false, true);
        assert_eq!(
            classify_stream_failure(&s),
            RootCause::ContextExhausted
        );
    }

    #[test]
    fn any_4xx_is_context_exhausted() {
        let s = signals(Some(429), Some(1), 100, false, false);
        assert_eq!(
            classify_stream_failure(&s),
            RootCause::ContextExhausted
        );
    }

    #[test]
    fn no_bytes_is_network_reset() {
        let s = signals(Some(503), Some(30), 0, false, false);
        assert_eq!(classify_stream_failure(&s), RootCause::NetworkReset);
    }

    #[test]
    fn long_idle_after_partial_is_model_timeout() {
        let s = signals(Some(200), Some(75), 5000, false, false);
        assert_eq!(classify_stream_failure(&s), RootCause::ModelTimeout);
    }

    #[test]
    fn many_bytes_short_idle_is_proxy_buffer() {
        let s = signals(Some(200), Some(2), 12000, false, false);
        assert_eq!(classify_stream_failure(&s), RootCause::ProxyBufferFull);
    }

    #[test]
    fn partial_short_idle_no_stop_is_premature_eof() {
        let s = signals(Some(200), Some(2), 500, false, false);
        assert_eq!(classify_stream_failure(&s), RootCause::PrematureEof);
    }

    #[test]
    fn retryable_table() {
        assert!(RootCause::ModelTimeout.is_retryable());
        assert!(RootCause::NetworkReset.is_retryable());
        assert!(RootCause::ProxyBufferFull.is_retryable());
        assert!(RootCause::PrematureEof.is_retryable());
        assert!(!RootCause::ContextExhausted.is_retryable());
    }

    #[test]
    fn buffer_full_suggests_shrink() {
        assert_eq!(
            RootCause::ProxyBufferFull.suggested_max_tokens_reduction(),
            Some(2048)
        );
        assert_eq!(
            RootCause::ModelTimeout.suggested_max_tokens_reduction(),
            None
        );
    }

    #[test]
    fn degrade_no_output_fails_cleanly() {
        assert_eq!(
            degrade_policy(RootCause::ModelTimeout, false),
            DegradeAction::FailFinal
        );
        // ContextExhausted with no output → hint about shortening prompt.
        match degrade_policy(RootCause::ContextExhausted, false) {
            DegradeAction::FailWithHint(h) => assert!(h.contains("shorten")),
            other => panic!("expected hint, got {:?}", other),
        }
    }

    #[test]
    fn degrade_with_output_returns_partial() {
        assert_eq!(
            degrade_policy(RootCause::ModelTimeout, true),
            DegradeAction::ReturnPartial
        );
    }
}