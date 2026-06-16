//! Graph execution events — the stream a compiled graph emits as it runs.
//!
//! The frontend Orchestrate canvas subscribes to these to light up nodes
//! one-by-one (idle gray → running blue pulse → done green / failed red).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::graph::NodeId;

/// Runtime status of a single node during execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    /// Not yet reached.
    Pending,
    /// Currently executing.
    Running,
    /// Completed successfully.
    Done,
    /// Failed (carries error in the GraphEvent).
    Failed,
    /// Skipped because a branch didn't select it.
    Skipped,
    /// Paused waiting for human approval (Human node).
    WaitingApproval,
}

/// Edge semantic. Normal carries data; Branch is conditional (evaluated by
/// the source node's predicate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Data flows along this edge unconditionally.
    #[default]
    Normal,
    /// Source is a branch node; edge fires only if its condition matches.
    Branch,
}

/// One observation during a graph run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GraphEvent {
    /// A node started executing.
    NodeStart { node: NodeId },
    /// A node finished (ok or not). `error` is set when status == Failed.
    NodeEnd {
        node: NodeId,
        status: NodeStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// A Human node paused, awaiting approval. `resume_token` is what the
    /// caller presents back via [`crate::runner::run_graph`] approval flow.
    ApprovalRequired {
        node: NodeId,
        prompt: String,
        resume_token: String,
    },
    /// Incremental output produced by a node (agent token stream forwarded).
    NodeOutput { node: NodeId, chunk: Value },
    /// The whole graph finished; carries the final output value.
    GraphDone { output: Value },
    /// The whole graph failed.
    GraphFailed { error: String },
}

impl GraphEvent {
    pub fn node_start(node: impl Into<String>) -> Self {
        Self::NodeStart { node: node.into() }
    }
    pub fn node_done(node: impl Into<String>) -> Self {
        Self::NodeEnd {
            node: node.into(),
            status: NodeStatus::Done,
            error: None,
        }
    }
    pub fn node_failed(node: impl Into<String>, err: impl Into<String>) -> Self {
        Self::NodeEnd {
            node: node.into(),
            status: NodeStatus::Failed,
            error: Some(err.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_status_serializes_snake_case() {
        let s = serde_json::to_string(&NodeStatus::WaitingApproval).unwrap();
        assert_eq!(s, "\"waiting_approval\"");
    }

    #[test]
    fn graph_event_tag_discriminator_present() {
        let e = GraphEvent::node_start("agent_1");
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(v["kind"], "node_start");
        assert_eq!(v["node"], "agent_1");
    }
}
