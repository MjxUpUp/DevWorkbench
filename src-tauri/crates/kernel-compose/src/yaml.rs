//! YAML workflow definition → Graph.
//!
//! Frontend DAG editor exports a workflow as YAML; this module parses it into a
//! [`crate::Graph`] that can be compiled and run. Schema is intentionally simple
//! to keep the frontend editor trivial.
//!
//! Example:
//! ```yaml
//! start: prompt_1
//! end: gate_1
//! nodes:
//!   prompt_1:
//!     type: prompt
//!     text: "refactor auth module"
//!   agent_1:
//!     type: agent
//!     agent: claude_code
//!     model: sonnet
//!   gate_1:
//!     type: gate
//!     gate: forge
//! edges:
//!   - { from: prompt_1, to: agent_1 }
//!   - { from: agent_1, to: gate_1 }
//! ```

use serde::{Deserialize, Serialize};

use crate::graph::{Edge, Graph, Node};
use crate::events::EdgeKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDef {
    pub start: String,
    pub end: String,
    #[serde(default)]
    pub nodes: std::collections::HashMap<String, Node>,
    #[serde(default)]
    pub edges: Vec<WorkflowEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEdge {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub kind: EdgeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

impl WorkflowDef {
    /// Parse YAML text into a workflow definition.
    pub fn from_yaml(yaml: &str) -> Result<Self, String> {
        serde_yaml::from_str::<WorkflowDef>(yaml).map_err(|e| format!("YAML parse error: {e}"))
    }

    /// Convert into a runnable [`Graph`].
    pub fn to_graph(self) -> Result<Graph, String> {
        if !self.nodes.contains_key(&self.start) {
            return Err(format!("start node '{}' missing from nodes", self.start));
        }
        if !self.nodes.contains_key(&self.end) {
            return Err(format!("end node '{}' missing from nodes", self.end));
        }
        let edges: Vec<Edge> = self
            .edges
            .into_iter()
            .map(|we| Edge {
                from: we.from,
                to: we.to,
                kind: we.kind,
                when: we.when,
            })
            .collect();
        Ok(Graph {
            nodes: self.nodes,
            edges,
            start: self.start,
            end: self.end,
        })
    }

    /// Parse YAML and compile in one step.
    pub fn parse_and_compile(yaml: &str) -> Result<crate::graph::CompiledGraph, String> {
        let def = Self::from_yaml(yaml)?;
        let graph = def.to_graph()?;
        graph.compile()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_YAML: &str = r#"
start: prompt_1
end: gate_1
nodes:
  prompt_1:
    type: prompt
    text: "refactor auth"
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

    #[test]
    fn parses_and_compiles_three_node_workflow() {
        let compiled = WorkflowDef::parse_and_compile(SAMPLE_YAML).unwrap();
        assert_eq!(compiled.graph.start, "prompt_1");
        assert_eq!(compiled.graph.end, "gate_1");
        assert_eq!(compiled.graph.edges.len(), 2);
    }

    #[test]
    fn missing_start_node_rejected() {
        let bad = r#"
start: ghost
end: e
nodes:
  e: { type: merge }
"#;
        let err = WorkflowDef::from_yaml(bad).unwrap().to_graph().unwrap_err();
        assert!(err.contains("start node"));
    }
}
