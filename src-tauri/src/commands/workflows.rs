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
use std::sync::{Arc, Mutex};

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
        Arc::new(executor),
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
                        state.0.lock().unwrap_or_else(|e| e.into_inner()).remove(&run_id_for_events);
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
            state.0.lock().unwrap_or_else(|e| e.into_inner()).remove(&run_id_for_events);
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

/// A built-in workflow template — a starter YAML the user clones into a real
/// workflow (`create_workflow`) instead of authoring the DAG from scratch. Not
/// persisted (returned by `list_workflow_templates`); the user's edited copy
/// lives in the `workflows` table after they pick one.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowTemplate {
    pub name: String,
    pub description: String,
    pub category: String,
    pub yaml_content: String,
}

// --- Built-in template YAML. Each MUST parse_and_compile (asserted at test
// time by `builtin_templates_all_compile`) — a template that doesn't compile is
// a broken UX: the user clones it and `run_workflow` fails immediately. Using
// the simplest proven shapes (prompt → agent [→ agent] → gate) avoids the
// branch/condition syntax that's easy to get wrong by hand.

const CODE_REVIEW_YAML: &str = r#"
start: prompt_review
end: gate_quality
nodes:
  prompt_review:
    type: prompt
    text: "审查最近的代码改动，找出 bug、坏味道、安全风险，给出可执行的修改建议"
  agent_reviewer:
    type: agent
    agent: claude_code
    model: sonnet
  gate_quality:
    type: gate
    gate: forge
edges:
  - { from: prompt_review, to: agent_reviewer }
  - { from: agent_reviewer, to: gate_quality }
"#;

const REQUIREMENT_DECOMPOSE_YAML: &str = r#"
start: prompt_decompose
end: gate_done
nodes:
  prompt_decompose:
    type: prompt
    text: "把给定需求拆解为可执行的子任务清单，标注依赖关系与验收标准"
  agent_analyst:
    type: agent
    agent: claude_code
    model: sonnet
  agent_coder:
    type: agent
    agent: claude_code
    model: sonnet
  gate_done:
    type: gate
    gate: forge
edges:
  - { from: prompt_decompose, to: agent_analyst }
  - { from: agent_analyst, to: agent_coder }
  - { from: agent_coder, to: gate_done }
"#;

const BUG_REPRO_YAML: &str = r#"
start: prompt_repro
end: gate_check
nodes:
  prompt_repro:
    type: prompt
    text: "据 bug 描述写出最小复现步骤，以及预期与实际的差异"
  agent_repro:
    type: agent
    agent: claude_code
    model: sonnet
  gate_check:
    type: gate
    gate: forge
edges:
  - { from: prompt_repro, to: agent_repro }
  - { from: agent_repro, to: gate_check }
"#;

const DOC_GEN_YAML: &str = r#"
start: prompt_doc
end: agent_writer
nodes:
  prompt_doc:
    type: prompt
    text: "阅读指定代码并生成 API 文档与使用示例"
  agent_writer:
    type: agent
    agent: claude_code
    model: sonnet
edges:
  - { from: prompt_doc, to: agent_writer }
"#;

const REFACTOR_YAML: &str = r#"
start: prompt_refactor
end: gate_quality
nodes:
  prompt_refactor:
    type: prompt
    text: "重构指定模块以提升可读性与可测性，保持外部行为不变"
  agent_refactor:
    type: agent
    agent: claude_code
    model: sonnet
  gate_quality:
    type: gate
    gate: forge
edges:
  - { from: prompt_refactor, to: agent_refactor }
  - { from: agent_refactor, to: gate_quality }
"#;

/// List the built-in workflow templates (D5): starters the "new workflow" UI
/// offers so the user doesn't face a blank DAG. Each `yaml_content` is a complete
/// WorkflowDef the user can save verbatim via `create_workflow` and run.
#[tauri::command]
pub async fn list_workflow_templates() -> Result<Vec<WorkflowTemplate>, AppError> {
    Ok(vec![
        WorkflowTemplate {
            name: "code-review".into(),
            description: "对一次改动跑代码审查 agent，再过 Forge 质量门禁".into(),
            category: "质量".into(),
            yaml_content: CODE_REVIEW_YAML.into(),
        },
        WorkflowTemplate {
            name: "requirement-decompose".into(),
            description: "需求分析 agent 拆解任务，再交实现 agent 落地，最后过门禁".into(),
            category: "研发".into(),
            yaml_content: REQUIREMENT_DECOMPOSE_YAML.into(),
        },
        WorkflowTemplate {
            name: "bug-repro".into(),
            description: "复现 bug 的 agent，输出最小复现步骤并过门禁".into(),
            category: "调试".into(),
            yaml_content: BUG_REPRO_YAML.into(),
        },
        WorkflowTemplate {
            name: "doc-gen".into(),
            description: "据代码生成 API 文档与使用示例的 agent".into(),
            category: "文档".into(),
            yaml_content: DOC_GEN_YAML.into(),
        },
        WorkflowTemplate {
            name: "refactor".into(),
            description: "重构指定模块的 agent，过 Forge 门禁验收行为不变".into(),
            category: "研发".into(),
            yaml_content: REFACTOR_YAML.into(),
        },
    ])
}

#[cfg(test)]
mod template_tests {
    use super::*;

    #[test]
    fn builtin_templates_all_compile() {
        // The single invariant for shipped templates: each YAML must
        // parse_and_compile. A template that fails this is a broken UX (user
        // clones it → run_workflow errors immediately), so this guard runs on
        // every build, not just when someone remembers to run the workflow.
        let templates = futures::executor::block_on(list_workflow_templates()).unwrap();
        assert!(
            templates.len() >= 5,
            "expected at least 5 builtin templates, got {}",
            templates.len()
        );
        for t in &templates {
            kernel_compose::WorkflowDef::parse_and_compile(&t.yaml_content)
                .unwrap_or_else(|e| panic!("builtin template '{}' does not compile: {e}", t.name));
        }
    }

    #[test]
    fn builtin_templates_have_unique_names() {
        // The frontend keys the "new from template" picker by name; a duplicate
        // would silently shadow one entry.
        let templates = futures::executor::block_on(list_workflow_templates()).unwrap();
        let mut names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
        let before = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), before, "duplicate builtin template names");
    }
}
