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
use std::sync::Arc;

use async_stream::stream;
use futures::stream::BoxStream;
use serde_json::Value;

use crate::events::NodeStatus;
use crate::graph::{
    AgentChunk, CompiledGraph, Executor, MergeStrategy, Node, NodeId, TransformNode,
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
    executor: Arc<dyn Executor>,
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
    executor: Arc<dyn Executor>,
) -> (BoxStream<'static, GraphEvent>, tokio::sync::mpsc::Sender<HumanApproval>) {
    let (approval_tx, approval_rx) = tokio::sync::mpsc::channel::<HumanApproval>(16);
    let stream = build_run_stream(compiled, input, working_dir, executor, approval_rx);
    (Box::pin(stream), approval_tx)
}

fn build_run_stream(
    compiled: CompiledGraph,
    input: Value,
    working_dir: Option<String>,
    executor: Arc<dyn Executor>,
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
        let mut ready: std::collections::VecDeque<NodeId> = std::collections::VecDeque::new();
        ready.push_back(start.clone());
        let mut outputs = outputs;
        // Nodes that were skipped by an un-taken branch edge. A skipped node is
        // NOT executed; it produces a Null value, emits a Skipped status, and
        // propagates the skip to its own successors (so a whole deselected path
        // is settled without running). This is the eino `reportSkip` analog.
        let mut skipped: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
        // Nodes that received at least one FIRING predecessor edge. A node with
        // no firing predecessor (all its in-edges were branch-deselected) is
        // skipped. This correctly handles diamond merge: if ONE predecessor
        // fired, the merge node runs (collecting that predecessor's output),
        // even if another predecessor was skipped.
        let mut has_fire_input: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
        // Pre-seed: the start node always has a (synthetic) firing input.
        has_fire_input.insert(start.clone());

        // Helper: settle a successor edge — record fired inputs, decrement
        // remaining, push to ready when all predecessors settled.
        let mut settle = |edge: &crate::graph::Edge,
                          node_value: Option<&Value>,
                          ready: &mut std::collections::VecDeque<NodeId>,
                          outputs: &mut HashMap<NodeId, Value>,
                          has_fire_input: &mut std::collections::HashSet<NodeId>| {
            if let Some(v) = node_value {
                outputs.entry(edge.to.clone()).or_insert_with(|| v.clone());
                has_fire_input.insert(edge.to.clone());
            }
            let r = remaining.get_mut(&edge.to).unwrap();
            if *r > 0 { *r -= 1; }
            if *r == 0 {
                ready.push_back(edge.to.clone());
            }
        };

        while let Some(nid) = ready.pop_front() {
            // A node is skipped iff NONE of its predecessors fired (no real
            // input). Diamond merge with one fired predecessor runs normally.
            let is_skipped = !has_fire_input.contains(&nid);
            if is_skipped {
                skipped.insert(nid.clone());
                let succs: Vec<crate::graph::Edge> = g.edges.iter()
                    .filter(|e| e.from == nid).cloned().collect();
                yield GraphEvent::NodeEnd {
                    node: nid.clone(),
                    status: NodeStatus::Skipped,
                    error: None,
                };
                if nid == end {
                    // END was skipped — EVERY predecessor was branch-deselected,
                    // so no path reached END. That is a ROUTING FAILURE, not a
                    // successful empty completion: emitting GraphDone{Null} would
                    // let the frontend treat a fully deselected workflow as
                    // success (silent wrong-path). Fail the graph instead.
                    yield GraphEvent::GraphFailed {
                        error: "end node unreachable: all predecessors skipped (no path taken)".into(),
                    };
                    return;
                }
                for edge in &succs {
                    settle(edge, None, &mut ready, &mut outputs, &mut has_fire_input);
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

            let incoming = outputs.get(&nid).cloned().unwrap_or(Value::Null);

            // Interrupt node: an unconditional or condition-gated halt of the
            // whole graph run. Handled BEFORE the result match so a firing
            // interrupt can short-circuit the stream (emit + return) instead of
            // producing a value. A non-firing condition falls through to the
            // match, where Interrupt passes its input through unchanged.
            if let Node::Interrupt(it) = &node {
                let fire = match &it.condition {
                    Some(cond) => eval_branch(&incoming, cond),
                    None => true,
                };
                if fire {
                    yield GraphEvent::NodeEnd {
                        node: nid.clone(),
                        status: NodeStatus::Interrupted,
                        error: None,
                    };
                    yield GraphEvent::GraphInterrupted { reason: it.message.clone() };
                    return;
                }
            }

            let result: Result<Value, String> = match &node {
                Node::Prompt(p) => Ok(Value::String(p.text.clone())),
                Node::Agent(spec) => {
                    // Streamed: drive the chunk stream, forwarding each Delta as
                    // a `NodeOutput` event and collecting the single `Final` as
                    // this node's output value. An `Err` chunk (or a failed
                    // stream construction) fails the node immediately.
                    match executor.run_agent(spec, incoming.clone(), working_dir.clone()) {
                        Ok(chunk_stream) => {
                            use futures::StreamExt;
                            let mut s = chunk_stream;
                            let mut final_val = Value::Null;
                            let mut agent_err: Option<String> = None;
                            while let Some(chunk_res) = s.next().await {
                                match chunk_res {
                                    Ok(AgentChunk::Delta(chunk)) => {
                                        yield GraphEvent::NodeOutput {
                                            node: nid.clone(),
                                            chunk,
                                        };
                                    }
                                    Ok(AgentChunk::Final(v)) => {
                                        final_val = v;
                                    }
                                    Err(e) => {
                                        agent_err = Some(e);
                                        break;
                                    }
                                }
                            }
                            match agent_err {
                                Some(e) => Err(e),
                                None => Ok(final_val),
                            }
                        }
                        Err(e) => Err(e),
                    }
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
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(300),
                        approval_rx.recv(),
                    ).await {
                        Ok(Some(approval)) if approval.resume_token == resume_token => {
                            match approval.decision {
                                Some(v) => Ok(v),
                                None => Err("human rejected".into()),
                            }
                        }
                        Ok(_) => Err("approval channel closed".into()),
                        Err(_) => Err("human approval timed out (300s)".into()),
                    }
                }
                Node::Transform(t) => Ok(apply_transform(t, incoming.clone())),
                Node::Branch(_) => Ok(incoming.clone()),
                // Only reached when an Interrupt's condition did NOT fire (a
                // firing interrupt returns above). Pass the input through.
                Node::Interrupt(_) => Ok(incoming.clone()),
                Node::Selector(s) => {
                    // First-match over cases (mutually exclusive). Emits the
                    // chosen label as the output value; successors route via
                    // branch edges whose `when` equals this label (see the
                    // fire logic below).
                    let chosen = s
                        .cases
                        .iter()
                        .find(|c| eval_branch(&incoming, &c.when))
                        .map(|c| c.label.clone())
                        .or_else(|| s.default.clone())
                        .unwrap_or_default();
                    Ok(Value::String(chosen))
                }
                Node::Loop(lp) => {
                    // Resolve iteration items: the array at `over` (dot-path),
                    // else a fixed `count`, else none (zero iterations).
                    let raw: Vec<Value> = match &lp.over {
                        Some(path) => extract_array(&incoming, path),
                        None => match lp.count {
                            Some(n) => (0..n).map(Value::from).collect(),
                            None => Vec::new(),
                        },
                    };
                    let cap = lp.max_iterations.unwrap_or(LOOP_DEFAULT_MAX_ITERATIONS);
                    let mut results: Vec<Value> = Vec::new();
                    let mut loop_err: Option<String> = None;
                    for item in raw.into_iter().take(cap) {
                        let body = match lp.body.clone().compile() {
                            Ok(c) => c,
                            Err(e) => {
                                loop_err = Some(format!("loop body: {e}"));
                                break;
                            }
                        };
                        match run_graph_to_completion(
                            body,
                            item,
                            working_dir.clone(),
                            executor.clone(),
                        )
                        .await
                        {
                            Ok(v) => results.push(v),
                            Err(e) => {
                                loop_err = Some(e);
                                break;
                            }
                        }
                    }
                    match loop_err {
                        Some(e) => Err(e),
                        None => Ok(merge_values(results, &lp.strategy)),
                    }
                }
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
                            // Selector emitted its chosen label as `v`; a
                            // successor branch edge fires iff its `when` equals
                            // that label (first-match, mutually exclusive).
                            (Node::Selector(_), Some(when_val)) => eval_branch(&v, when_val),
                            _ => true,
                        };
                        // fire → settle with value, parent not skipped.
                        // !fire (branch deselected) → settle WITHOUT value and mark
                        // the successor skipped (cascade).
                        settle(edge, if fire { Some(&v) } else { None },
                               &mut ready, &mut outputs, &mut has_fire_input);
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

/// Default cap on Loop iterations when `max_iterations` is unset. Bounds
/// runaway loops over unexpectedly large input arrays.
const LOOP_DEFAULT_MAX_ITERATIONS: usize = 1000;

/// Extract a JSON array at a dot-separated `path` from `input`. A missing path
/// or a non-array value yields an empty vec — the loop then runs zero times.
fn extract_array(input: &Value, path: &str) -> Vec<Value> {
    let mut cur = input;
    for seg in path.split('.') {
        cur = cur.get(seg).unwrap_or(&Value::Null);
    }
    cur.as_array().cloned().unwrap_or_default()
}

/// Drive a sub-graph (a Loop body) to completion, returning its `GraphDone`
/// output. A `GraphFailed` / `GraphInterrupted` from the body propagates as
/// `Err`. Only the Loop node uses this; the top-level run streams events via
/// `build_run_stream` instead. A Loop body containing a Human node will hang
/// here (no approval channel) — that is a documented limitation.
async fn run_graph_to_completion(
    compiled: CompiledGraph,
    input: Value,
    working_dir: Option<String>,
    executor: Arc<dyn Executor>,
) -> Result<Value, String> {
    use futures::StreamExt;
    let mut s = run_graph(compiled, input, working_dir, executor);
    while let Some(ev) = s.next().await {
        match ev {
            GraphEvent::GraphDone { output } => return Ok(output),
            GraphEvent::GraphFailed { error } => return Err(error),
            GraphEvent::GraphInterrupted { reason } => {
                return Err(format!("interrupted: {reason}"))
            }
            _ => {}
        }
    }
    Err("sub-graph ended without a terminal event".into())
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
    // ---- m8: execution-path tests (previously only pure functions were tested) ----

    /// A deterministic executor: agent echoes input upper-cased, gate passes.
    struct MockExec;
    #[async_trait::async_trait]
    impl crate::graph::Executor for MockExec {
        fn run_agent(
            &self,
            spec: &crate::graph::AgentNodeSpec,
            input: Value,
            _wd: Option<String>,
        ) -> Result<futures::stream::BoxStream<'static, Result<crate::graph::AgentChunk, String>>, String> {
            let t = spec.prompt.clone()
                .or_else(|| input.as_str().map(String::from))
                .unwrap_or_default();
            let final_val = json!({ "agent": spec.agent, "out": t.to_uppercase() });
            // Emit one Delta (observable in stream-forwarding tests) then Final.
            let chunks: Vec<Result<crate::graph::AgentChunk, String>> = vec![
                Ok(crate::graph::AgentChunk::Delta(json!({ "partial": t.to_uppercase() }))),
                Ok(crate::graph::AgentChunk::Final(final_val)),
            ];
            Ok(Box::pin(futures::stream::iter(chunks)))
        }
        async fn run_gate(&self, gate: &crate::graph::GateNode, input: Value, _wd: Option<String>) -> Result<Value, String> {
            Ok(json!({ "gate": gate.gate, "passed": true, "saw": input }))
        }
    }

    async fn collect_events(stream: impl futures::Stream<Item = GraphEvent>) -> Vec<GraphEvent> {
        use futures::StreamExt;
        let mut s = Box::pin(stream);
        let mut out = Vec::new();
        while let Some(ev) = s.next().await {
            let terminal = matches!(
                ev,
                GraphEvent::GraphDone { .. }
                    | GraphEvent::GraphFailed { .. }
                    | GraphEvent::GraphInterrupted { .. }
            );
            out.push(ev);
            if terminal { break; }
        }
        out
    }

    #[tokio::test]
    async fn linear_graph_executes_in_fifo_order() {
        use crate::graph::{GraphBuilder, MergeNode, Node, PromptNode};
        use std::collections::HashMap;
        let g = GraphBuilder::new()
            .node("p", Node::Prompt(PromptNode { text: "start".into(), vars: HashMap::new() }))
            .node("a", Node::Agent(crate::graph::AgentNodeSpec { agent: "mock".into(), model: None, prompt: None, resume_from: None }))
            .node("e", Node::Merge(MergeNode::default()))
            .edge("p", "a").edge("a", "e")
            .start("p").end("e").build().unwrap();
        let compiled = g.compile().unwrap();
        let events = collect_events(run_graph(compiled, json!("in"), None, Arc::new(MockExec))).await;
        let started: Vec<String> = events.iter().filter_map(|e| match e {
            GraphEvent::NodeStart { node } => Some(node.clone()), _ => None,
        }).collect();
        assert_eq!(started, vec!["p", "a", "e"], "FIFO order: got {started:?}");
        assert!(events.iter().any(|e| matches!(e, GraphEvent::GraphDone { .. })));
    }

    #[tokio::test]
    async fn branch_deselect_skips_unreached_path() {
        use crate::graph::{BranchNode, GraphBuilder, MergeNode, MergeStrategy, Node, PromptNode, TransformNode};
        let g = GraphBuilder::new()
            .node("in", Node::Prompt(PromptNode { text: "go".into(), vars: HashMap::new() }))
            .node("br", Node::Branch(BranchNode { condition: "contains:go".into() }))
            .node("go_path", Node::Transform(TransformNode { op: crate::graph::TransformOp::Truncate(100) }))
            .node("stop_path", Node::Transform(TransformNode { op: crate::graph::TransformOp::Truncate(0) }))
            .node("out", Node::Merge(MergeNode { strategy: MergeStrategy::LastWins }))
            .edge("in", "br")
            .branch_edge("br", "go_path", "contains:go")
            .branch_edge("br", "stop_path", "contains:stop")
            .edge("go_path", "out").edge("stop_path", "out")
            .start("in").end("out").build().unwrap();
        let compiled = g.compile().unwrap();
        let events = collect_events(run_graph(compiled, json!("x"), None, Arc::new(MockExec))).await;
        assert!(events.iter().any(|e| matches!(e,
            GraphEvent::NodeEnd { node, status: NodeStatus::Skipped, .. } if node == "stop_path")),
            "stop_path should be skipped");
        assert!(events.iter().any(|e| matches!(e,
            GraphEvent::NodeEnd { node, status: NodeStatus::Done, .. } if node == "go_path")),
            "go_path should be done");
    }

    #[tokio::test]
    async fn diamond_merge_collects_all_predecessor_outputs() {
        // H1 regression test: two predecessors both fire into a Merge(Concat);
        // the merge output must contain BOTH values (not empty/null).
        use crate::graph::{AgentNodeSpec, GraphBuilder, MergeNode, MergeStrategy, Node};
        // Diamond: prompt fans out to two agents, both merge into one node.
        // Collect strategy gathers both predecessor outputs into an array.
        let g = GraphBuilder::new()
            .node("p", Node::Prompt(crate::graph::PromptNode { text: "go".into(), vars: std::collections::HashMap::new() }))
            .node("a1", Node::Agent(AgentNodeSpec { agent: "x".into(), model: None, prompt: Some("alpha".into()), resume_from: None }))
            .node("a2", Node::Agent(AgentNodeSpec { agent: "y".into(), model: None, prompt: Some("beta".into()), resume_from: None }))
            .node("m", Node::Merge(MergeNode { strategy: MergeStrategy::Collect }))
            .edge("p", "a1").edge("p", "a2").edge("a1", "m").edge("a2", "m")
            .start("p").end("m").build().unwrap();
        let compiled = g.compile().unwrap();
        let events = collect_events(run_graph(compiled, json!("in"), None, Arc::new(MockExec))).await;
        let done = events.iter().find_map(|e| match e {
            GraphEvent::GraphDone { output } => Some(output.clone()), _ => None,
        }).expect("graph must complete");
        // MockExec returns json objects; Collect gathers them into an array.
        // The key assertion: merge received BOTH predecessors (not empty/null).
        assert!(done != Value::Null && done != json!(""), "merge output must not be null/empty: {done}");
        // With Collect strategy it's an array of 2 predecessor outputs.
        if let Some(arr) = done.as_array() {
            assert_eq!(arr.len(), 2, "Collect should have 2 preds: {arr:?}");
        }
    }

    /// Gap-④ proof: an agent node must emit `NodeOutput` events for each Delta
    /// chunk (previously NodeOutput was emitted nowhere), and its Final chunk
    /// must become the node's output value.
    #[tokio::test]
    async fn agent_node_forwards_delta_chunks_as_node_output() {
        use crate::graph::{AgentNodeSpec, GraphBuilder, MergeNode, MergeStrategy, Node};
        let g = GraphBuilder::new()
            .node("p", Node::Prompt(crate::graph::PromptNode {
                text: "seed".into(),
                vars: std::collections::HashMap::new(),
            }))
            .node("a", Node::Agent(AgentNodeSpec {
                agent: "mock".into(),
                model: None,
                prompt: Some("hello".into()),
                resume_from: None,
            }))
            // LastWins picks the agent's Final (a JSON object) — Concat would
            // drop non-string values and yield empty, which is what made the
            // first version of this test fail spuriously.
            .node("e", Node::Merge(MergeNode { strategy: MergeStrategy::LastWins }))
            .edge("p", "a").edge("a", "e")
            .start("p").end("e").build().unwrap();
        let compiled = g.compile().unwrap();
        let events = collect_events(run_graph(compiled, json!("in"), None, Arc::new(MockExec))).await;

        // NodeOutput for node "a" must have been emitted (Delta forwarded).
        let outputs: Vec<&GraphEvent> = events
            .iter()
            .filter(|e| matches!(e, GraphEvent::NodeOutput { node, .. } if node == "a"))
            .collect();
        assert!(!outputs.is_empty(), "agent node should emit NodeOutput deltas: {events:?}");

        // Final chunk becomes the node output → reaches GraphDone.
        let done = events.iter().find_map(|e| match e {
            GraphEvent::GraphDone { output } => Some(output.clone()),
            _ => None,
        }).expect("graph must complete");
        assert_eq!(done["out"], "HELLO", "Final chunk must propagate as node output: {done}");
    }

    /// An agent stream that yields an `Err` chunk fails the node and the graph.
    #[tokio::test]
    async fn agent_stream_error_fails_the_node() {
        use crate::graph::{AgentNodeSpec, GraphBuilder, MergeNode, Node};
        struct FailExec;
        #[async_trait::async_trait]
        impl crate::graph::Executor for FailExec {
            fn run_agent(
                &self,
                _spec: &crate::graph::AgentNodeSpec,
                _input: Value,
                _wd: Option<String>,
            ) -> Result<futures::stream::BoxStream<'static, Result<crate::graph::AgentChunk, String>>, String> {
                let chunks: Vec<Result<crate::graph::AgentChunk, String>> = vec![
                    Ok(crate::graph::AgentChunk::Delta(json!("partial"))),
                    Err("simulated agent failure".into()),
                ];
                Ok(Box::pin(futures::stream::iter(chunks)))
            }
            async fn run_gate(&self, _gate: &crate::graph::GateNode, _input: Value, _wd: Option<String>) -> Result<Value, String> {
                Ok(Value::Null)
            }
        }
        let g = GraphBuilder::new()
            .node("a", Node::Agent(AgentNodeSpec {
                agent: "fail".into(),
                model: None,
                prompt: Some("x".into()),
                resume_from: None,
            }))
            .node("e", Node::Merge(MergeNode::default()))
            .edge("a", "e")
            .start("a").end("e").build().unwrap();
        let compiled = g.compile().unwrap();
        let events = collect_events(run_graph(compiled, json!("in"), None, Arc::new(FailExec))).await;
        assert!(
            events.iter().any(|e| matches!(e, GraphEvent::GraphFailed { .. })),
            "agent Err chunk must fail the graph: {events:?}"
        );
    }

    // ---- C3: loop / selector / interrupt control-flow nodes ----

    /// An unconditional Interrupt node halts the run with `GraphInterrupted`
    /// and successors never execute.
    #[tokio::test]
    async fn interrupt_node_halts_the_graph() {
        use crate::graph::{
            GraphBuilder, InterruptNode, Node, PromptNode, TransformNode,
        };
        use std::collections::HashMap;
        let g = GraphBuilder::new()
            .node("p", Node::Prompt(PromptNode { text: "go".into(), vars: HashMap::new() }))
            .node("br", Node::Interrupt(InterruptNode { message: "stop here".into(), condition: None }))
            .node("after", Node::Transform(TransformNode { op: crate::graph::TransformOp::Truncate(0) }))
            .edge("p", "br").edge("br", "after")
            .start("p").end("after").build().unwrap();
        let compiled = g.compile().unwrap();
        let events = collect_events(run_graph(compiled, json!("in"), None, Arc::new(MockExec))).await;
        assert!(
            events.iter().any(|e| matches!(e, GraphEvent::GraphInterrupted { reason } if reason == "stop here")),
            "expected GraphInterrupted: {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(e, GraphEvent::NodeEnd { node, status: NodeStatus::Interrupted, .. } if node == "br")),
            "interrupt node must end Interrupted: {events:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(e, GraphEvent::NodeStart { node } if node == "after")),
            "successor must not run after interrupt: {events:?}"
        );
    }

    /// A conditional Interrupt whose condition does not match passes through
    /// and the graph completes normally.
    #[tokio::test]
    async fn interrupt_with_unmet_condition_passes_through() {
        use crate::graph::{GraphBuilder, InterruptNode, MergeNode, Node, PromptNode};
        use std::collections::HashMap;
        let g = GraphBuilder::new()
            .node("p", Node::Prompt(PromptNode { text: "x".into(), vars: HashMap::new() }))
            .node("it", Node::Interrupt(InterruptNode { message: "no".into(), condition: Some("contains:go".into()) }))
            .node("e", Node::Merge(MergeNode::default()))
            .edge("p", "it").edge("it", "e")
            .start("p").end("e").build().unwrap();
        let compiled = g.compile().unwrap();
        let events = collect_events(run_graph(compiled, json!("in"), None, Arc::new(MockExec))).await;
        assert!(
            events.iter().any(|e| matches!(e, GraphEvent::GraphDone { .. })),
            "non-firing interrupt should let graph finish: {events:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(e, GraphEvent::GraphInterrupted { .. })),
            "no interrupt expected: {events:?}"
        );
    }

    /// Selector routes exactly one branch (first-match); the non-matching
    /// branch is skipped — mutually exclusive, unlike Branch.
    #[tokio::test]
    async fn selector_first_match_routes_exactly_one_branch() {
        use crate::graph::{
            GraphBuilder, MergeNode, MergeStrategy, Node, PromptNode, SelectorCase, SelectorNode,
            TransformNode,
        };
        use std::collections::HashMap;
        let g = GraphBuilder::new()
            .node("in", Node::Prompt(PromptNode { text: "go".into(), vars: HashMap::new() }))
            .node("sel", Node::Selector(SelectorNode {
                cases: vec![
                    SelectorCase { when: "contains:go".into(), label: "go".into() },
                    SelectorCase { when: "contains:stop".into(), label: "stop".into() },
                ],
                default: None,
            }))
            .node("go_path", Node::Transform(TransformNode { op: crate::graph::TransformOp::Truncate(100) }))
            .node("stop_path", Node::Transform(TransformNode { op: crate::graph::TransformOp::Truncate(0) }))
            .node("out", Node::Merge(MergeNode { strategy: MergeStrategy::LastWins }))
            .edge("in", "sel")
            .branch_edge("sel", "go_path", "go")
            .branch_edge("sel", "stop_path", "stop")
            .edge("go_path", "out").edge("stop_path", "out")
            .start("in").end("out").build().unwrap();
        let compiled = g.compile().unwrap();
        let events = collect_events(run_graph(compiled, json!("x"), None, Arc::new(MockExec))).await;
        assert!(
            events.iter().any(|e| matches!(e, GraphEvent::NodeEnd { node, status: NodeStatus::Done, .. } if node == "go_path")),
            "go_path should run: {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(e, GraphEvent::NodeEnd { node, status: NodeStatus::Skipped, .. } if node == "stop_path")),
            "stop_path should be skipped (mutual exclusion): {events:?}"
        );
        assert!(events.iter().any(|e| matches!(e, GraphEvent::GraphDone { .. })));
    }

    /// Loop iterates its body once per element of the input array and Collects
    /// the per-iteration outputs.
    #[tokio::test]
    async fn loop_iterates_over_input_array() {
        use crate::graph::{
            AgentNodeSpec, Edge, Graph, GraphBuilder, LoopNode, MergeNode, MergeStrategy, Node,
        };
        use crate::events::EdgeKind;
        use std::collections::HashMap;
        let body = Graph {
            nodes: HashMap::from([
                ("ba".to_string(), Node::Agent(AgentNodeSpec { agent: "mock".into(), model: None, prompt: None, resume_from: None })),
                ("be".to_string(), Node::Merge(MergeNode { strategy: MergeStrategy::LastWins })),
            ]),
            edges: vec![Edge { from: "ba".into(), to: "be".into(), kind: EdgeKind::Normal, when: None }],
            start: "ba".into(),
            end: "be".into(),
        };
        let g = GraphBuilder::new()
            .node("lp", Node::Loop(LoopNode {
                over: Some("items".into()),
                count: None,
                max_iterations: None,
                body,
                strategy: MergeStrategy::Collect,
            }))
            .start("lp").end("lp").build().unwrap();
        let compiled = g.compile().unwrap();
        let events = collect_events(run_graph(compiled, json!({ "items": ["a", "b", "c"] }), None, Arc::new(MockExec))).await;
        let done = events.iter().find_map(|e| match e {
            GraphEvent::GraphDone { output } => Some(output.clone()),
            _ => None,
        }).expect("loop graph must complete");
        let arr = done.as_array().expect("Collect should yield an array");
        assert_eq!(arr.len(), 3, "3 items → 3 iterations: {arr:?}");
        assert_eq!(arr[0]["out"], "A", "iteration 0 uppercased element: {arr:?}");
        assert_eq!(arr[1]["out"], "B");
        assert_eq!(arr[2]["out"], "C");
    }

    /// Loop with a fixed `count` runs that many iterations.
    #[tokio::test]
    async fn loop_runs_fixed_count() {
        use crate::graph::{
            AgentNodeSpec, Edge, Graph, GraphBuilder, LoopNode, MergeNode, MergeStrategy, Node,
        };
        use crate::events::EdgeKind;
        use std::collections::HashMap;
        let body = Graph {
            nodes: HashMap::from([
                ("ba".to_string(), Node::Agent(AgentNodeSpec { agent: "mock".into(), model: None, prompt: None, resume_from: None })),
                ("be".to_string(), Node::Merge(MergeNode { strategy: MergeStrategy::LastWins })),
            ]),
            edges: vec![Edge { from: "ba".into(), to: "be".into(), kind: EdgeKind::Normal, when: None }],
            start: "ba".into(),
            end: "be".into(),
        };
        let g = GraphBuilder::new()
            .node("lp", Node::Loop(LoopNode { over: None, count: Some(3), max_iterations: None, body, strategy: MergeStrategy::Collect }))
            .start("lp").end("lp").build().unwrap();
        let compiled = g.compile().unwrap();
        let events = collect_events(run_graph(compiled, json!("x"), None, Arc::new(MockExec))).await;
        let done = events.iter().find_map(|e| match e {
            GraphEvent::GraphDone { output } => Some(output.clone()),
            _ => None,
        }).expect("must complete");
        assert_eq!(done.as_array().map(|a| a.len()), Some(3), "count=3 → 3 iterations: {done:?}");
    }

    /// `max_iterations` caps a loop over an oversized array (runaway guard).
    #[tokio::test]
    async fn loop_max_iterations_caps_runaway() {
        use crate::graph::{
            AgentNodeSpec, Edge, Graph, GraphBuilder, LoopNode, MergeNode, MergeStrategy, Node,
        };
        use crate::events::EdgeKind;
        use std::collections::HashMap;
        let body = Graph {
            nodes: HashMap::from([
                ("ba".to_string(), Node::Agent(AgentNodeSpec { agent: "mock".into(), model: None, prompt: None, resume_from: None })),
                ("be".to_string(), Node::Merge(MergeNode { strategy: MergeStrategy::LastWins })),
            ]),
            edges: vec![Edge { from: "ba".into(), to: "be".into(), kind: EdgeKind::Normal, when: None }],
            start: "ba".into(),
            end: "be".into(),
        };
        let g = GraphBuilder::new()
            .node("lp", Node::Loop(LoopNode { over: Some("items".into()), count: None, max_iterations: Some(5), body, strategy: MergeStrategy::Collect }))
            .start("lp").end("lp").build().unwrap();
        let compiled = g.compile().unwrap();
        let items: Vec<String> = (0..100).map(|i| format!("v{i}")).collect();
        let events = collect_events(run_graph(compiled, json!({ "items": items }), None, Arc::new(MockExec))).await;
        let done = events.iter().find_map(|e| match e {
            GraphEvent::GraphDone { output } => Some(output.clone()),
            _ => None,
        }).expect("must complete");
        assert_eq!(done.as_array().map(|a| a.len()), Some(5), "max_iterations=5 must cap 100 items: {done:?}");
    }

    /// Regression: when EVERY predecessor of the END node is branch-deselected,
    /// END is skipped — and skipping END must FAIL the graph (GraphFailed), not
    /// emit GraphDone{Null} (which the frontend renders as success). A fully
    /// deselected workflow is a routing failure, not an empty success.
    #[tokio::test]
    async fn fully_deselected_end_fails_not_succeeds() {
        use crate::graph::{BranchNode, GraphBuilder, MergeNode, Node, PromptNode};
        use std::collections::HashMap;
        // `in`(prompt "go") → br(Branch) → out(Merge, end). The ONLY edge into
        // `out` carries `when: "contains:zzz"`, which "go" does not match → the
        // edge never fires → `out` has no firing predecessor → skipped → END
        // skipped → must GraphFailed (the old code emitted GraphDone{Null}).
        let g = GraphBuilder::new()
            .node("in", Node::Prompt(PromptNode { text: "go".into(), vars: HashMap::new() }))
            .node("br", Node::Branch(BranchNode { condition: "contains:go".into() }))
            .node("out", Node::Merge(MergeNode::default()))
            .edge("in", "br")
            .branch_edge("br", "out", "contains:zzz")
            .start("in").end("out").build().unwrap();
        let compiled = g.compile().unwrap();
        let events = collect_events(run_graph(compiled, json!("x"), None, Arc::new(MockExec))).await;
        assert!(
            events.iter().any(|e| matches!(e, GraphEvent::GraphFailed { .. })),
            "deselected END must fail, not succeed: {events:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(e, GraphEvent::GraphDone { .. })),
            "must NOT emit GraphDone for a fully deselected run: {events:?}"
        );
    }

}
