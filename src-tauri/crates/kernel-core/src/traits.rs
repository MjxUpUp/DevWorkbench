//! Component traits — the eino `components/` layer.
//!
//! Each trait is a single capability a kernel component can implement:
//! - [`ChatModel`]: call an LLM (blocking + streaming)
//! - [`Tool`]: a callable tool the agent can invoke
//! - [`Retriever`]: fetch relevant documents (RAG)
//! - [`ChatTemplate`]: render variables into a message list
//!
//! All traits are `async` via `async_trait` and return `BoxStream` for streaming,
//! mirroring eino's `StreamReader`-based design but in idiomatic Rust.

use std::collections::HashMap;

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::document::Document;
use crate::schema::Message;
use crate::Error;

/// A streaming chat response: a sequence of message deltas (text or tool-call
/// fragments) terminated by `Ok(Ok(..))` end-of-stream or an `Err`.
pub type MessageStream = BoxStream<'static, Result<Message, Error>>;

// ---------------------------------------------------------------------------
// ChatModel
// ---------------------------------------------------------------------------

/// Options passed to a ChatModel call. All fields optional so callers only set
/// what they need (eino's `Option` pattern, Rust-ified as a builder-less struct).
#[derive(Debug, Clone, Default)]
pub struct ModelOptions {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub max_tokens: Option<u32>,
    pub model: Option<String>,
    pub stop: Option<Vec<String>>,
}

/// Tool declaration given to the model so it knows what it can call.
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the tool's arguments (may be the trivial
    /// `{"type":"object","properties":{}}` for no-arg tools).
    pub parameters_schema: serde_json::Value,
}

/// Context passed to a tool invocation — the execution environment, not the
/// tool's domain arguments.
#[derive(Debug, Clone, Default)]
pub struct ToolContext {
    /// Working directory the tool should operate in (e.g. the project root).
    pub working_dir: Option<String>,
    /// Conversation/session id, for tools that log or persist.
    pub conversation_id: Option<String>,
}

/// An LLM chat model. `generate` is the blocking path; `stream` yields tokens.
///
/// `with_tools` returns a NEW model instance bound to the given tools (immutable,
/// matching eino's preferred `ToolCallingChatModel` — the older in-place
/// `BindTools` is deliberately not provided).
#[async_trait]
pub trait ChatModel: Send + Sync {
    async fn generate(
        &self,
        messages: &[Message],
        opts: &ModelOptions,
    ) -> Result<Message, Error>;

    fn stream(
        &self,
        messages: &[Message],
        opts: &ModelOptions,
    ) -> Result<MessageStream, Error>;

    /// Return a model with tool definitions bound. Default panics by intent —
    /// a model that cannot call tools must override to error clearly.
    fn with_tools(&self, _tools: &[ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
        Err(Error::Unsupported(
            "this ChatModel does not support tool calling".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Tool
// ---------------------------------------------------------------------------

/// A tool an agent can invoke. `info` declares it to the model; `invoke`
/// executes it. `is_dangerous` flags destructive tools so the kernel can gate
/// them (mirrors LocalAgent's safety tier, needed by the honesty/quality layer).
#[async_trait]
pub trait Tool: Send + Sync {
    fn info(&self) -> ToolInfo;

    /// JSON-encoded arguments string (the model produces this; we keep it as a
    /// string to match streaming tool-call semantics). Returns the tool's
    /// textual result, fed back as a tool message.
    async fn invoke(
        &self,
        arguments: &str,
        ctx: &ToolContext,
    ) -> Result<String, Error>;

    /// Destructive tools (write/delete/exec) — the kernel surfaces these
    /// differently and may require confirmation.
    fn is_dangerous(&self) -> bool {
        false
    }

    /// Read-only tools (search/read) can run without risk in any context.
    fn is_read_only(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Retriever (RAG)
// ---------------------------------------------------------------------------

/// Retrieval options. `top_k` caps result count; `score_threshold` filters.
#[derive(Debug, Clone, Default)]
pub struct RetrieveOptions {
    pub top_k: Option<usize>,
    pub score_threshold: Option<f64>,
    /// Scope to a project / namespace; None = cross-project.
    pub scope: Option<String>,
}

#[async_trait]
pub trait Retriever: Send + Sync {
    async fn retrieve(
        &self,
        query: &str,
        opts: &RetrieveOptions,
    ) -> Result<Vec<Document>, Error>;
}

// ---------------------------------------------------------------------------
// ChatTemplate
// ---------------------------------------------------------------------------

/// Renders a variables map into the message list a ChatModel consumes.
/// Mirrors eino's `ChatTemplate.Format`. Missing required keys ⇒ Error.
#[async_trait]
pub trait ChatTemplate: Send + Sync {
    async fn format(
        &self,
        vars: &HashMap<String, serde_json::Value>,
    ) -> Result<Vec<Message>, Error>;
}
