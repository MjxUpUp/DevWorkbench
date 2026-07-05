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

    #[error("graph error: {0}")]
    Graph(String),

    #[error("agent error: {0}")]
    Agent(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("cancelled")]
    Cancelled,

    /// Upstream closed the SSE byte stream without ever sending `message_stop`.
    /// This is the truncation signature — e.g. Ollama hitting its `num_predict`
    /// cap mid-thinking, or a proxy dropping the connection. Distinct from a
    /// clean stream end: a clean end ALWAYS terminates with `message_stop`. The
    /// run loop decides retry-vs-degrade based on whether any content was
    /// already emitted to the UI. `got_reason` records whether a `message_delta`
    /// carrying `stop_reason` arrived before the cut — purely diagnostic (a
    /// clean turn has BOTH, so the field's absence is informational, not a
    /// second gate).
    #[error("stream incomplete: stop_reason={got_reason} (upstream closed the SSE stream without a terminal `message_stop` — likely truncated)")]
    StreamIncomplete { got_reason: bool },

    /// No bytes received from the upstream for `secs` seconds while a stream is
    /// nominally open. Covers the "thinking-forever / network stall" failure
    /// mode that a plain HTTP read timeout misses (reqwest's timeout gates the
    /// initial response, not an idle streaming body). Treated as retryable — a
    /// stall is transient the same way a network blip is.
    #[error("stream idle timeout: no data for {secs}s (upstream stalled mid-stream)")]
    StreamIdle { secs: u64 },

    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error("{0}")]
    Other(String),
}

/// Allow `?` from String-returning APIs (compose migration).
impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Other(s)
    }
}

/// Allow `?` from `ok_or("literal")` patterns.
impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Error::Other(s.into())
    }
}
