//! Trait implementations bridging kernel-core/kernel-compose to the existing
//! DevWorkbench subsystems (pty agents, knowledge store, quality gates).
//!
//! This module is where the abstract traits get concrete behavior:
//! - [`executor::KernelExecutor`] implements `kernel_compose::Executor`, routing
//!   graph Agent nodes to `spawn_pty_agent` and Gate nodes to Forge/quality.
//! - [`retriever::KnowledgeRetriever`] implements `kernel_core::Retriever` over
//!   the existing FTS5 knowledge store.
//! - [`honesty::HonestyVerifier`] is the anti-self-deception layer.
//! - [`react_agent`] the transparent ReactAgent + GLM ChatModel.
//! - [`unified_context`] the cross-CLI memory projection.

pub mod executor;
pub mod honesty;
pub mod react_agent;
pub mod retriever;
pub mod unified_context;
