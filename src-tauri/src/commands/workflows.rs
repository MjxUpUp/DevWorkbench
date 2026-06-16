//! Workflow management commands.

use tauri::State;

use crate::db::DbState;
use crate::error::AppError;
use crate::models::Workflow;

#[tauri::command]
pub async fn list_workflows(db: State<'_, DbState>) -> Result<Vec<Workflow>, AppError> {
    let conn = db.get().map_err(|e| AppError::NotFound(format!("Lock error: {}", e)))?;
    let mut stmt = conn.prepare(
        "SELECT id, name, yaml_content, created_at, updated_at FROM workflows ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Workflow {
            id: row.get(0)?,
            name: row.get(1)?,
            yaml_content: row.get(2)?,
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })?;
    let mut workflows = Vec::new();
    for w in rows {
        workflows.push(w?);
    }
    Ok(workflows)
}

#[tauri::command]
pub async fn create_workflow(
    db: State<'_, DbState>,
    name: String,
    yaml_content: String,
) -> Result<Workflow, AppError> {
    let conn = db.get().map_err(|e| AppError::NotFound(format!("Lock error: {}", e)))?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO workflows (id, name, yaml_content, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id, name, yaml_content, now, now],
    )?;
    Ok(Workflow {
        id,
        name,
        yaml_content,
        created_at: now.clone(),
        updated_at: now,
    })
}

#[tauri::command]
pub async fn get_workflow(db: State<'_, DbState>, id: String) -> Result<Workflow, AppError> {
    let conn = db.get().map_err(|e| AppError::NotFound(format!("Lock error: {}", e)))?;
    conn.query_row(
        "SELECT id, name, yaml_content, created_at, updated_at FROM workflows WHERE id = ?1",
        [&id],
        |row| {
            Ok(Workflow {
                id: row.get(0)?,
                name: row.get(1)?,
                yaml_content: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )
    .map_err(|e| AppError::NotFound(format!("Workflow not found: {}", e)))
}

#[tauri::command]
pub async fn update_workflow(
    db: State<'_, DbState>,
    id: String,
    name: String,
    yaml_content: String,
) -> Result<Workflow, AppError> {
    let conn = db.get().map_err(|e| AppError::NotFound(format!("Lock error: {}", e)))?;
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE workflows SET name = ?1, yaml_content = ?2, updated_at = ?3 WHERE id = ?4",
        rusqlite::params![name, yaml_content, now, id],
    )?;
    // Return the updated workflow
    conn.query_row(
        "SELECT id, name, yaml_content, created_at, updated_at FROM workflows WHERE id = ?1",
        [&id],
        |row| {
            Ok(Workflow {
                id: row.get(0)?,
                name: row.get(1)?,
                yaml_content: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )
    .map_err(|e| AppError::NotFound(format!("Workflow not found after update: {}", e)))
}

#[tauri::command]
pub async fn delete_workflow(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db.get().map_err(|e| AppError::NotFound(format!("Lock error: {}", e)))?;
    conn.execute("DELETE FROM workflows WHERE id = ?1", [&id])?;
    Ok(())
}

// Note: run_workflow was a stub returning "not yet implemented".
// It has been replaced by the real graph execution engine below.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::commands::agents::AgentState;
use crate::kernel_impl::executor::KernelExecutor;

use kernel_compose::HumanApproval;

/// A single progress event the frontend subscribes to via `workflow:progress`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowProgress {
    pub run_id: String,
    pub event: kernel_compose::GraphEvent,
}

/// Holds the approval sender for each active workflow run, keyed by run_id.
///
/// When a Human node pauses, the graph emits `ApprovalRequired{resume_token}`
/// and blocks on the approval channel. The frontend resolves it via
/// [`approve_workflow_step`], which pulls this run's sender and forwards the
/// decision. The entry is removed when the run reaches a terminal event.
#[derive(Default)]
pub struct ApprovalState(pub Mutex<HashMap<String, tokio::sync::mpsc::Sender<HumanApproval>>>);

/// Run a workflow defined as YAML. Parses → compiles → executes the graph,
/// streaming `workflow:progress` events (one per GraphEvent) to the frontend,
/// and returns the final output when the graph completes.
///
/// The workflow's Agent nodes resolve to opaque CLI agents (claude/codex/…)
/// via the existing PTY engine; Gate nodes resolve to Forge / honesty.
#[tauri::command]
pub async fn run_workflow(
    app: tauri::AppHandle,
    agent_state: State<'_, AgentState>,
    db: State<'_, DbState>,
    approval_state: State<'_, ApprovalState>,
    yaml_content: String,
    input: serde_json::Value,
    working_dir: Option<String>,
) -> Result<serde_json::Value, AppError> {
    let compiled = kernel_compose::yaml::WorkflowDef::parse_and_compile(&yaml_content)
        .map_err(AppError::NotFound)?;

    let executor = KernelExecutor::new(
        app.clone(),
        agent_state.inner().0.clone(),
        db.inner().clone(),
    );

    let run_id = uuid::Uuid::new_v4().to_string();
    let (stream, approval_tx) = kernel_compose::run_graph_with_approvals(
        compiled,
        input,
        working_dir,
        Box::new(executor),
    );
    // Keep the approval sender reachable so the frontend can resume a paused
    // Human node via `approve_workflow_step`. Previously this was discarded
    // (`_approval_tx`), which deadlocked any workflow containing a Human node.
    approval_state
        .0
        .lock()
        .unwrap()
        .insert(run_id.clone(), approval_tx);

    use futures::StreamExt;
    let mut stream = stream;
    let run_id_for_events = run_id.clone();
    let app_for_events = app.clone();
    let join_result = tokio::spawn(async move {
        let mut last_output = serde_json::Value::Null;
        while let Some(ev) = stream.next().await {
            let _ = app_for_events.emit(
                "workflow:progress",
                WorkflowProgress {
                    run_id: run_id_for_events.clone(),
                    event: ev.clone(),
                },
            );
            let terminal = matches!(
                ev,
                kernel_compose::GraphEvent::GraphDone { .. }
                    | kernel_compose::GraphEvent::GraphFailed { .. }
            );
            match ev {
                kernel_compose::GraphEvent::GraphDone { output } => {
                    last_output = output;
                }
                kernel_compose::GraphEvent::GraphFailed { error } => {
                    if let Some(state) = app_for_events.try_state::<ApprovalState>() {
                        state.0.lock().unwrap().remove(&run_id_for_events);
                    }
                    return Err(error);
                }
                _ => {}
            }
            if terminal {
                break;
            }
        }
        // Clean up this run's approval entry on terminal completion so the
        // map doesn't leak finished runs (sender drop also unblocks any
        // lingering waiters on the receiver side).
        if let Some(state) = app_for_events.try_state::<ApprovalState>() {
            state.0.lock().unwrap().remove(&run_id_for_events);
        }
        Ok(serde_json::json!({ "run_id": run_id_for_events, "output": last_output }))
    })
    .await;

    match join_result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(graph_err)) => Err(AppError::NotFound(graph_err)),
        Err(join_err) => Err(AppError::NotFound(format!("run task join: {join_err}"))),
    }
}

/// Resolve a paused Human node in a running workflow.
///
/// `approved = true` resumes the node with an affirmative decision;
/// `approved = false` rejects it (the node fails with "human rejected" and the
/// graph stops). `resume_token` must match the one in the `approval_required`
/// event the frontend received. The sender is looked up by `run_id` in
/// [`ApprovalState`]; a finished/cleaned-up run returns NotFound.
#[tauri::command]
pub async fn approve_workflow_step(
    approval_state: State<'_, ApprovalState>,
    run_id: String,
    resume_token: String,
    approved: bool,
) -> Result<(), AppError> {
    let tx = approval_state
        .0
        .lock()
        .unwrap()
        .get(&run_id)
        .cloned();
    let tx = tx.ok_or_else(|| {
        AppError::NotFound(format!(
            "no active approval channel for run {run_id} (already finished?)"
        ))
    })?;
    // None = rejected (graph fails this node); Some = resume with this value.
    let decision = if approved {
        Some(serde_json::json!({ "approved": true }))
    } else {
        None
    };
    tx.send(HumanApproval { resume_token, decision })
        .await
        .map_err(|_| AppError::NotFound("approval channel closed".into()))?;
    Ok(())
}
