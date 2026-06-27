//! B3 — per-LLM-call timing observability + slow-turn detection. This is the
//! "TimingChecker" half of the trace moat (the other half is the per-call timing
//! breakdown persisted to `llm_traces` — `ttfb_ms` / `stream_ms`).
//!
//! The design mirrors the eino Handler "five timing points" model
//! (before-request → first-byte → stream-chunks → completion → on-error): we
//! capture the wall-clock points at the ChatModel call site and DERIVE two
//! diagnostically meaningful intervals from them:
//!   - `ttfb_ms`  (time-to-first-byte): request send → first response signal.
//!     High ttfb = the model is slow to *start* (queueing, auth, cold model) —
//!     distinct from slow output.
//!   - `stream_ms`: first-byte → completion. High stream_ms = slow to *produce*
//!     output (long generation, network throttling).
//!
//! [`TimingChecker`] then flags turns whose total latency or ttfb crosses a
//! threshold, surfacing a [`TimingWarning`] the agent loop logs. It is pure +
//! synchronous so it's fully unit-testable with no clock/network.

/// One timing observation the checker surfaced. `kind` is `slow_turn` (total
/// latency over threshold) or `slow_ttfb` (first-byte over half the threshold —
/// a disproportionately long "model thinking" phase). `message` is the human
/// log line.
#[derive(Debug, Clone, PartialEq)]
pub struct TimingWarning {
    pub kind: &'static str,
    pub latency_ms: u64,
    pub ttfb_ms: Option<u64>,
    pub threshold_ms: u64,
    pub message: String,
}

/// Stateless checker for slow LLM turns. Holds only the configured thresholds;
/// [`TimingChecker::check`] is a pure function of (latency, ttfb). Cloning is
/// cheap, so one shared instance is held inside the ChatModel behind an `Arc`.
#[derive(Debug, Clone, Copy)]
pub struct TimingChecker {
    /// A turn whose total latency (request → completion) exceeds this is
    /// flagged `slow_turn`. Industry UX research: ~60s is where a user
    /// perceives an agent as hung (the threshold Phoenix/Arize default to for
    /// "slow span" surfacing). Tunable; 0 disables.
    slow_turn_threshold_ms: u64,
}

impl TimingChecker {
    /// 60s default — the "perceived as hung" point. Exposed so callers/tests
    /// can reference the same default rather than hardcoding.
    pub const DEFAULT_SLOW_TURN_MS: u64 = 60_000;

    /// Construct with an explicit slow-turn threshold (ms). 0 = disabled.
    pub fn new(slow_turn_threshold_ms: u64) -> Self {
        Self {
            slow_turn_threshold_ms,
        }
    }

    /// Construct with the default 60s threshold.
    pub fn default_threshold() -> Self {
        Self::new(Self::DEFAULT_SLOW_TURN_MS)
    }

    /// Disabled checker — never flags. The ad-hoc/test default.
    pub fn disabled() -> Self {
        Self::new(0)
    }

    pub fn slow_turn_threshold_ms(&self) -> u64 {
        self.slow_turn_threshold_ms
    }

    /// Whether this checker will ever flag anything. False when the threshold
    /// is 0 (disabled) — lets the agent loop skip the check entirely.
    pub fn is_enabled(&self) -> bool {
        self.slow_turn_threshold_ms > 0
    }

    /// Inspect one LLM call's timing and return a warning if it crossed a
    /// threshold. Returns `None` when disabled, or when latency is within
    /// bounds. `slow_turn` (total latency) takes precedence over `slow_ttfb`
    /// so a truly hung turn reports the most actionable signal first.
    ///
    /// `ttfb_ms` is optional: a pure network failure has no first-byte, so
    /// only the total-latency check applies there.
    pub fn check(&self, latency_ms: u64, ttfb_ms: Option<u64>) -> Option<TimingWarning> {
        if !self.is_enabled() {
            return None;
        }
        // Slow turn: total request→completion over threshold. This is the
        // primary "is the agent hung?" signal.
        if latency_ms > self.slow_turn_threshold_ms {
            return Some(TimingWarning {
                kind: "slow_turn",
                latency_ms,
                ttfb_ms,
                threshold_ms: self.slow_turn_threshold_ms,
                message: format!(
                    "slow LLM turn: {latency_ms}ms ({} ttfb) > {}ms threshold",
                    ttfb_ms.map_or("-".to_string(), |t| format!("{t}ms")),
                    self.slow_turn_threshold_ms
                ),
            });
        }
        // Slow TTFB: first-byte took longer than HALF the slow-turn threshold.
        // A turn under the total threshold but with a huge first-byte gap means
        // the model queued/auth-stalled — worth surfacing even though the turn
        // overall was "fast enough". Catches the auth/quota-stall failure mode
        // (e.g. session 41f2ddca's 0.8s-to-400 would show ttfb≈800ms, under
        // threshold but the slow_ttfb path catches misclassified "fast" 400s
        // when ttfb is disproportionate).
        let ttfb_threshold = self.slow_turn_threshold_ms / 2;
        if let Some(ttfb) = ttfb_ms {
            if ttfb > ttfb_threshold {
                return Some(TimingWarning {
                    kind: "slow_ttfb",
                    latency_ms,
                    ttfb_ms: Some(ttfb),
                    threshold_ms: ttfb_threshold,
                    message: format!(
                        "slow LLM first-byte (ttfb): {ttfb}ms > {ttfb_threshold}ms (model stalled before producing output; total turn {latency_ms}ms)"
                    ),
                });
            }
        }
        None
    }
}

impl Default for TimingChecker {
    fn default() -> Self {
        Self::default_threshold()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_threshold_yields_no_warning() {
        let c = TimingChecker::new(60_000);
        assert_eq!(c.check(1_000, Some(300)), None);
        assert_eq!(c.check(0, None), None);
    }

    #[test]
    fn latency_over_threshold_flags_slow_turn() {
        let c = TimingChecker::new(60_000);
        let w = c
            .check(75_000, Some(1_000))
            .expect("75s > 60s threshold → slow_turn");
        assert_eq!(w.kind, "slow_turn");
        assert_eq!(w.latency_ms, 75_000);
        assert_eq!(w.threshold_ms, 60_000);
        assert_eq!(w.ttfb_ms, Some(1_000));
        assert!(w.message.contains("slow LLM turn"));
    }

    #[test]
    fn slow_turn_takes_precedence_over_slow_ttfb() {
        // Both total latency AND ttfb are over their thresholds: the total
        // (slow_turn) must win — it's the most actionable "is it hung?" signal.
        let c = TimingChecker::new(60_000);
        let w = c.check(120_000, Some(80_000)).expect("flagged");
        assert_eq!(w.kind, "slow_turn", "slow_turn beats slow_ttfb");
    }

    #[test]
    fn disproportionate_ttfb_flags_slow_ttfb_under_total_threshold() {
        // Turn total (30s) is under the 60s threshold, but ttfb (25s) is way
        // over the half-threshold (30s) — wait, 25 < 30. Use ttfb=35s.
        let c = TimingChecker::new(60_000);
        // ttfb 35s > 30s half-threshold, total 40s < 60s → slow_ttfb.
        let w = c.check(40_000, Some(35_000)).expect("flagged");
        assert_eq!(w.kind, "slow_ttfb");
        assert_eq!(w.threshold_ms, 30_000);
        assert!(w.message.contains("first-byte"));
    }

    #[test]
    fn network_failure_with_no_ttfb_only_checks_total_latency() {
        // A pure network failure has no first-byte (ttfb None). Only the
        // total-latency check applies; a sub-threshold network failure is not
        // flagged (there's no ttfb to be slow).
        let c = TimingChecker::new(60_000);
        assert_eq!(c.check(500, None), None);
        // But a network-failure that somehow took 90s (DNS/timeout stall) IS
        // flagged as slow_turn — ttfb absent is fine for that path.
        let w = c.check(90_000, None).expect("flagged");
        assert_eq!(w.kind, "slow_turn");
        assert_eq!(w.ttfb_ms, None);
    }

    #[test]
    fn disabled_checker_never_flags() {
        let c = TimingChecker::disabled();
        assert!(!c.is_enabled());
        // Even an absurdly slow turn is silent when disabled.
        assert_eq!(c.check(u64::MAX, Some(u64::MAX)), None);
    }

    #[test]
    fn default_threshold_is_sixty_seconds() {
        let c = TimingChecker::default();
        assert_eq!(c.slow_turn_threshold_ms(), 60_000);
        assert!(c.is_enabled());
        // 59_999 just under, 60_001 just over (strictly greater-than).
        assert_eq!(c.check(59_999, None), None);
        assert!(c.check(60_001, None).is_some());
    }

    #[test]
    fn boundary_strictly_greater_than() {
        // Exactly at threshold is NOT slow (> is strict), so the boundary value
        // itself is clean. Pins the off-by-one: `latency_ms > threshold`.
        let c = TimingChecker::new(10_000);
        assert_eq!(
            c.check(10_000, None),
            None,
            "exactly at threshold is not slow"
        );
        assert!(c.check(10_001, None).is_some(), "one ms over is slow");
    }
}
