//! Platform-mechanism eval (P4): deterministically exercise the kernel-compose
//! DAG engine itself — does it execute a graph in the expected order, skip the
//! expected branches, fail on the expected routing errors? No LLM: a stub
//! [`Executor`] echoes its input (agent) / passes (gate), so the verdict
//! reflects PURE engine mechanics (反刷分 #1: the objective fact is the
//! `GraphEvent` sequence the engine emitted, nothing the agent "said").
//!
//! This is distinct from an agent eval (did the agent pick the right tools?) —
//! here the agent is stubbed and the PLATFORM's routing / gate / skip / fail
//! behavior is the subject. kernel-compose's own `#[test]` suite covers the
//! same mechanics in-process; this driver surfaces it as a user-facing eval
//! verdict so a platform-behavior case can anchor a regression gate in the
//! panel. Compaction-behavior cases are not covered here (they need a
//! history-size scenario, left as a future extension).

use std::sync::Arc;

use futures::stream::BoxStream;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use kernel_compose::graph::{AgentChunk, AgentNodeSpec, Executor, GateNode};
use kernel_compose::{run_graph, GraphEvent, WorkflowDef};

/// A deterministic executor for mechanism evals. An agent node emits its
/// `prompt` (or stringified input) upper-cased as the Final value, so
/// downstream Merge / Transform / Branch nodes have a concrete value to route
/// on; every gate passes. No I/O, no LLM — the run's outcome depends only on
/// graph topology + branch / selector conditions, which is exactly the surface
/// a mechanism eval means to exercise.
struct StubExecutor;

#[async_trait::async_trait]
impl Executor for StubExecutor {
    fn run_agent(
        &self,
        spec: &AgentNodeSpec,
        input: Value,
        _working_dir: Option<String>,
    ) -> Result<BoxStream<'static, Result<AgentChunk, String>>, String> {
        let text = spec
            .prompt
            .clone()
            .or_else(|| input.as_str().map(String::from))
            .unwrap_or_default();
        Ok(Box::pin(futures::stream::iter(vec![Ok(
            AgentChunk::Final(Value::String(text.to_uppercase())),
        )])))
    }

    async fn run_gate(
        &self,
        gate: &GateNode,
        _input: Value,
        _working_dir: Option<String>,
    ) -> Result<Value, String> {
        Ok(serde_json::json!({ "gate": gate.gate, "passed": true }))
    }
}

/// Expected mechanism behavior — the deterministic contract a platform case
/// asserts. Either field may be empty (= "don't care" for that dimension).
/// `expect_order` is the expected `NodeStart` sequence. NOTE: the wave-parallel
/// runner may interleave same-wave independent nodes, so `expect_order` is only
/// deterministic for linear / branch / selector graphs (the common case); a
/// graph with a `Parallel` fan-out should leave `expect_order` empty and assert
/// on `expect_terminal` alone.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MechanismExpect {
    #[serde(default)]
    pub expect_order: Vec<String>,
    /// `"done"` | `"failed"` | `"interrupted"`. Empty = don't care.
    #[serde(default)]
    pub expect_terminal: String,
}

/// The verdict a platform-mechanism run produces. `pass` requires every
/// non-empty expectation to match the engine's actual behavior.
#[derive(Debug, Clone, Serialize)]
pub struct MechanismVerdict {
    pub pass: bool,
    pub actual_order: Vec<String>,
    pub actual_terminal: String,
    pub expected_order: Vec<String>,
    pub expected_terminal: String,
    /// Human-readable list of which expectations missed (empty on pass).
    pub mismatches: Vec<String>,
}

/// Run a platform-mechanism case: compile the YAML workflow, drive it with the
/// stub executor, collect the `GraphEvent` stream, and compare the observed
/// node-start order + terminal outcome against `expect`. Pure mechanics — no
/// LLM, no provider — so the verdict is a deterministic fact about the engine.
pub async fn run_platform_mechanism(
    yaml: &str,
    input: Value,
    expect: MechanismExpect,
) -> Result<MechanismVerdict, String> {
    let compiled = WorkflowDef::parse_and_compile(yaml)?;
    let mut stream = run_graph(compiled, input, None, Arc::new(StubExecutor));

    let mut actual_order: Vec<String> = Vec::new();
    let mut actual_terminal = String::from("unknown");
    while let Some(ev) = stream.next().await {
        match ev {
            GraphEvent::NodeStart { node } => actual_order.push(node),
            GraphEvent::GraphDone { .. } => actual_terminal = "done".into(),
            GraphEvent::GraphFailed { .. } => actual_terminal = "failed".into(),
            GraphEvent::GraphInterrupted { .. } => actual_terminal = "interrupted".into(),
            _ => {}
        }
    }

    let mut mismatches: Vec<String> = Vec::new();
    if !expect.expect_order.is_empty() && actual_order != expect.expect_order {
        mismatches.push(format!("order: {:?} ≠ {:?}", actual_order, expect.expect_order));
    }
    if !expect.expect_terminal.is_empty() && actual_terminal != expect.expect_terminal {
        mismatches.push(format!(
            "terminal: {actual_terminal} ≠ {}",
            expect.expect_terminal
        ));
    }

    Ok(MechanismVerdict {
        pass: mismatches.is_empty(),
        actual_order,
        actual_terminal,
        expected_order: expect.expect_order,
        expected_terminal: expect.expect_terminal,
        mismatches,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINEAR_YAML: &str = r#"
start: prompt_1
end: gate_1
nodes:
  prompt_1: { type: prompt, text: "refactor auth" }
  agent_1:  { type: agent,  agent: stub, prompt: "do work" }
  gate_1:   { type: gate,   gate: forge }
edges:
  - { from: prompt_1, to: agent_1 }
  - { from: agent_1,  to: gate_1 }
"#;

    #[tokio::test]
    async fn linear_graph_passes_when_order_and_terminal_match() {
        let v = run_platform_mechanism(
            LINEAR_YAML,
            Value::String("seed".into()),
            MechanismExpect {
                expect_order: vec!["prompt_1".into(), "agent_1".into(), "gate_1".into()],
                expect_terminal: "done".into(),
            },
        )
        .await
        .expect("linear graph runs");
        assert!(v.pass, "should pass: {:?}", v.mismatches);
        assert_eq!(v.actual_order, vec!["prompt_1", "agent_1", "gate_1"]);
        assert_eq!(v.actual_terminal, "done");
    }

    #[tokio::test]
    async fn wrong_expected_order_fails_with_mismatch() {
        let v = run_platform_mechanism(
            LINEAR_YAML,
            Value::String("seed".into()),
            MechanismExpect {
                expect_order: vec!["prompt_1".into(), "gate_1".into()],
                expect_terminal: "done".into(),
            },
        )
        .await
        .unwrap();
        assert!(!v.pass, "wrong order must fail");
        assert!(v.mismatches.iter().any(|m| m.contains("order")), "mismatch names order: {:?}", v.mismatches);
    }

    /// A graph whose END is fully branch-deselected must report terminal
    /// "failed" (run_graph's fail-fast on routing failure) — asserting this
    /// behavior is exactly the kind of platform-mechanism regression a case
    /// anchors. Reuses the kernel-compose `fully_deselected_end_fails` invariant.
    const DESELECTED_END_YAML: &str = r#"
start: in
end: out
nodes:
  in:  { type: prompt, text: "go" }
  br:  { type: branch, condition: "contains:go" }
  out: { type: merge }
edges:
  - { from: in, to: br }
  - { from: br, to: out, when: "contains:zzz" }
"#;

    #[tokio::test]
    async fn deselected_end_reports_failed_terminal() {
        let v = run_platform_mechanism(
            DESELECTED_END_YAML,
            Value::String("go".into()),
            MechanismExpect {
                expect_order: vec![],
                expect_terminal: "failed".into(),
            },
        )
        .await
        .unwrap();
        assert!(v.pass, "deselected END must be 'failed': {:?}", v.mismatches);
        assert_eq!(v.actual_terminal, "failed");
    }

    #[tokio::test]
    async fn malformed_yaml_returns_err_not_panic() {
        let err = run_platform_mechanism(
            "not: [valid yaml",
            Value::Null,
            MechanismExpect::default(),
        )
        .await
        .expect_err("malformed YAML must error");
        assert!(err.contains("YAML parse error") || err.contains("node"));
    }
}
