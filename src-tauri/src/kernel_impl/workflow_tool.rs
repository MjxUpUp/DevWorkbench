//! WorkflowTool — `run_workflow_graph`: the bridge that turns kernel-compose's
//! static graph engine into an agent-driven dynamic workflow (the Anthropic
//! "plan → parallel subagents → verify → report" recipe). The orchestrator
//! agent authors a DAG (graph JSON) for a complex task and calls this tool to
//! execute it; the tool drives the engine with a fresh in-process executor.
//!
//! Worker isolation: every Agent node runs in a FRESH context — opaque CLI
//! agents or transparent ReactAgents built per-node, inheriting NO context
//! from the orchestrator (the strong-orchestrator / weak-worker split). The
//! orchestrator sees ONLY per-node outcomes + the NodeRetried sequence
//! (reliability), never a worker's execution context — Mode C (back-flowing
//! worker context into the strong model) is excluded by design: it would
//! pollute the orchestrator's context, invert the cost model, and tempt it to
//! interfere. A dead worker is retried or tolerated per its on_failure policy;
//! the orchestrator never reaches into the execution.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use kernel_core::{Error, Tool, ToolContext, ToolInfo};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::agents::AgentState;
use crate::db::DbState;
use crate::kernel_impl::executor::KernelExecutor;

/// Default wall-clock cap (seconds) for one graph run. A workflow fans out
/// multiple agents; bounding the total keeps a wedged worker from hanging the
/// orchestrator's turn indefinitely.
const DEFAULT_WORKFLOW_TIMEOUT_SECS: u64 = 1800;

/// The `run_workflow_graph` tool. Holds only an AppHandle — processes / db are
/// resolved from managed state at invoke time (same pattern as
/// `commands::workflows::run_workflow`). Built by `build_react_agent` for
/// orchestrator agents only (app = Some); worker agents get none, bounding
/// self-planning recursion at depth 1.
pub struct WorkflowTool {
    app: AppHandle,
    timeout_secs: u64,
}

impl WorkflowTool {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            timeout_secs: DEFAULT_WORKFLOW_TIMEOUT_SECS,
        }
    }
}

#[async_trait]
impl Tool for WorkflowTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "run_workflow_graph".into(),
            description: "把一个复杂任务作为 DAG（有向无环图）自规划自执行：你提供 graph 定义\
                （nodes/edges/start/end），本工具驱动内核引擎逐节点执行。worker（Agent 节点）\
                在隔离的全新上下文里跑——你看不到 worker 的执行过程，只看到每个节点的产出 +\
                重试历史（用以判断 worker 可靠性）。用于：能拆成多个独立子任务、需要结构化\
                扇出/条件分支/验收闸门/逐节点容错的复杂工作。先在脑子里 plan（拆成 graph）再\
                调用本工具执行——这就是 plan→执行 的桥。".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "graph": {
                        "type": "object",
                        "description": "DAG 定义：{nodes: {id: {type, ...}}, edges: [{from, to, when?}], start: id, end: id}。\
                            节点 type: prompt | agent | gate | parallel | merge | human | transform | branch | loop | selector | interrupt。\
                            agent 节点可配 on_failure 控制失败策略：\"fail\"（默认，失败即终止）| \"continue\"（容错，失败产出 error 值继续）| {\"retry\":{\"max_attempts\":3,\"backoff_secs\":1,\"continue_on_exhausted\":false}}（重试）。\
                            parallel 节点把输入扇出到所有后继；graph 按依赖拓扑执行，同一波独立节点并发运行（波式并行），任一节点失败 fail-fast 中止全图。"
                    },
                    "input": {
                        "description": "喂给 start 节点的输入值（任务描述/种子数据），缺省 null。",
                        "default": null
                    }
                },
                "required": ["graph"]
            }),
        }
    }

    async fn invoke(&self, args: &str, ctx: &ToolContext) -> Result<String, Error> {
        let parsed: Value = serde_json::from_str(args)
            .map_err(|e| Error::Agent(format!("run_workflow_graph 参数非合法 JSON: {e}")))?;
        let graph_value = parsed.get("graph").ok_or_else(|| {
            Error::Agent("run_workflow_graph 需要 {graph: {nodes,edges,start,end}}".into())
        })?;
        let graph: kernel_compose::Graph = serde_json::from_value(graph_value.clone())
            .map_err(|e| Error::Agent(format!("graph 结构无效: {e}")))?;
        let compiled = graph
            .compile()
            .map_err(|e| Error::Agent(format!("graph 编译失败（含环/断节点/loop 体无效）: {e}")))?;
        let input = parsed.get("input").cloned().unwrap_or(Value::Null);
        let working_dir = ctx.working_dir.clone();

        // Resolve executor deps from managed state (mirrors run_workflow). The
        // orchestrator always runs under an app with these registered; a missing
        // state is a wiring error, not a recoverable runtime condition.
        let processes = self
            .app
            .try_state::<AgentState>()
            .map(|s| s.inner().0.clone())
            .ok_or_else(|| Error::Agent("workflow 引擎不可用：AgentState 未初始化".into()))?;
        let db = self
            .app
            .try_state::<DbState>()
            .map(|s| s.inner().clone())
            .ok_or_else(|| Error::Agent("workflow 引擎不可用：DbState 未初始化".into()))?;
        let executor = KernelExecutor::new(self.app.clone(), processes, db);

        let (stream, _approval_tx) = kernel_compose::run_graph_with_approvals(
            compiled,
            input,
            working_dir,
            Arc::new(executor),
        );

        // Drive the stream with a wall-clock cap. Collect per-node final status
        // + the retry sequence so the orchestrator learns worker reliability
        // WITHOUT seeing any worker's execution context.
        use futures::StreamExt;
        let mut stream = stream;
        let mut node_status: HashMap<String, (String, Option<String>)> = HashMap::new();
        let mut retried: HashMap<String, Vec<String>> = HashMap::new();
        let run_id = uuid::Uuid::new_v4().to_string();
        let drive = async {
            let mut last = Value::Null;
            while let Some(ev) = stream.next().await {
                // Emit real-time progress so the frontend chat DAG panel can
                // render the graph executing node-by-node (the same live view
                // the Orchestrate canvas has for YAML workflows). Best-effort:
                // a missing listener just means no live view — the result
                // string still returns to the orchestrator.
                let _ = self.app.emit(
                    "workflow_graph:progress",
                    serde_json::json!({ "run_id": run_id.as_str(), "event": &ev }),
                );
                match ev {
                    kernel_compose::GraphEvent::NodeEnd { node, status, error } => {
                        node_status
                            .insert(node, (format!("{status:?}").to_lowercase(), error));
                    }
                    kernel_compose::GraphEvent::NodeRetried { node, attempt, error } => {
                        retried
                            .entry(node)
                            .or_default()
                            .push(format!("attempt {attempt}: {error}"));
                    }
                    kernel_compose::GraphEvent::GraphDone { output } => {
                        last = output;
                        break;
                    }
                    kernel_compose::GraphEvent::GraphFailed { error } => return Err(error),
                    _ => {}
                }
            }
            Ok(last)
        };
        let timed =
            tokio::time::timeout(std::time::Duration::from_secs(self.timeout_secs), drive).await;
        match timed {
            Ok(Ok(output)) => Ok(format_outcome(true, None, &output, &node_status, &retried)),
            Ok(Err(graph_err)) => {
                Ok(format_outcome(false, Some(&graph_err), &Value::Null, &node_status, &retried))
            }
            Err(_) => Ok(format!("[workflow 超时（{}s），已中止]", self.timeout_secs)),
        }
    }

    /// Driving a graph can mutate the workspace (worker agents write files), so
    /// this is NOT read-only — surfaced alongside Bash/Write, not the search
    /// tools.
    fn is_read_only(&self) -> bool {
        false
    }
}

/// Render the workflow outcome for the orchestrator: success/failure, the end
/// node's output, and a per-node status table INCLUDING retry history (so the
/// orchestrator sees which workers were flaky without seeing their execution).
fn format_outcome(
    ok: bool,
    err: Option<&str>,
    output: &Value,
    node_status: &HashMap<String, (String, Option<String>)>,
    retried: &HashMap<String, Vec<String>>,
) -> String {
    let mut lines = Vec::new();
    lines.push(if ok {
        "[workflow 完成]".to_string()
    } else {
        format!("[workflow 失败: {}]", err.unwrap_or("未知错误"))
    });
    if ok {
        let out_str = match output {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if !out_str.trim().is_empty() {
            lines.push(format!("最终产出:\n{}", out_str));
        }
    }
    if !node_status.is_empty() {
        lines.push("节点状态:".to_string());
        let mut ids: Vec<&String> = node_status.keys().collect();
        ids.sort();
        for id in ids {
            let (status, error) = &node_status[id];
            let mut row = format!("  - {id}: {status}");
            if let Some(retries) = retried.get(id) {
                row.push_str(&format!("（重试 {} 次: {}）", retries.len(), retries.join("; ")));
            }
            if let Some(e) = error {
                row.push_str(&format!(" err={e}"));
            }
            lines.push(row);
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_outcome_lists_retry_history_for_flaky_worker() {
        let mut status = HashMap::new();
        status.insert("w1".to_string(), ("done".to_string(), None));
        status.insert(
            "w2".to_string(),
            ("failed".to_string(), Some("timeout".to_string())),
        );
        let mut retried = HashMap::new();
        retried.insert(
            "w1".to_string(),
            vec!["attempt 1: 503".to_string(), "attempt 2: 503".to_string()],
        );
        let s = format_outcome(
            true,
            None,
            &Value::String("汇总结果".into()),
            &status,
            &retried,
        );
        assert!(s.contains("[workflow 完成]"), "header present: {s}");
        assert!(s.contains("w1: done"), "w1 done row: {s}");
        assert!(s.contains("重试 2 次"), "retry count surfaced: {s}");
        assert!(s.contains("w2: failed"), "w2 failed row: {s}");
        assert!(s.contains("汇总结果"), "end output surfaced: {s}");
    }

    #[test]
    fn format_outcome_reports_graph_failure() {
        let s = format_outcome(false, Some("end unreachable"), &Value::Null, &HashMap::new(), &HashMap::new());
        assert!(s.contains("[workflow 失败"), "failure header: {s}");
        assert!(s.contains("end unreachable"), "error surfaced: {s}");
    }
}
