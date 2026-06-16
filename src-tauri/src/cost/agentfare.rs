//! AgentFare data reader — reads AgentFare SQLite DB, aggregates cost data, checks budget.

use rusqlite::Connection;

use crate::error::AppError;
use crate::models::{BudgetSettings, CostRecord, CostSummary, CostTrendPoint};

/// Read cost records from the local AgentFare database.
pub fn read_cost_records(_db_path: &str) -> Result<Vec<CostRecord>, AppError> {
    // Skeleton: will open external AgentFare SQLite and read records.
    Ok(Vec::new())
}

/// Persist one cost record. This is the write half that was missing — the read
/// side (aggregate_costs / cost_trend) and the table both already existed, but
/// nothing was ever inserting rows, so cost tracking stayed at zero. Called from
/// `DbCostSink::record` (fire-and-forget on a blocking thread) per completed
/// model request.
pub fn insert_cost_record(conn: &Connection, record: &CostRecord) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO cost_records (id, session_id, agent_type, model, input_tokens, output_tokens, cost_usd, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            record.id,
            record.session_id,
            record.agent_type,
            record.model,
            record.input_tokens,
            record.output_tokens,
            record.cost_usd,
            record.recorded_at,
        ],
    )?;
    Ok(())
}

/// Aggregate cost data into a summary.
pub fn aggregate_costs(conn: &Connection) -> Result<CostSummary, AppError> {
    let row = conn.query_row(
        "SELECT COALESCE(SUM(cost_usd), 0.0), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COUNT(DISTINCT COALESCE(session_id, id)) FROM cost_records",
        [],
        |row| {
            Ok(CostSummary {
                total_cost: row.get(0)?,
                total_input_tokens: row.get(1)?,
                total_output_tokens: row.get(2)?,
                session_count: row.get(3)?,
            })
        },
    )?;
    Ok(row)
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

/// Save budget settings.
pub fn save_budget_settings(conn: &Connection, settings: &BudgetSettings) -> Result<(), AppError> {
    conn.execute(
        "INSERT OR REPLACE INTO budget_settings (id, monthly_budget_usd, alert_threshold, updated_at)
         VALUES (1, ?1, ?2, ?3)",
        rusqlite::params![
            settings.monthly_budget_usd,
            settings.alert_threshold,
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

    Ok(month_cost >= budget * settings.alert_threshold)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CostRecord;

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
                cost_usd REAL NOT NULL DEFAULT 0,
                recorded_at TEXT NOT NULL
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
            cost_usd: 0.0026,
            // Use "now" so the row always falls inside cost_trend's window,
            // whatever calendar day the test runs on.
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
        assert!(
            (summary.total_cost - 0.0052).abs() < 1e-9,
            "total_cost {}",
            summary.total_cost
        );
        // session_count counts DISTINCT session_id — both rows share sess-1.
        assert_eq!(summary.session_count, 1);
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
}
