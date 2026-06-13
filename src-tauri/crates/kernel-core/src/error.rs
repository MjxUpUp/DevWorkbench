//! Error type for the kernel-core layer.
//!
//! Kept intentionally broad: kernel-core has no dependencies that produce
//! concrete errors (no DB, no HTTP), so this enum captures capability-level
//! failure modes. Implementations map their provider/DB errors into these.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("network error: {0}")]
    Network(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("model error: {0}")]
    Model(String),

    #[error("tool error: {0}")]
    Tool(String),

    #[error("retrieval error: {0}")]
    Retrieval(String),

    #[error("template error: {0}")]
    Template(String),

    #[error("graph error: {0}")]
    Graph(String),

    #[error("agent error: {0}")]
    Agent(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("cancelled")]
    Cancelled,

    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error("{0}")]
    Other(String),
}
