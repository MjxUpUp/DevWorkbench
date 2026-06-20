//! Cost recording sink — the seam between GlmChatModel (which observes token
//! usage per request) and the `cost_records` table. GlmChatModel holds an
//! `Option<Arc<dyn CostSink>>`; when present, each completed request records its
//! usage + computed cost. `DbCostSink` writes to SQLite on a blocking thread
//! (fire-and-forget — a cost-write failure must never break the agent loop).

use std::sync::Arc;

use crate::cost::{agentfare, pricing};
use crate::db::DbState;
use crate::models::CostRecord;

/// Receives one cost record per model request. Implementations must be cheap to
/// share (held inside GlmChatModel behind an `Arc`) and must NOT propagate
/// errors into the caller — a failed cost write is logged, not fatal.
///
/// B5: the signature takes a `TokenUsage` (input/output + prompt-cache tiers)
/// instead of bare input/output, so the transparent cost breakdown is preserved
/// end-to-end. `cost_usd` is an optional override; 0.0 means "derive from the
/// pricing table" (the common path — callers meter usage and let the sink price
/// it). A non-zero value (e.g. a precomputed cost) is used as-is.
pub trait CostSink: Send + Sync {
    fn record(&self, model: &str, usage: pricing::TokenUsage, cost_usd: f64);
}

/// A CostSink that drops everything — the default when no DB/session context is
/// available (an ad-hoc agent without a session id, or tests).
#[derive(Debug, Default, Clone, Copy)]
pub struct NullCostSink;

impl CostSink for NullCostSink {
    fn record(&self, _: &str, _: pricing::TokenUsage, _: f64) {}
}

/// A CostSink writing to the `cost_records` table. `record` spawns a blocking
/// task so the synchronous rusqlite INSERT never stalls the async stream, and
/// swallows errors (logged at warn) so cost tracking can't crash the agent.
pub struct DbCostSink {
    db: DbState,
    agent_type: String,
    session_id: Option<String>,
}

impl DbCostSink {
    pub fn new(
        db: DbState,
        agent_type: impl Into<String>,
        session_id: Option<String>,
    ) -> Self {
        Self {
            db,
            agent_type: agent_type.into(),
            session_id,
        }
    }
}

impl CostSink for DbCostSink {
    fn record(&self, model: &str, usage: pricing::TokenUsage, cost_usd: f64) {
        // If the caller didn't precompute cost, derive the total from the
        // pricing table so a missed cost upstream still leaves an honest value
        // rather than a silent zero. The breakdown is re-derived at read time
        // (aggregate_costs), so only the total is persisted per row.
        let cost = if cost_usd > 0.0 {
            cost_usd
        } else {
            pricing::cost_breakdown(usage, pricing::pricing_for(model)).total()
        };
        let rec = CostRecord {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: self.session_id.clone(),
            agent_type: self.agent_type.clone(),
            model: model.to_string(),
            input_tokens: usage.input as i64,
            output_tokens: usage.output as i64,
            cache_read_tokens: usage.cache_read as i64,
            cache_write_tokens: usage.cache_write as i64,
            cost_usd: cost,
            recorded_at: chrono::Utc::now().to_rfc3339(),
        };
        let db = self.db.clone();
        // Fire-and-forget blocking write. DbCostSink is only ever constructed
        // on the agent path (inside a tokio runtime), so a runtime is present.
        tokio::task::spawn_blocking(move || match db.get() {
            Ok(conn) => {
                if let Err(e) = agentfare::insert_cost_record(&conn, &rec) {
                    log::warn!("[cost] insert_cost_record failed: {e}");
                }
            }
            Err(e) => log::warn!("[cost] db lock failed: {e}"),
        });
    }
}

/// Build a shared sink, or a `NullCostSink` when `db` is absent. Convenience for
/// the agent construction path (build_react_agent).
pub fn optional_shared(
    db: Option<DbState>,
    agent_type: &str,
    session_id: Option<String>,
) -> Arc<dyn CostSink> {
    match db {
        Some(db) => Arc::new(DbCostSink::new(db, agent_type, session_id)),
        None => Arc::new(NullCostSink),
    }
}

/// C2 per-dispatch cost counter. Wraps the parent's real sink (the DB sink, when
/// present) and a shared [`CostAccumulator`]: every `record` increments the
/// accumulator (so the SubAgentTool can attribute THIS dispatch's tokens + cost)
/// AND forwards to the inner sink (so cost_records + the dashboard aggregate are
/// unchanged — the parent turn's total still includes the child's calls). When
/// `inner` is None the counter tallies but persists nothing (tests / ad-hoc).
///
/// Installed by [`crate::kernel_impl::react_agent::GlmChatModel::fork_with_counting_cost`]
/// onto a per-dispatch forked model, so a fan-out's per-child cost is visible on
/// the multi-agent board — the anti-"10× cost" visibility the C2 design requires
/// (prerequisite B3/B5 cost infrastructure now in place).
pub struct CountingCostSink {
    inner: Option<Arc<dyn CostSink>>,
    accumulator: Arc<kernel_core::CostAccumulator>,
}

impl CountingCostSink {
    pub fn new(
        inner: Option<Arc<dyn CostSink>>,
        accumulator: Arc<kernel_core::CostAccumulator>,
    ) -> Self {
        Self { inner, accumulator }
    }
}

impl CostSink for CountingCostSink {
    fn record(&self, model: &str, usage: pricing::TokenUsage, cost_usd: f64) {
        // Derive the same honest total the DB sink would, so the accumulator's
        // cost matches what's persisted (a 0.0 caller means "price it here").
        let cost = if cost_usd > 0.0 {
            cost_usd
        } else {
            pricing::cost_breakdown(usage, pricing::pricing_for(model)).total()
        };
        self.accumulator.add(
            usage.input as u64,
            usage.output as u64,
            usage.cache_read as u64,
            usage.cache_write as u64,
            cost,
        );
        if let Some(inner) = &self.inner {
            inner.record(model, usage, cost_usd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_sink_silently_drops() {
        // The contract: no panic, no output, no side effect.
        NullCostSink.record("glm-4.6", pricing::TokenUsage::new(100, 200), 0.001);
    }

    #[test]
    fn optional_shared_returns_null_when_no_db() {
        // Without a DB the sink must still be callable (NullCostSink) — no
        // runtime required, no panic.
        let s = optional_shared(None, "x", None);
        s.record("glm-4.6", pricing::TokenUsage::new(1, 1), 0.0);
    }

    #[test]
    fn null_sink_accepts_cache_tiers() {
        // B5: the new TokenUsage signature must round-trip cache tiers without
        // panicking even on the no-op sink.
        NullCostSink.record(
            "claude-sonnet-4-5",
            pricing::TokenUsage { input: 10, output: 5, cache_read: 8, cache_write: 3 },
            0.0,
        );
    }

    /// Captures every record so a test can assert the inner sink still received
    /// what the counter forwarded (DB attribution must survive the wrap).
    #[derive(Default)]
    struct CapturingSink {
        records: std::sync::Mutex<Vec<(String, u32, u32, f64)>>,
    }

    impl CostSink for CapturingSink {
        fn record(&self, model: &str, usage: pricing::TokenUsage, cost_usd: f64) {
            self.records.lock().unwrap().push((
                model.to_string(),
                usage.input,
                usage.output,
                cost_usd,
            ));
        }
    }

    #[test]
    fn counting_sink_tallies_and_forwards_to_inner() {
        // C2 contract: the accumulator sums every record's tokens + cost so the
        // SubAgentTool can label one dispatch, AND the inner sink still receives
        // each record so cost_records / the dashboard total are unchanged.
        let inner = Arc::new(CapturingSink::default());
        let accumulator = Arc::new(kernel_core::CostAccumulator::new());
        let sink = CountingCostSink::new(Some(Arc::clone(&inner) as Arc<dyn CostSink>), Arc::clone(&accumulator));

        // glm-4.6: $1/M in, $3.2/M out. 1000 in + 500 out = $0.001 + $0.0016 = $0.0026.
        sink.record("glm-4.6", pricing::TokenUsage::new(1000, 500), 0.0);
        sink.record("glm-4.6", pricing::TokenUsage::new(200, 100), 0.0);

        let tally = accumulator.tally();
        assert_eq!(tally.input_tokens, 1200, "inputs accumulate");
        assert_eq!(tally.output_tokens, 600, "outputs accumulate");
        assert!(tally.cost_usd > 0.0, "cost derived from pricing when 0.0");
        // Forwarding: both records reached the inner sink verbatim.
        let recs = inner.records.lock().unwrap();
        assert_eq!(recs.len(), 2, "inner sink received every record (DB attribution preserved)");
        assert_eq!(recs[0].0, "glm-4.6");
    }

    #[test]
    fn counting_sink_tallies_with_no_inner_sink() {
        // A forked model with no parent DB sink (ad-hoc / test) must still tally
        // without panicking — the SubAgentTool gets a cost line either way.
        let accumulator = Arc::new(kernel_core::CostAccumulator::new());
        let sink = CountingCostSink::new(None, Arc::clone(&accumulator));
        sink.record("glm-4.6", pricing::TokenUsage::new(50, 25), 0.0009);
        let tally = accumulator.tally();
        assert_eq!(tally.input_tokens, 50);
        assert_eq!(tally.output_tokens, 25);
        assert!((tally.cost_usd - 0.0009).abs() < 1e-9, "non-zero caller cost used as-is");
    }

    #[test]
    fn counting_sink_empty_tally_when_no_records() {
        // The SubAgentTool suppresses the cost line when the child made no tracked
        // LLM calls — a fresh accumulator must read all-zero.
        let accumulator = Arc::new(kernel_core::CostAccumulator::new());
        let sink = CountingCostSink::new(None, Arc::clone(&accumulator));
        let _ = sink; // constructed but never recorded into
        let tally = accumulator.tally();
        assert_eq!(tally.input_tokens, 0);
        assert_eq!(tally.output_tokens, 0);
        assert_eq!(tally.cost_usd, 0.0);
    }
}
