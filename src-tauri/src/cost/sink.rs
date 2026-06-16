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
pub trait CostSink: Send + Sync {
    fn record(&self, model: &str, input_tokens: u32, output_tokens: u32, cost_usd: f64);
}

/// A CostSink that drops everything — the default when no DB/session context is
/// available (an ad-hoc agent without a session id, or tests).
#[derive(Debug, Default, Clone, Copy)]
pub struct NullCostSink;

impl CostSink for NullCostSink {
    fn record(&self, _: &str, _: u32, _: u32, _: f64) {}
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
    fn record(&self, model: &str, input_tokens: u32, output_tokens: u32, cost_usd: f64) {
        // If the caller didn't precompute cost, derive it from the pricing
        // table so a missed cost upstream still leaves an honest value rather
        // than a silent zero.
        let cost = if cost_usd > 0.0 {
            cost_usd
        } else {
            pricing::cost(input_tokens, output_tokens, pricing::pricing_for(model))
        };
        let rec = CostRecord {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: self.session_id.clone(),
            agent_type: self.agent_type.clone(),
            model: model.to_string(),
            input_tokens: input_tokens as i64,
            output_tokens: output_tokens as i64,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_sink_silently_drops() {
        // The contract: no panic, no output, no side effect.
        NullCostSink.record("glm-4.6", 100, 200, 0.001);
    }

    #[test]
    fn optional_shared_returns_null_when_no_db() {
        // Without a DB the sink must still be callable (NullCostSink) — no
        // runtime required, no panic.
        let s = optional_shared(None, "x", None);
        s.record("glm-4.6", 1, 1, 0.0);
    }
}
