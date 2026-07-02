//! DevWorkbench kernel-compose — the graph/chain orchestration engine.
//!
//! Rust-native adaptation of eino's `compose/` layer. Key design choices that
//! DIVERGE from eino (deliberately — see the kernel design doc):
//!
//! - **Currency type is `serde_json::Value`**, not Go generics over `I`/`O`.
//!   eino's reflect-based type erasure is an anti-pattern in Rust; we lean on
//!   runtime schema validation at YAML load + node-input contracts instead.
//! - **Execution is topological (DAG all-predecessor)**, not Pregel supersteps.
//!   Coding-agent workflows are small (2–5 nodes), never need BSP cycles.
//! - **Streaming is push-based** via an `async_stream` yielding `GraphEvent`s
//!   (node start/end, errors, final output) — the frontend renders the canvas
//!   node-by-node as this stream arrives.
//!
//! ## The 7 node types (from the v1.0 product spec)
//!
//! `Prompt` · `Agent` · `Gate` · `Parallel` · `Merge` · `Human` · `Transform`,
//! plus `Branch` edges for conditional routing.

pub mod events;
pub mod gates;
pub mod graph;
pub mod runner;
pub mod yaml;

pub use events::{EdgeKind, GraphEvent, NodeStatus};
pub use graph::{
    BranchNode, CompiledGraph, Edge, GateNode, Graph, GraphBuilder, HumanNode, InterruptNode,
    LoopNode, MergeNode, Node, NodeId, NodeType, ParallelNode, PromptNode, SelectorCase,
    SelectorNode, TransformNode,
};
pub use runner::{run_graph, run_graph_with_approvals, HumanApproval};
pub use yaml::WorkflowDef;
