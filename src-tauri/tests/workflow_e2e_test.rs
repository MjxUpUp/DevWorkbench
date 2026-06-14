//! End-to-end PoC test: a complete prompt → agent → gate workflow.
//!
//! This exercises the full kernel pipeline without requiring a Tauri runtime or
//! real CLI processes:
//! - YAML definition parses → compiles into a Graph
//! - Graph runs topologically, emitting GraphEvent stream
//! - A MockExecutor stands in for the real KernelExecutor (agent runs become
//!   echo + upper-case; gate runs become a trivial pass)
//!
//! This is the proof that the eino-inspired Rust kernel is wired end-to-end
//! and that the v1.0 "Orchestrate" surface has a working engine under it.

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use kernel_compose::graph::{AgentChunk, AgentNodeSpec, Executor, GateNode};
use kernel_compose::{GraphEvent, WorkflowDef};
use serde_json::{json, Value};

struct MockExecutor;

#[async_trait]
impl Executor for MockExecutor {
    fn run_agent(
        &self,
        spec: &AgentNodeSpec,
        input: Value,
        _working_dir: Option<String>,
    ) -> Result<BoxStream<'static, Result<AgentChunk, String>>, String> {
        let task = spec
            .prompt
            .clone()
            .or_else(|| input.as_str().map(String::from))
            .unwrap_or_default();
        // Echo the task upper-cased, tagged with the agent name — a deterministic
        // stand-in for a real agent's output. Emit one Delta (forwarded as
        // NodeOutput) then the Final (becomes the node's output value).
        let final_val = json!({
            "agent": spec.agent,
            "output": task.to_uppercase(),
        });
        let chunks: Vec<Result<AgentChunk, String>> = vec![
            Ok(AgentChunk::Delta(json!({ "partial": task.to_uppercase() }))),
            Ok(AgentChunk::Final(final_val)),
        ];
        Ok(Box::pin(futures::stream::iter(chunks)))
    }

    async fn run_gate(
        &self,
        gate: &GateNode,
        input: Value,
        _working_dir: Option<String>,
    ) -> Result<Value, String> {
        Ok(json!({
            "gate": gate.gate,
            "status": "passed",
            "input_seen": input,
        }))
    }
}

const THREE_NODE_YAML: &str = r#"
start: prompt_1
end: gate_1
nodes:
  prompt_1:
    type: prompt
    text: "refactor the auth module"
  agent_1:
    type: agent
    agent: claude_code
    model: sonnet
  gate_1:
    type: gate
    gate: forge
edges:
  - { from: prompt_1, to: agent_1 }
  - { from: agent_1, to: gate_1 }
"#;

#[tokio::test]
async fn three_node_workflow_compiles_and_runs_end_to_end() {
    let compiled = WorkflowDef::parse_and_compile(THREE_NODE_YAML)
        .expect("YAML must parse and compile");

    let (mut stream, _approval_tx) = kernel_compose::run_graph_with_approvals(
        compiled,
        json!("initial"),
        None,
        Box::new(MockExecutor),
    );

    let mut events = Vec::new();
    while let Some(ev) = stream.next().await {
        let is_terminal = matches!(
            ev,
            GraphEvent::GraphDone { .. } | GraphEvent::GraphFailed { .. }
        );
        events.push(ev);
        if is_terminal {
            break;
        }
    }

    // The graph must reach DONE (not FAILED).
    let done = events.iter().find_map(|e| match e {
        GraphEvent::GraphDone { output } => Some(output.clone()),
        _ => None,
    });
    let output = done.expect("graph should complete with GraphDone");

    // All three nodes should have started and finished.
    let started: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            GraphEvent::NodeStart { node } => Some(node.clone()),
            _ => None,
        })
        .collect();
    assert!(started.iter().any(|n| n == "prompt_1"), "prompt node must start; got {started:?}");
    assert!(started.iter().any(|n| n == "agent_1"), "agent node must start; got {started:?}");
    assert!(started.iter().any(|n| n == "gate_1"), "gate node must start; got {started:?}");

    // The gate received the agent's (upper-cased) output.
    let gate_out = &output["input_seen"]["output"];
    assert_eq!(
        gate_out,
        &json!("REFACTOR THE AUTH MODULE"),
        "gate should see the agent's transformed output"
    );
}

#[tokio::test]
async fn branch_workflow_routes_conditionally() {
    let yaml = r#"
start: in
end: out
nodes:
  in:
    type: prompt
    text: "let's go now"
  branch_a:
    type: branch
    condition: "contains:go"
  go_path:
    type: transform
    op:
      truncate: 100
  stop_path:
    type: transform
    op:
      truncate: 0
  out:
    type: merge
    strategy: last_wins
edges:
  - { from: in, to: branch_a }
  - { from: branch_a, to: go_path, when: "contains:go" }
  - { from: branch_a, to: stop_path, when: "contains:stop" }
  - { from: go_path, to: out }
  - { from: stop_path, to: out }
"#;
    let compiled = WorkflowDef::parse_and_compile(yaml).expect("branch YAML compiles");
    let (mut stream, _) = kernel_compose::run_graph_with_approvals(
        compiled,
        json!("let's go now"),
        None,
        Box::new(MockExecutor),
    );

    let mut final_out = Value::Null;
    while let Some(ev) = stream.next().await {
        if let GraphEvent::GraphDone { output } = ev {
            final_out = output;
            break;
        }
        if let GraphEvent::GraphFailed { error } = ev {
            panic!("branch graph failed: {error}");
        }
    }
    // go_path fires because input contains "go"; stop_path is skipped.
    // The merge collects only non-skipped predecessors, so it takes go_path's value.
    assert_eq!(final_out, json!("let's go now"));
}
