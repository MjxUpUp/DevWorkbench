//! Graph execution engine.
//!
//! Topological execution (DAG all-predecessor). For each node we evaluate its
//! predecessors, run the node, emit [`crate::GraphEvent`]s, and propagate the
//! output value to successors. Human nodes pause and await approval via the
//! approval channel.
//!
//! This is the push-based stream analog of eino's `compose/graph_run.go`, but
//! far simpler — no Pregel supersteps, no channel abstraction, just a worklist.

use std::collections::HashMap;

use async_stream::stream;
use futures::stream::BoxStream;
use serde_json::Value;

use crate::events::NodeStatus;
use crate::graph::{
    CompiledGraph, Executor, MergeStrategy, Node, NodeId, TransformNode,
};
use crate::GraphEvent;

/// Approval for a paused Human node. Sent back into the run via the approval
/// channel returned by [`run_graph`].
#[derive(Debug, Clone)]
pub struct HumanApproval {
    pub resume_token: String,
    /// None = rejected (graph fails this node).
    pub decision: Option<Value>,
}

/// Run a compiled graph WITHOUT human-approval support (Human nodes will hang).
/// For graphs containing Human nodes, use [`run_graph_with_approvals`] to obtain
/// the approval sender.
pub fn run_graph(
    compiled: CompiledGraph,
    input: Value,
    working_dir: Option<String>,
    executor: Box<dyn Executor>,
) -> BoxStream<'static, GraphEvent> {
    run_graph_with_approvals(compiled, input, working_dir, executor).0
}

/// Run a graph, returning the event stream AND the approval sender. When a
/// Human node pauses, the stream emits `ApprovalRequired` carrying a
/// `resume_token`; the caller sends a [`HumanApproval`] with the matching token
/// to resume (or `decision: None` to reject).
pub fn run_graph_with_approvals(
    compiled: CompiledGraph,
    input: Value,
    working_dir: Option<String>,
    executor: Box<dyn Executor>,
) -> (BoxStream<'static, GraphEvent>, tokio::sync::mpsc::Sender<HumanApproval>) {
    let (approval_tx, approval_rx) = tokio::sync::mpsc::channel::<HumanApproval>(16);
    let stream = build_run_stream(compiled, input, working_dir, executor, approval_rx);
    (Box::pin(stream), approval_tx)
}

fn build_run_stream(
    compiled: CompiledGraph,
    input: Value,
    working_dir: Option<String>,
    executor: Box<dyn Executor>,
    mut approval_rx: tokio::sync::mpsc::Receiver<HumanApproval>,
) -> impl futures::Stream<Item = GraphEvent> {
    let g = compiled.graph.clone();
    let start = g.start.clone();
    let end = g.end.clone();

    let mut outputs: HashMap<NodeId, Value> = HashMap::new();
    outputs.insert(start.clone(), input);

    let mut remaining: HashMap<NodeId, usize> = HashMap::new();
    let mut preds: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for nid in g.nodes.keys() {
        preds.entry(nid.clone()).or_default();
        remaining.entry(nid.clone()).or_insert(0);
    }
    for e in &g.edges {
        *remaining.get_mut(&e.to).unwrap() += 1;
        preds.entry(e.to.clone()).or_default().push(e.from.clone());
    }

    stream! {
        let mut ready: Vec<NodeId> = vec![start.clone()];
        let mut outputs = outputs;
        // Nodes that were skipped by an un-taken branch edge. A skipped node is
        // NOT executed; it produces a Null value, emits a Skipped status, and
        // propagates the skip to its own successors (so a whole deselected path
        // is settled without running). This is the eino `reportSkip` analog.
        let mut skipped: std::collections::HashSet<NodeId> = std::collections::HashSet::new();

        // Helper: settle a node's successor edge — decrement remaining, push when
        // ready, and propagate skip if the current node was skipped.
        let mut settle = |edge: &crate::graph::Edge,
                          node_value: Option<&Value>,
                          parent_skipped: bool,
                          ready: &mut Vec<NodeId>,
                          outputs: &mut HashMap<NodeId, Value>,
                          skipped: &mut std::collections::HashSet<NodeId>| {
            if let Some(v) = node_value {
                outputs.entry(edge.to.clone()).or_insert_with(|| v.clone());
            }
            let r = remaining.get_mut(&edge.to).unwrap();
            if *r > 0 { *r -= 1; }
            if *r == 0 {
                if parent_skipped {
                    // Parent was skipped → this successor has no real input from
                    // this path. Mark it skipped too (cascade) and settle it
                    // without executing.
                    skipped.insert(edge.to.clone());
                    ready.push(edge.to.clone());
                } else {
                    ready.push(edge.to.clone());
                }
            }
        };

        while let Some(nid) = ready.pop() {
            // If this node was marked skipped by a branch, settle it without
            // executing — emit Skipped, propagate to its successors.
            if skipped.contains(&nid) {
                let succs: Vec<crate::graph::Edge> = g.edges.iter()
                    .filter(|e| e.from == nid).cloned().collect();
                yield GraphEvent::NodeEnd {
                    node: nid.clone(),
                    status: NodeStatus::Skipped,
                    error: None,
                };
                if nid == end {
                    yield GraphEvent::GraphDone { output: Value::Null };
                    return;
                }
                for edge in &succs {
                    settle(edge, None, true, &mut ready, &mut outputs, &mut skipped);
                }
                continue;
            }

            let node = match g.nodes.get(&nid) {
                Some(n) => n.clone(),
                None => {
                    yield GraphEvent::GraphFailed { error: format!("missing node {nid}") };
                    return;
                }
            };
            yield GraphEvent::NodeStart { node: nid.clone() };

            let incoming = outputs.remove(&nid).unwrap_or(Value::Null);

            let result: Result<Value, String> = match &node {
                Node::Prompt(p) => Ok(Value::String(p.text.clone())),
                Node::Agent(spec) => {
                    executor.run_agent(spec, incoming.clone(), working_dir.clone()).await
                }
                Node::Gate(gate) => {
                    executor.run_gate(gate, incoming.clone(), working_dir.clone()).await
                }
                Node::Merge(m) => {
                    // Collect only NON-skipped predecessors' outputs.
                    let pred_vals: Vec<Value> = preds.get(&nid).cloned().unwrap_or_default()
                        .into_iter()
                        .filter(|p| !skipped.contains(p))
                        .filter_map(|p| outputs.get(&p).cloned())
                        .collect();
                    Ok(merge_values(pred_vals, &m.strategy))
                }
                Node::Parallel(_) => Ok(incoming.clone()),
                Node::Human(h) => {
                    let resume_token = format!("approve__{nid}");
                    yield GraphEvent::ApprovalRequired {
                        node: nid.clone(),
                        prompt: h.prompt.clone(),
                        resume_token: resume_token.clone(),
                    };
                    match approval_rx.recv().await {
                        Some(approval) if approval.resume_token == resume_token => {
                            match approval.decision {
                                Some(v) => Ok(v),
                                None => Err("human rejected".into()),
                            }
                        }
                        _ => Err("approval channel closed".into()),
                    }
                }
                Node::Transform(t) => Ok(apply_transform(t, incoming.clone())),
                Node::Branch(_) => Ok(incoming.clone()),
            };

            match result {
                Ok(v) => {
                    outputs.insert(nid.clone(), v.clone());
                    yield GraphEvent::NodeEnd { node: nid.clone(), status: NodeStatus::Done, error: None };

                    if nid == end {
                        yield GraphEvent::GraphDone { output: v };
                        return;
                    }

                    let succs: Vec<crate::graph::Edge> = g.edges.iter()
                        .filter(|e| e.from == nid).cloned().collect();
                    for edge in &succs {
                        let fire = match (&node, &edge.when) {
                            (Node::Branch(_), Some(when_val)) => eval_branch(&incoming, when_val),
                            _ => true,
                        };
                        // fire → settle with value, parent not skipped.
                        // !fire (branch deselected) → settle WITHOUT value and mark
                        // the successor skipped (cascade).
                        settle(edge, if fire { Some(&v) } else { None }, !fire,
                               &mut ready, &mut outputs, &mut skipped);
                    }
                }
                Err(e) => {
                    yield GraphEvent::NodeEnd { node: nid.clone(), status: NodeStatus::Failed, error: Some(e.clone()) };
                    yield GraphEvent::GraphFailed { error: e };
                    return;
                }
            }
        }
        yield GraphEvent::GraphFailed { error: "graph ended without reaching END".into() };
    }
}

fn merge_values(vals: Vec<Value>, strategy: &MergeStrategy) -> Value {
    match strategy {
        MergeStrategy::Concat => {
            let parts: Vec<String> = vals.iter().filter_map(|v| v.as_str().map(String::from)).collect();
            Value::String(parts.join("\n"))
        }
        MergeStrategy::LastWins => {
            vals.into_iter().rev().find(|v| !v.is_null()).unwrap_or(Value::Null)
        }
        MergeStrategy::Collect => Value::Array(vals),
    }
}

fn apply_transform(t: &TransformNode, input: Value) -> Value {
    use crate::graph::TransformOp;
    match &t.op {
        TransformOp::Extract(path) => {
            let mut cur = &input;
            for seg in path.split('.') {
                cur = cur.get(seg).unwrap_or(&Value::Null);
            }
            cur.clone()
        }
        TransformOp::Wrap { prefix, suffix } => {
            let s = input.as_str().unwrap_or("").to_string();
            Value::String(format!("{prefix}{s}{suffix}"))
        }
        TransformOp::Truncate(n) => {
            let s = input.as_str().unwrap_or("");
            Value::String(s.chars().take(*n).collect())
        }
    }
}

/// Minimal branch evaluator. `when_val` is the expected string; the incoming
/// value is matched as: exact string equals, or "contains:substr" semantics
/// encoded in when_val.
fn eval_branch(input: &Value, when_val: &str) -> bool {
    let s = input.as_str().unwrap_or("");
    if let Some(substr) = when_val.strip_prefix("contains:") {
        s.contains(substr)
    } else {
        s == when_val
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GateNode, MergeStrategy};
    use serde_json::json;

    #[test]
    fn merge_concat_joins_strings() {
        let v = merge_values(vec![json!("a"), json!("b")], &MergeStrategy::Concat);
        assert_eq!(v, json!("a\nb"));
    }

    #[test]
    fn merge_last_wins_picks_last_non_null() {
        let v = merge_values(
            vec![json!("a"), Value::Null, json!("c")],
            &MergeStrategy::LastWins,
        );
        assert_eq!(v, json!("c"));
    }

    #[test]
    fn transform_extract_walks_path() {
        let t = TransformNode { op: crate::graph::TransformOp::Extract("a.b".into()) };
        let out = apply_transform(&t, json!({"a": {"b": 42}}));
        assert_eq!(out, json!(42));
    }

    #[test]
    fn transform_truncate_snaps_to_chars() {
        let t = TransformNode { op: crate::graph::TransformOp::Truncate(3) };
        let out = apply_transform(&t, json!("héllo世界"));
        let s = out.as_str().unwrap();
        assert_eq!(s.chars().count(), 3);
    }

    #[test]
    fn branch_eval_supports_contains_and_equals() {
        assert!(eval_branch(&json!("hello world"), "contains:world"));
        assert!(!eval_branch(&json!("hello"), "contains:world"));
        assert!(eval_branch(&json!("ok"), "ok"));
    }

    #[test]
    fn gate_node_serializes_with_config() {
        let g = GateNode { gate: "forge".into(), config: json!({"strict": true}) };
        let v: Value = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(v["gate"], "forge");
        assert_eq!(v["config"]["strict"], true);
    }
}
