//! Cost tracking and budget commands.

use tauri::State;

use crate::cost::agentfare;
use crate::db::DbState;
use crate::error::AppError;
use crate::models::{BudgetSettings, CostSummary, CostTrendPoint};

#[tauri::command]
pub async fn get_cost_summary(db: State<'_, DbState>) -> Result<CostSummary, AppError> {
    let conn = db.get().map_err(|e| AppError::Cost(format!("Lock error: {}", e)))?;
    agentfare::aggregate_costs(&conn)
}

#[tauri::command]
pub async fn get_cost_trend(db: State<'_, DbState>, days: i64) -> Result<Vec<CostTrendPoint>, AppError> {
    let conn = db.get().map_err(|e| AppError::Cost(format!("Lock error: {}", e)))?;
    agentfare::cost_trend(&conn, days)
}

#[tauri::command]
pub async fn load_budget(db: State<'_, DbState>) -> Result<BudgetSettings, AppError> {
    let conn = db.get().map_err(|e| AppError::Cost(format!("Lock error: {}", e)))?;
    agentfare::load_budget_settings(&conn)
}

#[tauri::command]
pub async fn save_budget(db: State<'_, DbState>, settings: BudgetSettings) -> Result<(), AppError> {
    let conn = db.get().map_err(|e| AppError::Cost(format!("Lock error: {}", e)))?;
    agentfare::save_budget_settings(&conn, &settings)
}
