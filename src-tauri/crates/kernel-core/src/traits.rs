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

// ---------------------------------------------------------------------------
// C2 per-dispatch cost tally (kernel-core, DB-agnostic)
// ---------------------------------------------------------------------------

/// A point-in-time read of a [`CostAccumulator`]. Plain `Copy` data so the
/// SubAgentTool can snapshot a dispatch's totals after the child run without
/// holding the lock. All-zero (the default) means the child made no tracked LLM
/// calls — the cost line is suppressed.
#[derive(Debug, Clone, Copy, Default)]
pub struct CostTally {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
}

/// A thread-safe per-dispatch cost tally. Lives in kernel-core (no DB or pricing
/// dependency) so [`ChatModel::fork_with_counting_cost`] can return it across
/// the trait seam: the production model wraps its DB cost sink in a counter that
/// increments this accumulator AND forwards to the DB (attribution preserved),
/// then the SubAgentTool reads [`CostAccumulator::tally`] to label that one
/// dispatch's token + cost usage on the multi-agent board (C2 — the
/// anti-"10× cost" visibility the design requires, now that B3/B5 make cost
/// computable).
#[derive(Debug, Default)]
pub struct CostAccumulator {
    inner: std::sync::Mutex<CostTally>,
}

impl CostAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one LLM call's usage + cost to this dispatch's running total. Called
    /// by the counting cost sink wrapper on every record.
    pub fn add(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
        cost_usd: f64,
    ) {
        let mut t = self.inner.lock().expect("CostAccumulator mutex poisoned");
        t.input_tokens += input_tokens;
        t.output_tokens += output_tokens;
        t.cache_read_tokens += cache_read_tokens;
        t.cache_write_tokens += cache_write_tokens;
        t.cost_usd += cost_usd;
    }

    /// Snapshot the accumulated totals. Zero across the board when no tracked
    /// call landed (the SubAgentTool uses that to suppress an empty cost line).
    pub fn tally(&self) -> CostTally {
        *self.inner.lock().expect("CostAccumulator mutex poisoned")
    }
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

    /// Fork this model so a dispatched sub-agent's LLM cost is tallied
    /// separately and can be attributed to that one dispatch (C2 per-dispatch
    /// cost visibility on the multi-agent board). Returns `None` by default —
    /// every test/ad-hoc model opts out, so the SubAgentTool falls back to the
    /// shared parent model and runs cost-blind (unchanged behavior). Only the
    /// production GlmChatModel overrides this: it clones itself with a counting
    /// cost sink wrapping the parent's DB sink (DB attribution preserved), and
    /// returns the forked model + the accumulator the caller reads after the
    /// child run.
    fn fork_with_counting_cost(
        &self,
    ) -> Option<(std::sync::Arc<dyn ChatModel>, std::sync::Arc<CostAccumulator>)> {
        None
    }

    /// The concrete model id this instance sends in the request body (e.g.
    /// `"glm-5.2"`). ReactAgent's per-step router uses it as the base model
    /// when the caller didn't pass one in `AgentInput.model`, so the router
    /// decides against the model the user ACTUALLY picked (after provider
    /// `model_mapping` resolution) instead of a hardcoded flagship. Default
    /// `""` → the router keeps its legacy fallback (test stubs that don't
    /// model a concrete id are unaffected). Production models override.
    fn model_id(&self) -> &str {
        ""
    }
}

// ---------------------------------------------------------------------------
// EmbedModel (I1 vector memory fallback)
// ---------------------------------------------------------------------------

/// A text-embedding model used as a FALLBACK when FTS bm25 confidence is too
/// low (`retrieve_relevant_with_vector`). Keyword search misses synonyms and
/// rephrasings; embedding the query and cosine-ranking stored document vectors
/// recovers semantically-related memories the lexicon gap hid.
///
/// Only OpenAI-compatible providers implement this (POST `{base}/embeddings`).
/// Anthropic exposes no embeddings API, so an Anthropic-resolved session simply
/// gets `None` for its embedder and `retrieve_relevant_with_vector` degrades to
/// the FTS-only [`ChatModel`] path — I1 is opt-in per protocol, never blocking.
#[async_trait]
pub trait EmbedModel: Send + Sync {
    /// Embed a batch of texts. Returns one vector per input, in order. The SAME
    /// model id must embed queries AND documents, or the vectors are not
    /// comparable; callers thread [`EmbedModel::embed_model_id`] into storage so
    /// a model change invalidates stale rows.
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, Error>;

    /// The embedding model id sent in the request body. Stored alongside each
    /// vector so a model swap can be detected (dim mismatch / re-embed). Default
    /// `""` for test stubs that don't model a concrete id.
    fn embed_model_id(&self) -> &str {
        ""
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
