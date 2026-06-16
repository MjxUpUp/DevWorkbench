//! Graph data model + builder + compile.
//!
//! A [`Graph`] is a DAG of [`Node`]s connected by [`Edge`]s. It is built with
//! [`GraphBuilder`], then [`Graph::compile`] produces a [`CompiledGraph`] that
//! can be run via [`crate::run_graph`].
//!
//! Nodes that need external capability (running an agent, invoking a quality
//! gate) do NOT hold the executor directly — that would couple this crate to
//! concrete implementations. Instead they carry a *spec* (agent kind, gate id)
//! and the actual executor is injected at run time via the [`Executor`] trait
//! (implemented by the host application).

use std::collections::HashMap;

use futures::stream::BoxStream;
use kernel_core::AgentInput;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::events::EdgeKind;

pub type NodeId = String;

/// Discriminator for the 7 node types + branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    Prompt,
    Agent,
    Gate,
    Parallel,
    Merge,
    Human,
    Transform,
    Branch,
}

/// A graph node. Spec nodes carry their config inline; capability nodes
/// (Agent/Gate) carry only a spec — the executor resolves them at run time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    /// Emits a fixed/templated prompt. Seeds the graph with the task.
    Prompt(PromptNode),
    /// Runs an agent (opaque CLI or transparent ReactAgent).
    Agent(AgentNodeSpec),
    /// Runs a quality gate (Forge, honesty check, …).
    Gate(GateNode),
    /// Fan-out: spawns all successor branches concurrently.
    Parallel(ParallelNode),
    /// Fan-in: waits for all predecessors, merges their outputs.
    Merge(MergeNode),
    /// Pauses for human approval before continuing.
    Human(HumanNode),
    /// Pure data transform (lambda over the node input value).
    Transform(TransformNode),
    /// Conditional routing — selects which successors fire.
    Branch(BranchNode),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptNode {
    pub text: String,
    /// Optional variables to substitute ({{name}}) from graph input.
    #[serde(default)]
    pub vars: HashMap<String, Value>,
}

/// Spec for an agent node. The host's Executor resolves `agent` + `model` into
/// a concrete `Box<dyn Agent>` at run time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentNodeSpec {
    /// Agent identifier, e.g. "claude_code", "codex", "react" (transparent).
    pub agent: String,
    #[serde(default)]
    pub model: Option<String>,
    /// Carry prompt explicitly; falls back to incoming edge value if absent.
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_from: Option<String>,
}

impl AgentNodeSpec {
    /// Build an AgentInput from this spec + the graph's incoming value.
    pub fn to_input(&self, working_dir: Option<String>) -> AgentInput {
        AgentInput {
            prompt: self.prompt.clone().unwrap_or_default(),
            working_dir,
            model: self.model.clone(),
            resume_from: self.resume_from.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateNode {
    /// Gate identifier: "forge", "honesty", "compile", "test", …
    pub gate: String,
    #[serde(default)]
    pub config: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParallelNode {
    /// Number of branches expected to fan out (informational; actual fan-out
    /// is determined by outgoing edges).
    #[serde(default)]
    pub branches: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MergeNode {
    /// Strategy for combining multiple predecessor outputs.
    #[serde(default)]
    pub strategy: MergeStrategy,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeStrategy {
    /// Concatenate string outputs (default).
    #[default]
    Concat,
    /// Pick the last non-empty predecessor output.
    LastWins,
    /// Collect into a JSON array, one entry per predecessor.
    Collect,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HumanNode {
    /// The question/prompt shown to the human approver.
    #[serde(default)]
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformNode {
    /// A small set of built-in transforms (avoids eval/arbitrary code).
    pub op: TransformOp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformOp {
    /// Extract a JSON field path (e.g. "output.summary").
    Extract(String),
    /// Prefix/suffix wrapping (for prompt assembly).
    Wrap { prefix: String, suffix: String },
    /// Trim to N chars.
    Truncate(usize),
}

/// Conditional routing. The executor evaluates `condition` against the node's
/// input value and returns the list of successor node ids to activate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchNode {
    /// Expression language is intentionally minimal: "key==value" or "contains:substr".
    pub condition: String,
}

/// An edge between two nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    #[serde(default)]
    pub kind: EdgeKind,
    /// For branch edges: the value that must match for this edge to fire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

// ---------------------------------------------------------------------------
// Executor — injected capability for Agent/Gate nodes
// ---------------------------------------------------------------------------

/// One chunk of a streaming agent run.
///
/// Agents (opaque CLI or transparent ReactAgent) emit incremental output as
/// they work. The executor maps the kernel-core `AgentEvent` stream onto this
/// two-variant chunk so the runner can forward `Delta`s to the frontend (as
/// `GraphEvent::NodeOutput`) and treat the single `Final` as the node's output
/// value (propagated to successors).
///
/// - `Delta` — incremental output (an agent token chunk). May be emitted many
///   times; the runner forwards each as a `NodeOutput` event but does NOT use
///   it as the node's logical output.
/// - `Final` — the terminal value. Emitted exactly once, last, and becomes the
///   node's output (what successors receive). For agents this is typically the
///   completed textual answer (opaque: session output_summary; transparent:
///   final assistant message).
#[derive(Debug, Clone)]
pub enum AgentChunk {
    Delta(Value),
    Final(Value),
}

/// The capability the host application provides to run capability-bearing
/// nodes. Keeping this as a trait (rather than the graph holding `Box<dyn
/// Agent>` directly) decouples kernel-compose from any concrete implementation
/// and from kernel-core's `Agent` trait object lifetime concerns.
#[async_trait::async_trait]
pub trait Executor: Send + Sync {
    /// Run an agent node as a STREAM of chunks. The stream must yield zero or
    /// more `Delta` chunks followed by exactly one `Final` (the node's output
    /// value); an `Err` item aborts the node. This is non-async because
    /// constructing the stream is synchronous (the underlying kernel-core
    /// `Agent::run` returns a stream directly); the caller drives the stream.
    fn run_agent(
        &self,
        spec: &AgentNodeSpec,
        input: Value,
        working_dir: Option<String>,
    ) -> Result<BoxStream<'static, Result<AgentChunk, String>>, String>;

    /// Run a quality gate. Returns a report as JSON.
    async fn run_gate(
        &self,
        gate: &GateNode,
        input: Value,
        working_dir: Option<String>,
    ) -> Result<Value, String>;
}

// ---------------------------------------------------------------------------
// Graph + Builder + CompiledGraph
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: HashMap<NodeId, Node>,
    pub edges: Vec<Edge>,
    pub start: NodeId,
    pub end: NodeId,
}

impl Graph {
    pub fn compile(self) -> Result<CompiledGraph, String> {
        CompiledGraph::new(self)
    }
}

/// A validated, ready-to-run graph.
#[derive(Debug, Clone)]
pub struct CompiledGraph {
    pub graph: Graph,
}

impl CompiledGraph {
    pub fn new(g: Graph) -> Result<Self, String> {
        Self::validate(&g)?;
        Ok(Self { graph: g })
    }

    fn validate(g: &Graph) -> Result<(), String> {
        if !g.nodes.contains_key(&g.start) {
            return Err(format!("start node '{}' not in nodes", g.start));
        }
        if !g.nodes.contains_key(&g.end) {
            return Err(format!("end node '{}' not in nodes", g.end));
        }
        for e in &g.edges {
            if !g.nodes.contains_key(&e.from) {
                return Err(format!("edge from unknown node '{}'", e.from));
            }
            if !g.nodes.contains_key(&e.to) {
                return Err(format!("edge to unknown node '{}'", e.to));
            }
        }
        Self::check_no_cycle(g)?;
        Ok(())
    }

    /// Kahn's algorithm — reject cycles. Branch edges are excluded from the
    /// cycle check only when they create a back-edge to a branch source
    /// (conditional, may legitimately re-route); for the small graphs we run,
    /// any cycle is treated as an error.
    fn check_no_cycle(g: &Graph) -> Result<(), String> {
        let mut indeg: HashMap<&NodeId, usize> = g.nodes.keys().map(|n| (n, 0usize)).collect();
        let mut adj: HashMap<&NodeId, Vec<&NodeId>> = HashMap::new();
        for e in &g.edges {
            adj.entry(&e.from).or_default().push(&e.to);
            *indeg.get_mut(&e.to).unwrap() += 1;
        }
        let mut queue: Vec<&NodeId> = indeg.iter().filter(|(_, &d)| d == 0).map(|(n, _)| *n).collect();
        let mut visited = 0usize;
        while let Some(n) = queue.pop() {
            visited += 1;
            if let Some(succs) = adj.get(n) {
                for s in succs {
                    let d = indeg.get_mut(s).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        queue.push(s);
                    }
                }
            }
        }
        if visited != g.nodes.len() {
            return Err(format!(
                "graph contains a cycle (visited {} of {} nodes)",
                visited,
                g.nodes.len()
            ));
        }
        Ok(())
    }

    /// Successors of a node (in edge order).
    pub fn successors(&self, id: &str) -> Vec<&Edge> {
        self.graph.edges.iter().filter(|e| e.from == id).collect()
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder DSL for constructing a graph in code.
#[derive(Debug, Default)]
pub struct GraphBuilder {
    nodes: HashMap<NodeId, Node>,
    edges: Vec<Edge>,
    start: Option<NodeId>,
    end: Option<NodeId>,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn node(mut self, id: impl Into<String>, node: Node) -> Self {
        self.nodes.insert(id.into(), node);
        self
    }

    pub fn edge(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.edges.push(Edge {
            from: from.into(),
            to: to.into(),
            kind: EdgeKind::Normal,
            when: None,
        });
        self
    }

    /// Branch edge: fires only when `when_value` matches the branch node's
    /// evaluated result.
    pub fn branch_edge(
        mut self,
        from: impl Into<String>,
        to: impl Into<String>,
        when_value: impl Into<String>,
    ) -> Self {
        self.edges.push(Edge {
            from: from.into(),
            to: to.into(),
            kind: EdgeKind::Branch,
            when: Some(when_value.into()),
        });
        self
    }

    pub fn start(mut self, id: impl Into<String>) -> Self {
        self.start = Some(id.into());
        self
    }

    pub fn end(mut self, id: impl Into<String>) -> Self {
        self.end = Some(id.into());
        self
    }

    pub fn build(self) -> Result<Graph, String> {
        let start = self.start.ok_or("start node not set")?;
        let end = self.end.ok_or("end node not set")?;
        Ok(Graph {
            nodes: self.nodes,
            edges: self.edges,
            start,
            end,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linear_graph() -> Graph {
        GraphBuilder::new()
            .node("p", Node::Prompt(PromptNode { text: "hi".into(), vars: HashMap::new() }))
            .node("e", Node::Merge(MergeNode::default()))
            .edge("p", "e")
            .start("p")
            .end("e")
            .build()
            .unwrap()
    }

    #[test]
    fn valid_linear_graph_compiles() {
        let g = linear_graph();
        assert!(g.compile().is_ok());
    }

    #[test]
    fn missing_start_node_is_rejected() {
        let g = Graph {
            nodes: HashMap::from([("a".to_string(), Node::Merge(MergeNode::default()))]),
            edges: vec![],
            start: "missing".into(),
            end: "a".into(),
        };
        let err = CompiledGraph::new(g).unwrap_err();
        assert!(err.contains("start node"));
    }

    #[test]
    fn cycle_is_rejected() {
        // a -> b -> a  (cycle), plus an end node to keep start/end valid
        let mk = || Graph {
            nodes: HashMap::from([
                ("a".to_string(), Node::Merge(MergeNode::default())),
                ("b".to_string(), Node::Merge(MergeNode::default())),
            ]),
            edges: vec![
                Edge { from: "a".into(), to: "b".into(), kind: EdgeKind::Normal, when: None },
                Edge { from: "b".into(), to: "a".into(), kind: EdgeKind::Normal, when: None },
            ],
            start: "a".into(),
            end: "b".into(),
        };
        let err = CompiledGraph::new(mk()).unwrap_err();
        assert!(err.contains("cycle"), "expected cycle error, got: {err}");
    }
}
