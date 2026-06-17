//! Component traits — the eino `components/` layer.
//!
//! Each trait is a single capability a kernel component can implement:
//! - [`ChatModel`]: call an LLM (blocking + streaming)
//! - [`Tool`]: a callable tool the agent can invoke
//! - [`Retriever`]: fetch relevant documents (RAG)
//!
//! All traits are `async` via `async_trait` and return `BoxStream` for streaming,
//! mirroring eino's `StreamReader`-based design but in idiomatic Rust.

use async_trait::async_trait;
use futures::stream::BoxStream;

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
    /// Enable extended/interleaved thinking (Anthropic Messages protocol).
    /// When set, the request carries `thinking: {type:"enabled", budget_tokens}`
    /// and the model's thinking blocks are parsed into `Message.reasoning` +
    /// surfaced as `AgentEvent::Reasoning`, then preserved across turns. None
    /// = thinking off (the default; preserves current behavior).
    pub thinking: Option<ThinkingConfig>,
}

/// Extended-thinking budget — Anthropic's `thinking.budget_tokens`. The model
/// may spend up to this many tokens reasoning before its visible answer. The
/// ChatModel raises `max_tokens` above this when thinking is enabled (Anthropic
/// requires `max_tokens > budget_tokens`).
#[derive(Debug, Clone, Copy)]
pub struct ThinkingConfig {
    pub budget_tokens: u32,
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

    /// Return a model with tool definitions bound. Default returns Unsupported
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

// Note: ChatTemplate trait removed — it had zero implementors and zero callers
// (YAGNI). If prompt templating is needed, add it back when a real consumer
// exists, carrying the concrete templating syntax required then.
