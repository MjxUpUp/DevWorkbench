//! AgentFare data reader — reads AgentFare SQLite DB, aggregates cost data, checks budget.

use rusqlite::Connection;

use crate::cost::pricing::{self, CostBreakdown, TokenUsage};
use crate::error::AppError;
use crate::models::{BudgetSettings, CostRecord, CostSummary, CostTrendPoint};

/// Persist one cost record. This is the write half that was missing — the read
/// side (aggregate_costs / cost_trend) and the table both already existed, but
/// nothing was ever inserting rows, so cost tracking stayed at zero. Called from
/// `DbCostSink::record` (fire-and-forget on a blocking thread) per completed
/// model request. B5: now persists the cache-read/write token tiers too.
pub fn insert_cost_record(conn: &Connection, record: &CostRecord) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO cost_records (id, session_id, agent_type, model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_usd, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            record.id,
            record.session_id,
            record.agent_type,
            record.model,
            record.input_tokens,
            record.output_tokens,
            record.cache_read_tokens,
            record.cache_write_tokens,
            record.cost_usd,
            record.recorded_at,
        ],
    )?;
    Ok(())
}

/// Aggregate cost data into a summary. B5: the summary now carries the
/// transparent per-tier breakdown (input/output/cache token totals + their USD
/// split). The total cost is still `SUM(cost_usd)` (what was actually charged at
/// insert time); the per-tier split is RE-DERIVED here by grouping rows by model
/// and multiplying each model's token sums by `pricing_for(model)`. Recomputing
/// from tokens — rather than storing a split per row — keeps one source of truth
/// (token counts) and means the dashboard split is always internally consistent
/// with the pricing table even if a model id was reclassified later.
pub fn aggregate_costs(conn: &Connection) -> Result<CostSummary, AppError> {
    let row = conn.query_row(
        "SELECT
            COALESCE(SUM(cost_usd), 0.0),
            COALESCE(SUM(input_tokens), 0),
            COALESCE(SUM(output_tokens), 0),
            COALESCE(SUM(cache_read_tokens), 0),
            COALESCE(SUM(cache_write_tokens), 0),
            COUNT(DISTINCT COALESCE(session_id, id))
         FROM cost_records",
        [],
        |row| {
            Ok((
                row.get::<_, f64>(0)?, // total_cost
                row.get::<_, i64>(1)?, // total_input_tokens
                row.get::<_, i64>(2)?, // total_output_tokens
                row.get::<_, i64>(3)?, // total_cache_read_tokens
                row.get::<_, i64>(4)?, // total_cache_write_tokens
                row.get::<_, i64>(5)?, // session_count
            ))
        },
    )?;
    let (total_cost, total_in, total_out, total_cache_read, total_cache_write, session_count) = row;

    // Per-tier USD split: group by model so each family's tokens hit its own
    // pricing tier, then fold the per-model CostBreakdowns into one total.
    let mut stmt = conn.prepare(
        "SELECT model,
            COALESCE(SUM(input_tokens), 0),
            COALESCE(SUM(output_tokens), 0),
            COALESCE(SUM(cache_read_tokens), 0),
            COALESCE(SUM(cache_write_tokens), 0)
         FROM cost_records GROUP BY model",
    )?;
    let groups = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut split = CostBreakdown::default();
    for group in groups {
        let (model, input, output, cache_read, cache_write) = group?;
        let usage = TokenUsage {
            input: input.max(0) as u32,
            output: output.max(0) as u32,
            cache_read: cache_read.max(0) as u32,
            cache_write: cache_write.max(0) as u32,
        };
        let b = pricing::cost_breakdown(usage, pricing::pricing_for(&model));
        split.input_cost += b.input_cost;
        split.output_cost += b.output_cost;
        split.cache_read_cost += b.cache_read_cost;
        split.cache_write_cost += b.cache_write_cost;
    }

    Ok(CostSummary {
        total_cost,
        total_input_tokens: total_in,
        total_output_tokens: total_out,
        session_count,
        total_cache_read_tokens: total_cache_read,
        total_cache_write_tokens: total_cache_write,
        input_cost: split.input_cost,
        output_cost: split.output_cost,
        cache_read_cost: split.cache_read_cost,
        cache_write_cost: split.cache_write_cost,
    })
}

/// Get daily cost trend.
pub fn cost_trend(conn: &Connection, days: i64) -> Result<Vec<CostTrendPoint>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT DATE(recorded_at) as date, SUM(cost_usd) as cost, SUM(input_tokens + output_tokens) as tokens
         FROM cost_records
         WHERE recorded_at >= DATE('now', ?1)
         GROUP BY DATE(recorded_at)
         ORDER BY date",
    )?;
    let param = format!("-{} days", days);
    let rows = stmt.query_map([&param], |row| {
        Ok(CostTrendPoint {
            date: row.get(0)?,
            cost: row.get(1)?,
            tokens: row.get(2)?,
        })
    })?;
    let mut points = Vec::new();
    for point in rows {
        points.push(point?);
    }
    Ok(points)
}

/// Load budget settings from the database.
pub fn load_budget_settings(conn: &Connection) -> Result<BudgetSettings, AppError> {
    let result = conn.query_row(
        "SELECT monthly_budget_usd, alert_threshold FROM budget_settings WHERE id = 1",
        [],
        |row| {
            Ok(BudgetSettings {
                monthly_budget_usd: row.get(0)?,
                alert_threshold: row.get::<_, Option<f64>>(1)?.unwrap_or(0.8),
            })
        },
    );
    match result {
        Ok(settings) => Ok(settings),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(BudgetSettings {
            monthly_budget_usd: None,
            alert_threshold: 0.8,
        }),
        Err(e) => Err(AppError::Cost(e.to_string())),
    }
}

/// Clamp `alert_threshold` into a sane `[0.1, 1.0]` band. NaN/inf/negative
/// would otherwise either trip the alert forever (0/negative → `cost >= budget*0`
/// is always true) or never fire (`>=1` always false; NaN makes every `>=`
/// comparison false) — silently disabling the alert (F9). Falls back to 0.8
/// (the schema default) when the stored value isn't finite.
fn normalize_alert_threshold(raw: f64) -> f64 {
    if raw.is_finite() {
        raw.clamp(0.1, 1.0)
    } else {
        0.8
    }
}

/// Save budget settings.
pub fn save_budget_settings(conn: &Connection, settings: &BudgetSettings) -> Result<(), AppError> {
    conn.execute(
        "INSERT OR REPLACE INTO budget_settings (id, monthly_budget_usd, alert_threshold, updated_at)
         VALUES (1, ?1, ?2, ?3)",
        rusqlite::params![
            settings.monthly_budget_usd,
            normalize_alert_threshold(settings.alert_threshold),
            chrono::Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Check if the current month's spending exceeds the budget alert threshold.
pub fn check_budget_alert(conn: &Connection) -> Result<bool, AppError> {
    let settings = load_budget_settings(conn)?;
    let budget = match settings.monthly_budget_usd {
        Some(b) => b,
        None => return Ok(false),
    };

    let month_cost: f64 = conn.query_row(
        "SELECT COALESCE(SUM(cost_usd), 0.0) FROM cost_records WHERE recorded_at >= DATE('now', 'start of month')",
        [],
        |row| row.get(0),
    )?;

    Ok(month_cost >= budget * normalize_alert_threshold(settings.alert_threshold))
}

/// Month-to-date spend (USD). The window is `start of month` to now, matching
/// `check_budget_alert`. Extracted so the hard-limit check below and the alert
/// share one source of truth for "how much has been spent this month".
pub fn monthly_cost(conn: &Connection) -> Result<f64, AppError> {
    let month_cost: f64 = conn.query_row(
        "SELECT COALESCE(SUM(cost_usd), 0.0) FROM cost_records WHERE recorded_at >= DATE('now', 'start of month')",
        [],
        |row| row.get(0),
    )?;
    Ok(month_cost)
}

/// Hard budget limit (v1.2 T10): true when month-to-date spend has reached the
/// configured monthly budget. Distinct from `check_budget_alert` (which trips at
/// `alert_threshold`, e.g. 80%); this trips at 100% and is what the ReactAgent
/// turn loop uses to halt before burning past the cap. No budget configured →
/// never exhausted (unlimited).
pub fn is_budget_exhausted(conn: &Connection) -> Result<bool, AppError> {
    let budget = match load_budget_settings(conn)?.monthly_budget_usd {
        Some(b) if b > 0.0 => b,
        _ => return Ok(false),
    };
    Ok(monthly_cost(conn)? >= budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CostRecord;

    #[test]
    fn normalize_alert_threshold_clamps_and_handles_nan() {
        // F9: NaN/inf/out-of-band thresholds silently disabled the alert
        // (0/negative → trip forever; NaN/inf → `>=` always false → never fire).
        assert_eq!(normalize_alert_threshold(0.8), 0.8);
        assert_eq!(normalize_alert_threshold(0.05), 0.1, "clamps up to 0.1");
        assert_eq!(normalize_alert_threshold(1.5), 1.0, "clamps down to 1.0");
        assert_eq!(normalize_alert_threshold(0.0), 0.1, "0 → 0.1 (was: trip forever)");
        assert_eq!(normalize_alert_threshold(-0.5), 0.1, "negative → 0.1");
        assert_eq!(normalize_alert_threshold(f64::NAN), 0.8, "NaN → default 0.8");
        assert_eq!(normalize_alert_threshold(f64::INFINITY), 0.8, "inf → default 0.8");
    }

    /// In-memory DB with the cost_records table applied — so insert_cost_record
    /// runs against the real table definition. Mirrors the DDL in db.rs SCHEMA;
    /// duplicated here to avoid reaching into the private SCHEMA constant.
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE cost_records (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                agent_type TEXT NOT NULL,
                model TEXT NOT NULL,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_write_tokens INTEGER NOT NULL DEFAULT 0,
                cost_usd REAL NOT NULL DEFAULT 0,
                recorded_at TEXT NOT NULL
            );
            CREATE TABLE budget_settings (
                id INTEGER PRIMARY KEY,
                monthly_budget_usd REAL,
                alert_threshold REAL DEFAULT 0.8,
                updated_at TEXT NOT NULL
            );",
        )
        .unwrap();
        conn
    }

    fn sample_record(id: &str) -> CostRecord {
        CostRecord {
            id: id.into(),
            session_id: Some("sess-1".into()),
            agent_type: "react_kernel".into(),
            model: "glm-4.6".into(),
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: 0.0026,
            // Use "now" so the row always falls inside cost_trend's window,
            // whatever calendar day the test runs on.
            recorded_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// A Sonnet row with prompt-cache tokens, so the B5 breakdown path has
    /// something to split. Sonnet pricing: input $3, output $15, cache-read $0.30,
    /// cache-write $3.75 per 1M. For 1M/1M/500k/500k:
    ///   input 3.0 + output 15.0 + cache_read 0.15 + cache_write 1.875 = 20.025.
    fn sonnet_cache_record(id: &str) -> CostRecord {
        CostRecord {
            id: id.into(),
            session_id: Some("sess-sonnet".into()),
            agent_type: "react_kernel".into(),
            model: "claude-sonnet-4-5".into(),
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_tokens: 500_000,
            cache_write_tokens: 500_000,
            cost_usd: 20.025,
            recorded_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    #[test]
    fn insert_then_aggregate_round_trip() {
        let conn = test_conn();
        insert_cost_record(&conn, &sample_record("r1")).unwrap();
        insert_cost_record(&conn, &sample_record("r2")).unwrap();
        let summary = aggregate_costs(&conn).unwrap();
        assert_eq!(summary.total_input_tokens, 2000);
        assert_eq!(summary.total_output_tokens, 1000);
        assert_eq!(summary.total_cache_read_tokens, 0);
        assert_eq!(summary.total_cache_write_tokens, 0);
        assert!(
            (summary.total_cost - 0.0052).abs() < 1e-9,
            "total_cost {}",
            summary.total_cost
        );
        // session_count counts DISTINCT session_id — both rows share sess-1.
        assert_eq!(summary.session_count, 1);
    }

    #[test]
    fn aggregate_breakdown_splits_input_output_and_cache_by_model() {
        let conn = test_conn();
        insert_cost_record(&conn, &sonnet_cache_record("c1")).unwrap();
        let summary = aggregate_costs(&conn).unwrap();
        // Token tiers flow through.
        assert_eq!(summary.total_input_tokens, 1_000_000);
        assert_eq!(summary.total_output_tokens, 1_000_000);
        assert_eq!(summary.total_cache_read_tokens, 500_000);
        assert_eq!(summary.total_cache_write_tokens, 500_000);
        // USD split is derived from Sonnet pricing.
        assert!((summary.input_cost - 3.0).abs() < 1e-9, "input: {}", summary.input_cost);
        assert!((summary.output_cost - 15.0).abs() < 1e-9, "output: {}", summary.output_cost);
        assert!((summary.cache_read_cost - 0.15).abs() < 1e-9, "cache_read: {}", summary.cache_read_cost);
        assert!((summary.cache_write_cost - 1.875).abs() < 1e-9, "cache_write: {}", summary.cache_write_cost);
    }

    #[test]
    fn aggregate_breakdown_folds_multiple_models_into_one_split() {
        let conn = test_conn();
        // Two GLM rows (no cache pricing → cache contributes $0) + one Sonnet row.
        insert_cost_record(&conn, &sample_record("g1")).unwrap();
        insert_cost_record(&conn, &sonnet_cache_record("s1")).unwrap();
        let summary = aggregate_costs(&conn).unwrap();
        // GLM 1000 input @ $1/M = $0.001; 500 output @ $3.2/M = $0.0016 → $0.0026.
        // Sonnet split total = 3.0 + 15.0 + 0.15 + 1.875 = 20.025.
        let glm_split = 0.0026;
        let expected_input = 0.001 + 3.0;
        let expected_output = 0.0016 + 15.0;
        assert!((summary.input_cost - expected_input).abs() < 1e-9, "input: {}", summary.input_cost);
        assert!((summary.output_cost - expected_output).abs() < 1e-9, "output: {}", summary.output_cost);
        // GLM cache pricing is $0, so only Sonnet contributes.
        assert!((summary.cache_read_cost - 0.15).abs() < 1e-9);
        assert!((summary.cache_write_cost - 1.875).abs() < 1e-9);
        // The split components summed ≈ total of both per-model totals.
        let split_total =
            summary.input_cost + summary.output_cost + summary.cache_read_cost + summary.cache_write_cost;
        assert!((split_total - (glm_split + 20.025)).abs() < 1e-9, "split_total: {split_total}");
    }

    #[test]
    fn insert_with_null_session_id() {
        let conn = test_conn();
        let mut rec = sample_record("r3");
        rec.session_id = None;
        insert_cost_record(&conn, &rec).unwrap();
        let summary = aggregate_costs(&conn).unwrap();
        assert_eq!(summary.total_input_tokens, 1000);
    }

    #[test]
    fn cost_trend_picks_up_today_row() {
        let conn = test_conn();
        insert_cost_record(&conn, &sample_record("r4")).unwrap();
        let trend = cost_trend(&conn, 7).unwrap();
        assert!(
            trend.iter().any(|p| (p.cost - 0.0026).abs() < 1e-9),
            "today's row missing from trend: {trend:?}"
        );
    }

    #[test]
    fn is_budget_exhausted_trips_at_full_budget_not_threshold() {
        let conn = test_conn();
        // No budget set → never exhausted.
        assert!(!is_budget_exhausted(&conn).unwrap());

        // Spend $0.0026. Budget $1.00 → not exhausted (alert at 80% would also
        // be false, but the point is the hard limit is at 100%).
        insert_cost_record(&conn, &sample_record("b1")).unwrap();
        save_budget_settings(
            &conn,
            &BudgetSettings { monthly_budget_usd: Some(1.0), alert_threshold: 0.8 },
        )
        .unwrap();
        assert!(!is_budget_exhausted(&conn).unwrap());

        // Lower the budget to just below the spend → now exhausted.
        save_budget_settings(
            &conn,
            &BudgetSettings { monthly_budget_usd: Some(0.002), alert_threshold: 0.8 },
        )
        .unwrap();
        assert!(is_budget_exhausted(&conn).unwrap());

        // Budget cleared → unlimited again.
        save_budget_settings(
            &conn,
            &BudgetSettings { monthly_budget_usd: None, alert_threshold: 0.8 },
        )
        .unwrap();
        assert!(!is_budget_exhausted(&conn).unwrap());
    }
}
