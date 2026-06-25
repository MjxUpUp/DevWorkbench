//! Upstream circuit breaker — protects against cascading failures when the GLM
//! endpoint (or any model gateway) starts failing. Ported 1:1 from AgentFare's
//! `packages/hook/src/failover.ts`, adapted to Rust's thread model: the TS
//! single-threaded `Map` becomes a `Mutex<HashMap>`, and the admit→record
//! sequence stays coherent because each decision holds the lock for its whole
//! critical section.
//!
//! Per-host state machine: `Closed` (normal) → `Open` (tripped, fast-fail) after
//! `failure_threshold` consecutive failures → `HalfOpen` (one probe allowed
//! after `cooldown`) → `Closed` on probe success, or back to `Open` on probe
//! failure. The breaker is process-local and in-memory (like AgentFare's
//! module singleton): state resets on restart. Persisting across restarts is a
//! later concern — for a local workbench a cold start clearing a stuck circuit
//! is acceptable.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Per-host circuit state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests flow through.
    Closed,
    /// Tripped — requests are short-circuited until the cooldown elapses.
    Open,
    /// Cooldown elapsed; a bounded number of probe requests are allowed to
    /// test whether the upstream has recovered.
    HalfOpen,
}

#[derive(Debug, Clone, Copy)]
pub struct CircuitBreakerConfig {
    /// Consecutive failures on a Closed host before tripping to Open.
    pub failure_threshold: u32,
    /// How long Open holds before allowing a HalfOpen probe.
    pub cooldown: Duration,
    /// Max inflight probes while HalfOpen.
    pub half_open_max: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        // AgentFare's DEFAULT_CIRCUIT_CONFIG: { failureThreshold: 5,
        // cooldownMs: 30_000, halfOpenMax: 1 }.
        Self {
            failure_threshold: 5,
            cooldown: Duration::from_secs(30),
            half_open_max: 1,
        }
    }
}

/// Mutable per-host accounting. `opened_at` is the instant the host entered
/// `Open` (refreshed on every (re)trip); used to test the cooldown.
#[derive(Debug, Clone, Copy)]
struct HostCircuit {
    state: CircuitState,
    failures: u32,
    opened_at: Instant,
    half_open_inflight: u32,
}

impl HostCircuit {
    fn fresh() -> Self {
        Self {
            state: CircuitState::Closed,
            failures: 0,
            // `Instant::now()` is only read relative to `elapsed()`, so the
            // initial value is irrelevant for a Closed host.
            opened_at: Instant::now(),
            half_open_inflight: 0,
        }
    }
}

/// Decide whether an upstream response warrants a failover / failure record.
/// Mirrors AgentFare `shouldFailover`: any 5xx, 429 (rate limit), 408 (timeout),
/// or an outright transport error counts. 4xx other than 408/429 are caller
/// bugs (bad request shape) — NOT upstream failures — so they do not trip the
/// breaker (tripping on a 400 would brick a misconfigured agent).
pub fn should_failover(status: Option<u16>, is_error: bool) -> bool {
    if is_error {
        return true;
    }
    match status {
        Some(s) if s >= 500 => true,
        Some(429) | Some(408) => true,
        _ => false,
    }
}

pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    hosts: Mutex<HashMap<String, HostCircuit>>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            hosts: Mutex::new(HashMap::new()),
        }
    }

    /// Atomic "admit + count" in ONE lock (F1). The HalfOpen `inflight < cap`
    /// check and the `inflight += 1` increment share a single critical section,
    /// so two concurrent probes can never both observe `0 < 1` and both
    /// increment. Closed admits freely (no inflight accounting); Open past
    /// cooldown flips to HalfOpen and counts itself as the first probe. This is
    /// the only gate the production request path uses.
    #[must_use]
    pub fn try_admit(&self, host: &str) -> bool {
        let mut hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        let c = hosts
            .entry(host.to_string())
            .or_insert_with(HostCircuit::fresh);
        match c.state {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => {
                if c.half_open_inflight < self.config.half_open_max {
                    c.half_open_inflight += 1;
                    true
                } else {
                    false
                }
            }
            CircuitState::Open => {
                if c.opened_at.elapsed() >= self.config.cooldown {
                    c.state = CircuitState::HalfOpen;
                    c.half_open_inflight = 1;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Record a successful request: any state collapses back to `Closed`,
    /// clearing failures and inflight probes. A HalfOpen probe succeeding is
    /// the signal that the upstream has recovered.
    pub fn record_success(&self, host: &str) {
        let mut hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = hosts.get_mut(host) {
            c.state = CircuitState::Closed;
            c.failures = 0;
            c.half_open_inflight = 0;
        }
    }

    /// Record a failed request. HalfOpen probe failure reopens immediately
    /// (refreshing the cooldown clock); Closed failure increments and trips at
    /// the threshold; Open stays Open (clock untouched).
    pub fn record_failure(&self, host: &str) {
        let mut hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        let c = hosts
            .entry(host.to_string())
            .or_insert_with(HostCircuit::fresh);
        match c.state {
            CircuitState::HalfOpen => {
                c.state = CircuitState::Open;
                c.opened_at = Instant::now();
                c.half_open_inflight = 0;
            }
            CircuitState::Closed => {
                c.failures += 1;
                if c.failures >= self.config.failure_threshold {
                    c.state = CircuitState::Open;
                    c.opened_at = Instant::now();
                }
            }
            CircuitState::Open => {}
        }
    }

    /// Record a probe outcome that is neither a success nor an upstream
    /// failure — a non-failover 4xx (caller error: bad request shape, a 401 on
    /// one call, etc.). The probe slot it occupied in HalfOpen must be released
    /// (`try_admit` incremented `half_open_inflight`), but the outcome carries
    /// no signal about upstream health, so the state is left in HalfOpen rather
    /// than collapsing to Closed (a 400 doesn't prove recovery) or tripping to
    /// Open (a 400 isn't an upstream failure). Without this, a single 400 during
    /// the HalfOpen probe leaks the slot and — under the default
    /// `half_open_max = 1` — wedges the circuit in HalfOpen permanently (no probe
    /// re-admitted, no path back to Open).
    pub fn record_probe_inconclusive(&self, host: &str) {
        let mut hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = hosts.get_mut(host) {
            if c.state == CircuitState::HalfOpen {
                c.half_open_inflight = c.half_open_inflight.saturating_sub(1);
            }
        }
    }

    /// Current state for `host` (Closed if never seen).
    pub fn state(&self, host: &str) -> CircuitState {
        self.hosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(host)
            .map(|c| c.state)
            .unwrap_or(CircuitState::Closed)
    }

    /// Number of hosts currently tripped (non-Closed) — observability hook.
    pub fn tripped_count(&self) -> usize {
        self.hosts
            .lock()
            .unwrap()
            .values()
            .filter(|c| c.state != CircuitState::Closed)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast(threshold: u32) -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: threshold,
            cooldown: Duration::from_secs(60),
            half_open_max: 1,
        }
    }

    #[test]
    fn closed_allows_until_threshold_then_opens() {
        let cb = CircuitBreaker::new(fast(3));
        let host = "glm";
        for _ in 0..3 {
            assert!(cb.try_admit(host), "should admit while closed");
            cb.record_failure(host);
        }
        assert_eq!(cb.state(host), CircuitState::Open);
        // Open inside cooldown → blocked.
        assert!(!cb.try_admit(host));
    }

    #[test]
    fn try_admit_counts_atomically_so_halfopen_second_probe_is_blocked() {
        // F1 regression: try_admit must judge + count under one lock. With
        // half_open_max=1, the first HalfOpen probe is admitted and counted;
        // the second must be rejected because inflight is already at the cap.
        // The old allow_request + on_attempt pair took the lock twice, so two
        // concurrent probes could both pass the `0 < 1` check before either
        // incremented. Single-threaded here verifies the counting contract;
        // concurrency safety is "one lock" by construction.
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            cooldown: Duration::from_millis(10),
            half_open_max: 1,
        });
        cb.record_failure("h");
        assert_eq!(cb.state("h"), CircuitState::Open);
        std::thread::sleep(Duration::from_millis(20));
        assert!(cb.try_admit("h"), "first HalfOpen probe admitted + counted");
        assert!(
            !cb.try_admit("h"),
            "second probe blocked: atomic admit already filled the slot"
        );
    }

    #[test]
    fn sub_threshold_failures_keep_closed() {
        let cb = CircuitBreaker::new(fast(3));
        cb.record_failure("h");
        cb.record_failure("h");
        assert_eq!(cb.state("h"), CircuitState::Closed);
    }

    #[test]
    fn success_resets_failures_and_closes() {
        let cb = CircuitBreaker::new(fast(2));
        cb.record_failure("h");
        cb.record_failure("h");
        assert_eq!(cb.state("h"), CircuitState::Open);
        cb.record_success("h");
        assert_eq!(cb.state("h"), CircuitState::Closed);
        // Failures cleared: needs another full threshold to trip again.
        cb.record_failure("h");
        assert_eq!(cb.state("h"), CircuitState::Closed);
    }

    #[test]
    fn open_transitions_to_halfopen_after_cooldown_and_admits_one_probe() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            cooldown: Duration::from_millis(10),
            half_open_max: 1,
        });
        cb.record_failure("h");
        assert_eq!(cb.state("h"), CircuitState::Open);
        std::thread::sleep(Duration::from_millis(20));
        // Past cooldown → flip to HalfOpen, admit + count the single probe.
        assert!(cb.try_admit("h"));
        assert_eq!(cb.state("h"), CircuitState::HalfOpen);
        // A second probe while one is inflight is blocked.
        assert!(!cb.try_admit("h"));
    }

    #[test]
    fn halfopen_success_closes() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            cooldown: Duration::from_millis(10),
            half_open_max: 1,
        });
        cb.record_failure("h");
        std::thread::sleep(Duration::from_millis(20));
        assert!(cb.try_admit("h"));
        cb.record_success("h");
        assert_eq!(cb.state("h"), CircuitState::Closed);
    }

    #[test]
    fn non_failover_4xx_in_half_open_releases_probe_slot() {
        // Regression: a non-failover 4xx (e.g. 400) during a HalfOpen probe used
        // to leak the probe slot — try_admit incremented half_open_inflight but
        // neither record_success nor record_failure ran, so under half_open_max=1
        // the circuit wedged in HalfOpen forever (no probe re-admitted, no path
        // back to Open). record_probe_inconclusive releases the slot.
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            cooldown: Duration::from_millis(10),
            half_open_max: 1,
        });
        cb.record_failure("h");
        std::thread::sleep(Duration::from_millis(20));
        assert!(cb.try_admit("h")); // Open → HalfOpen, probe admitted + counted
        assert_eq!(cb.state("h"), CircuitState::HalfOpen);
        assert!(!cb.try_admit("h"), "slot occupied: second probe blocked");
        // 400 is a caller error, not an upstream failure:
        assert!(!should_failover(Some(400), false));
        cb.record_probe_inconclusive("h");
        // Slot freed, still HalfOpen → a follow-up probe is admitted (not wedged).
        assert!(cb.try_admit("h"), "probe slot must be released, not leaked");
        assert_eq!(cb.state("h"), CircuitState::HalfOpen);
    }

    #[test]
    fn halfopen_failure_reopens_immediately() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            cooldown: Duration::from_millis(10),
            half_open_max: 1,
        });
        cb.record_failure("h");
        std::thread::sleep(Duration::from_millis(20));
        assert!(cb.try_admit("h"));
        cb.record_failure("h");
        assert_eq!(cb.state("h"), CircuitState::Open);
    }

    #[test]
    fn should_failover_covers_5xx_429_408_and_transport_errors() {
        assert!(should_failover(Some(500), false));
        assert!(should_failover(Some(503), false));
        assert!(should_failover(Some(429), false));
        assert!(should_failover(Some(408), false));
        assert!(should_failover(None, true));
        // Caller-side errors (4xx other than 408/429) are NOT upstream failures.
        assert!(!should_failover(Some(400), false));
        assert!(!should_failover(Some(404), false));
        assert!(!should_failover(Some(200), false));
    }

    #[test]
    fn tripped_count_reflects_open_hosts() {
        let cb = CircuitBreaker::new(fast(1));
        assert_eq!(cb.tripped_count(), 0);
        cb.record_failure("a");
        cb.record_failure("b");
        assert_eq!(cb.tripped_count(), 2);
    }

    #[test]
    fn hosts_are_isolated() {
        let cb = CircuitBreaker::new(fast(1));
        cb.record_failure("a");
        assert_eq!(cb.state("a"), CircuitState::Open);
        assert_eq!(cb.state("b"), CircuitState::Closed);
        assert!(cb.try_admit("b"));
    }
}
