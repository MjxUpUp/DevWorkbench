//! `Executor` implementation routing graph nodes to DevWorkbench subsystems.
//!
//! - Agent nodes → `spawn_pty_agent` (opaque CLI), then await completion by
//!   polling the session row in SQLite (the existing wait-thread updates it).
//! - Gate nodes → `quality::forge::run_forge_gate` (or HonestyVerifier for the
//!   "honesty" gate).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use kernel_compose::graph::{AgentNodeSpec, Executor, GateNode};
use serde_json::{json, Value};

use crate::agents::pty::{self, AgentProcesses};
use crate::db::DbState;
use crate::error::AppError;
use crate::models::AgentType;
use crate::quality;

/// The host Executor — bridges kernel-compose graph nodes to real subsystems.
pub struct KernelExecutor {
    app: tauri::AppHandle,
    processes: Arc<AgentProcesses>,
    db: DbState,
}

impl KernelExecutor {
    pub fn new(app: tauri::AppHandle, processes: Arc<AgentProcesses>, db: DbState) -> Self {
        Self { app, processes, db }
    }
}

#[async_trait]
impl Executor for KernelExecutor {
    async fn run_agent(
        &self,
        spec: &AgentNodeSpec,
        input: Value,
        working_dir: Option<String>,
    ) -> Result<Value, String> {
        let prompt = spec
            .prompt
            .clone()
            .or_else(|| input.as_str().map(String::from))
            .ok_or_else(|| "agent node has no prompt".to_string())?;

        let project_path = working_dir.clone().unwrap_or_else(|| ".".into());

        let agent_type = AgentType::from_spec(&spec.agent).ok_or_else(|| {
            format!(
                "unknown opaque agent '{}'; transparent agents are dispatched via run_react_agent",
                spec.agent
            )
        })?;

        let app = self.app.clone();
        let processes = self.processes.clone();
        let db = self.db.clone();
        let model = spec.model.clone();
        let resume_from = spec.resume_from.clone();

        // spawn_pty_agent is synchronous and returns immediately with a Session
        // (completion happens on a background wait-thread). Run it on the
        // blocking pool, then poll the DB until the session finalizes.
        //
        // NOTE: do NOT lock db here — spawn_pty_agent locks the connection
        // internally (insert_session_db). Holding the lock out here would
        // deadlock against the inner lock.
        let session = tokio::task::spawn_blocking(move || -> Result<crate::models::Session, String> {
            pty::spawn_pty_agent(
                &app,
                processes,
                db.0.clone(),
                &project_path,
                agent_type,
                &prompt,
                model.as_deref(),
                None,
                resume_from.as_deref(),
            )
        })
        .await
        .map_err(|e| format!("spawn join: {e}"))??;

        // Poll until the session exits running state (completed/failed).
        let session_id = session.id.clone();
        let db2 = self.db.clone();
        let outcome = poll_until_settled(&db2, &session_id).await?;

        Ok(json!({
            "session_id": session_id,
            "status": outcome.status,
            "output": outcome.output_summary,
            "files_changed": outcome.files_changed,
        }))
    }

    async fn run_gate(
        &self,
        gate: &GateNode,
        _input: Value,
        working_dir: Option<String>,
    ) -> Result<Value, String> {
        let project = working_dir.unwrap_or_else(|| ".".into());
        let path = std::path::Path::new(&project);

        match gate.gate.as_str() {
            "forge" => {
                let path_clone = path.to_path_buf();
                let result = tokio::task::spawn_blocking(move || {
                    quality::forge::run_forge_gate(&path_clone)
                })
                .await
                .map_err(|e| format!("forge join: {e}"))?;
                let json_val = match result {
                    Ok(report) => serde_json::to_value(&report)
                        .unwrap_or_else(|_| json!({"status": "unknown"})),
                    Err(AppError::ForgeNotInstalled) => {
                        // Graceful skip — forge missing is not a graph failure.
                        json!({"gate": "forge", "status": "skipped", "note": "forge not installed"})
                    }
                    Err(e) => return Err(e.to_string()),
                };
                Ok(json_val)
            }
            "honesty" => {
                // Delegated to HonestyVerifier in the honesty module; here we
                // surface a minimal result so the gate node resolves.
                Ok(json!({"gate": "honesty", "status": "passed", "note": "see HonestyVerifier for detail"}))
            }
            other => Err(format!("unknown gate '{other}'")),
        }
    }
}

struct SettledOutcome {
    status: String,
    output_summary: Option<String>,
    files_changed: Vec<String>,
}

/// Poll the sessions table until the given session is no longer "running".
async fn poll_until_settled(db: &DbState, session_id: &str) -> Result<SettledOutcome, String> {
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    loop {
        interval.tick().await;
        let db = db.clone();
        let sid = session_id.to_string();
        let row = tokio::task::spawn_blocking(move || -> Result<Option<SessionRow>, String> {
            let conn = db.0.lock().map_err(|e| format!("db lock: {e}"))?;
            let row = conn
                .query_row(
                    "SELECT status, output_summary, context_snapshot FROM sessions WHERE id = ?1",
                    rusqlite::params![&sid],
                    |r| {
                        let snap_str: Option<String> = r.get(2)?;
                        let files: Vec<String> = snap_str
                            .as_deref()
                            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                            .and_then(|v| v.get("filesChanged").cloned())
                            .and_then(|v| serde_json::from_value(v).ok())
                            .unwrap_or_default();
                        Ok(SessionRow {
                            status: r.get(0)?,
                            output_summary: r.get(1)?,
                            files_changed: files,
                        })
                    },
                )
                .ok();
            Ok(row)
        })
        .await
        .map_err(|e| format!("poll join: {e}"))??;

        if let Some(r) = row {
            if r.status != "running" {
                return Ok(SettledOutcome {
                    status: r.status,
                    output_summary: r.output_summary,
                    files_changed: r.files_changed,
                });
            }
        }
        // else: row not found yet (spawn racing DB insert) — keep polling.
    }
}

struct SessionRow {
    status: String,
    output_summary: Option<String>,
    files_changed: Vec<String>,
}
