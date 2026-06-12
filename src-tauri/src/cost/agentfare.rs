//! AgentFare data reader — reads AgentFare SQLite DB, aggregates cost data, checks budget.

use rusqlite::Connection;

use crate::error::AppError;
use crate::models::{BudgetSettings, CostRecord, CostSummary, CostTrendPoint};

/// Read cost records from the local AgentFare database.
pub fn read_cost_records(_db_path: &str) -> Result<Vec<CostRecord>, AppError> {
    // Skeleton: will open external AgentFare SQLite and read records.
    Ok(Vec::new())
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
