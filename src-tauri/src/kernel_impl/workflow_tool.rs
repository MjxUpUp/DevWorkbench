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

use crate::agents::pty::ChatStreamEvent;
use crate::commands::agents::{AgentApprovalState, AgentState};
use crate::db::DbState;
use crate::kernel_impl::executor::KernelExecutor;
use crate::kernel_impl::human_gate::{ApprovalMap, HumanGateDecision, HUMAN_GATE_TIMEOUT};

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
                        "description": "DAG 定义。顶层 {nodes:{id:{type,字段...}}, edges:[{from,to,when?}], start:id, end:id}。\n每种 type 的字段（【必需】不可省，缺一个 graph 反序列化即失败）：\n- prompt: {type:\"prompt\", text:\"提示文本\"【必需】, vars?:{名:值}}。常作 start 种子或 end 报告。\n- agent: {type:\"agent\", agent:\"agent标识\"【必需】, prompt?:\"给worker的指令\", model?:\"...\", on_failure?:...}。agent 标识取值：claude_code/claude、codex、gemini_cli/gemini、qwen_code/qwen、copilot、pi 走对应 CLI worker；填其他（如 react_kernel、react）走自研内核 worker。prompt 缺省时取入边传入值。\n- parallel: {type:\"parallel\", branches?:N}。扇出到所有后继。\n- merge: {type:\"merge\", strategy?:\"concat\"|\"last_wins\"|\"collect\"}。等所有前驱到齐再汇总。\n- gate: {type:\"gate\", gate:\"验收名\"【必需】(如 forge/compile/test), config?:{...}}。\n- transform: {type:\"transform\", op:...}【必需 op】。\n- branch: {type:\"branch\", condition:\"key==value\"|\"contains:子串\"}【必需】，后继 edge 用 when 路由。\n- selector: {type:\"selector\", cases:[{when,label}], default?:\"标签\"}。\n- loop: {type:\"loop\", over?:\"数组路径\", count?:N, body:{子graph}}【必需 body】。\n- interrupt: {type:\"interrupt\", message?:\"...\", condition?:\"...\"}。\n- human: {type:\"human\", prompt?:\"...\"}。\non_failure（仅 agent/gate）：\"fail\"(默认) | \"continue\"(容错) | {\"retry\":{\"max_attempts\":3,\"backoff_secs\":1,\"continue_on_exhausted\":false}}。\n执行：同一波独立节点并发，任一失败 fail-fast 中止。\n最小完整示例（fan-out 两 worker 汇总，字段照抄即可）：\n{nodes:{start:{type:\"prompt\",text:\"审查安全问题\"},fan:{type:\"parallel\"},w1:{type:\"agent\",agent:\"react_kernel\",prompt:\"查shell注入\",on_failure:\"continue\"},w2:{type:\"agent\",agent:\"react_kernel\",prompt:\"查路径穿越\",on_failure:\"continue\"},gather:{type:\"merge\",strategy:\"concat\"},report:{type:\"prompt\",text:\"汇总为报告\"}},edges:[{from:\"start\",to:\"fan\"},{from:\"fan\",to:\"w1\"},{from:\"fan\",to:\"w2\"},{from:\"w1\",to:\"gather\"},{from:\"w2\",to:\"gather\"},{from:\"gather\",to:\"report\"}],start:\"start\",end:\"report\"}"
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

        // B4 DAG human-in-the-loop: the shared Human-Gate approval registry
        // (AgentApprovalState, the same map resolve_human_gate_cmd delivers to).
        // Reusing it — instead of a workflow-specific channel — means a `human`
        // DAG node surfaces in the SAME approval modal as a destructive tool
        // call, and the same resolve command + verdict ledger persist it. None
        // only for non-orchestrator contexts (tests / ACP), where a human node
        // auto-rejects fast rather than wedging 300s with no UI to resolve it.
        let approvals = self
            .app
            .try_state::<AgentApprovalState>()
            .map(|s| s.inner().0.clone());
        // The orchestrator's session id — embedded in the approval token so
        // (a) clear_session_approvals reclaims it on abort, (b) the verdict
        // ledger attributes the intervention to this session, (c) the frontend
        // agent:event listener routes the modal to the right session.
        let session_id = ctx.conversation_id.clone();

        let (stream, approval_tx) = kernel_compose::run_graph_with_approvals(
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
                    kernel_compose::GraphEvent::ApprovalRequired {
                        node,
                        prompt,
                        resume_token: runner_token,
                    } => {
                        // B4: a `human` DAG node paused. Surface it via the same
                        // approval modal the Human Gate uses, and spawn a bridge
                        // that forwards the user's decision to the runner's
                        // approval channel. Without this the discarded approval
                        // sender left every human node hanging 300s then failing.
                        // The drive loop does NOT block on the human — it keeps
                        // consuming the stream while the bridge awaits the modal.
                        match (session_id.as_ref(), approvals.as_ref()) {
                            (Some(sid), Some(ap)) => {
                                // Session-scoped token: `approve__{sid}__wf-{run}-{node}`.
                                // session_of_resume_token (agents.rs) does
                                // rsplit_once("__") on the part after `approve__`,
                                // so the SUFFIX must contain no `__` or the sid
                                // parses wrong (→ verdict ledger attributes to a
                                // non-existent session). sid (UUID) and run_id
                                // (UUID) are `__`-free, but a graph author may
                                // write a node id like `review__security` —
                                // sanitize it. The token is opaque (only a map
                                // key + ledger attribution key; no consumer parses
                                // the node out of it), so mangling `__`→`-` here
                                // is safe. run_id keeps it unique across concurrent
                                // workflow runs; the `wf-` prefix can't collide
                                // with a Human-Gate seq token (`{n}`, numeric).
                                let safe_node = node.replace("__", "-");
                                let wf_token = format!(
                                    "approve__{sid}__wf-{run_id}-{safe_node}"
                                );
                                let (tx, rx) = tokio::sync::oneshot::channel();
                                if let Ok(mut g) = ap.lock() {
                                    g.insert(wf_token.clone(), tx);
                                }
                                // Control meta-event — NOT a chat block; the
                                // frontend short-circuits on `approval_required`
                                // to open the modal (agentStore → ApprovalModal).
                                let wire = ChatStreamEvent::ApprovalRequired {
                                    tool: "run_workflow_graph".to_string(),
                                    arguments: prompt.clone(),
                                    resume_token: wf_token.clone(),
                                    summary: format!("工作流人工审批节点「{node}」：{prompt}"),
                                };
                                let _ = self.app.emit(
                                    "agent:event",
                                    serde_json::json!({ "sessionId": sid, "event": &wire }),
                                );
                                // Bridge: oneshot decision → runner approval.
                                // Detached so the drive loop keeps pulling events
                                // while the human decides; the runner is paused
                                // on its approval receiver until this sends (or
                                // its own 300s timeout fires, whichever first).
                                let ap_bridge = ap.clone();
                                let tx_bridge = approval_tx.clone();
                                let wf_bridge = wf_token.clone();
                                tokio::spawn(async move {
                                    forward_approval(
                                        rx,
                                        runner_token,
                                        wf_bridge,
                                        ap_bridge,
                                        tx_bridge,
                                        HUMAN_GATE_TIMEOUT,
                                    )
                                    .await;
                                });
                            }
                            _ => {
                                // No session/approval context (test/ACP) → no UI
                                // can resolve this. Reject the node fast instead
                                // of wedging the runner's full 300s.
                                log::warn!(
                                    "[workflow] human node '{node}' paused with no \
                                     session/approval context — auto-rejecting"
                                );
                                let _ = approval_tx.send(
                                    kernel_compose::HumanApproval {
                                        resume_token: runner_token,
                                        decision: None,
                                    },
                                ).await;
                            }
                        }
                    }
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

/// Bridge a paused `human` DAG node to its resolution: await the user's
/// one-shot decision (delivered by `resolve_human_gate_cmd` into the shared
/// approval map) and forward it to the runner's approval channel as a
/// [`HumanApproval`].
///
/// The mapping is 1:1 with the approval modal's three buttons:
/// - **Approve** → green-light; the (optional) feedback flows onward as the
///   node's output value (so a human can steer the downstream path by typing).
///   No feedback → `"approved"` sentinel — a non-empty value that still reads
///   as "the human said go" to successor nodes.
/// - **Reject** → `decision: None`, which the runner treats as "human
///   rejected" → the node fails (and fails the graph unless a successor routes
///   around it).
/// - **Retry** → the feedback becomes the node value. For a pure approval gate
///   there is no action to "redo", so Retry collapses to "approved, but use my
///   edited value" — the useful payload (the amended text) is preserved rather
///   than discarded.
///
/// Three failure modes, all honest (never silently succeed against a dead run):
/// - The bridge's own [`HUMAN_GATE_TIMEOUT`] (300s) fires → the human never
///   decided; reclaim the stale Sender and return WITHOUT sending. The runner's
///   own 300s on its approval receiver fires independently → node fails. Not
///   sending avoids a race with that timeout (both paths fail the node anyway).
/// - The Sender was dropped (`clear_session_approvals` on abort) → forward a
///   Reject so the runner fails the node immediately instead of waiting out its
///   own 300s.
/// - The send itself fails → the runner already moved on (its 300s fired or the
///   graph ended). Reclaim the stale Sender so a LATE second resolve returns
///   NotFound instead of silently "succeeding" against a run that's already over.
async fn forward_approval(
    rx: tokio::sync::oneshot::Receiver<HumanGateDecision>,
    runner_token: String,
    wf_token: String,
    approvals: ApprovalMap,
    approval_tx: tokio::sync::mpsc::Sender<kernel_compose::HumanApproval>,
    // How long to wait for the human before reclaiming. Production passes
    // HUMAN_GATE_TIMEOUT (300s); tests pass a short duration to exercise the
    // timeout-reclaim path without waiting the full 300s.
    timeout: std::time::Duration,
) {
    let decision: Option<Value> = match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(HumanGateDecision::Approve)) => Some(Value::String("approved".into())),
        Ok(Ok(HumanGateDecision::Retry { feedback })) => Some(Value::String(feedback)),
        Ok(Ok(HumanGateDecision::Reject)) => None,
        // Sender dropped (session aborted / cleared) → reject so the runner
        // fails the node now rather than waiting out its own 300s.
        Ok(Err(_)) => None,
        // Our 300s — human never decided. Reclaim the stale Sender; let the
        // runner's own 300s fail the node (don't send → no race).
        Err(_) => {
            if let Ok(mut g) = approvals.lock() {
                g.remove(&wf_token);
            }
            return;
        }
    };
    // Forward to the runner.
    if approval_tx
        .send(kernel_compose::HumanApproval {
            resume_token: runner_token,
            decision,
        })
        .await
        .is_err()
    {
        // Runner already gone (its 300s fired or the graph ended). Reclaim so a
        // late resolve returns NotFound instead of silently succeeding.
        if let Ok(mut g) = approvals.lock() {
            g.remove(&wf_token);
        }
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

    // ---- B4: forward_approval bridges a paused human DAG node to its resolution ----

    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::mpsc;

    /// Drive `forward_approval` under one of the resolution shapes and report
    /// (what the runner received, whether the token is still in the map). The
    /// caller picks: the decision to resolve with (None = don't resolve), whether
    /// to simulate an abort that drops the Sender, whether the runner's approval
    /// receiver stays alive, and the bridge timeout.
    async fn run_forward(
        decision: Option<HumanGateDecision>,
        drop_sender: bool,
        keep_approval_rx: bool,
        timeout: Duration,
    ) -> (
        Option<kernel_compose::HumanApproval>, // what the runner received
        bool,                                  // wf_token still in map?
    ) {
        let wf_token = "approve__sess__wf-r1-h".to_string();
        let map: ApprovalMap = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = tokio::sync::oneshot::channel();
        // resolve_approval owns "remove-then-send", so resolving through it
        // mirrors resolve_human_gate_cmd exactly. Register tx under the token.
        map.lock().unwrap().insert(wf_token.clone(), tx);

        if drop_sender {
            // Abort: clear_session_approvals strips the Sender → rx Err.
            crate::kernel_impl::human_gate::clear_session_approvals(&map, "sess");
        } else if let Some(d) = decision {
            crate::kernel_impl::human_gate::resolve_approval(&map, &wf_token, d).unwrap();
        }

        let (approval_tx, approval_rx) = mpsc::channel::<kernel_compose::HumanApproval>(8);
        let rx_for_runner = approval_tx.clone();
        // Wrap in Option so the receiver is dropped (runner-gone case) without
        // the borrow-checker seeing a move-then-use across the branches.
        let mut approval_rx_opt = if keep_approval_rx { Some(approval_rx) } else { None };

        forward_approval(
            rx,
            "approve__h".into(),
            wf_token.clone(),
            map.clone(),
            rx_for_runner,
            timeout,
        )
        .await;

        let received = approval_rx_opt
            .as_mut()
            .and_then(|rx| rx.try_recv().ok());
        let still_present = map.lock().unwrap().contains_key(&wf_token);
        (received, still_present)
    }

    #[tokio::test]
    async fn forward_approval_approve_sends_default_value() {
        let (received, still_present) =
            run_forward(Some(HumanGateDecision::Approve), false, true, Duration::from_secs(5)).await;
        let approval = received.expect("runner should receive an approval");
        assert_eq!(approval.resume_token, "approve__h");
        assert_eq!(approval.decision, Some(Value::String("approved".into())));
        assert!(!still_present, "resolved token must be removed from the map");
    }

    #[tokio::test]
    async fn forward_approval_reject_sends_none_decision() {
        let (received, _) =
            run_forward(Some(HumanGateDecision::Reject), false, true, Duration::from_secs(5)).await;
        let approval = received.expect("runner should receive the reject");
        assert_eq!(approval.decision, None, "reject maps to decision: None");
    }

    #[tokio::test]
    async fn forward_approval_retry_carries_feedback_as_value() {
        let (received, _) = run_forward(
            Some(HumanGateDecision::Retry { feedback: "use plan B".into() }),
            false,
            true,
            Duration::from_secs(5),
        )
        .await;
        let approval = received.expect("runner should receive the retry");
        assert_eq!(
            approval.decision,
            Some(Value::String("use plan B".into())),
            "retry feedback becomes the node's onward value"
        );
    }

    #[tokio::test]
    async fn forward_approval_dropped_sender_sends_none() {
        // Session aborted mid-approval → clear_session_approvals drops the Sender
        // → rx returns Err → forward_approval forwards a Reject (None) so the
        // runner fails the node now instead of waiting out its own 300s.
        let (received, _) =
            run_forward(None, true, true, Duration::from_secs(5)).await;
        let approval = received.expect("a dropped Sender must still produce a forward");
        assert_eq!(approval.decision, None, "dropped Sender → reject (None)");
    }

    #[tokio::test]
    async fn forward_approval_timeout_reclaims_and_does_not_send() {
        // Human never decides. After the (short) timeout the bridge must reclaim
        // the stale Sender and NOT send — letting the runner's own timeout fail
        // the node without a race. No decision reaches the runner.
        let (received, still_present) =
            run_forward(None, false, true, Duration::from_millis(50)).await;
        assert!(received.is_none(), "timeout must not forward anything to the runner");
        assert!(!still_present, "timed-out token must be reclaimed from the map");
    }

    #[tokio::test]
    async fn forward_approval_send_failed_reclaims_stale_token() {
        // Runner already gone (its receiver dropped) → send fails. The bridge
        // must reclaim so a LATE resolve returns NotFound instead of silently
        // succeeding against a dead run.
        let (received, still_present) =
            run_forward(Some(HumanGateDecision::Approve), false, false, Duration::from_secs(5)).await;
        assert!(received.is_none(), "no receiver alive to observe the send");
        assert!(!still_present, "stale token must be reclaimed after a failed send");
    }
}
