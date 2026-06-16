//! Transparent ReactAgent + GLM ChatModel + ToolRegistry.
//!
//! The "transparent" agent: the kernel controls the LLM call AND the tool loop
//! directly (eino `adk/react.go` Rust port). Used for kernel-internal tasks and
//! as a self-built agent that can call MCP tools and Skills.
//!
//! Three pieces:
//! - [`GlmChatModel`]: `ChatModel` impl calling Zhipu GLM via Anthropic API,
//!   with real SSE streaming and tool binding.
//! - [`ToolRegistry`]: a cloneable collection of `dyn Tool` (MCP + Skill + builtin).
//! - [`ReactAgent`]: reason->act->observe loop, bounded by max_steps, implements
//!   `kernel_core::Agent`. Binds tools to the model, dispatches hooks around
//!   tool calls, and streams AgentEvents.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use kernel_core::{
    AgentCaps, AgentEvent, AgentInput, AgentKind, AgentOutcome, AgentRunStatus,
    ChatModel, Error, Message, MessageStream, ModelOptions, Role, Tool, ToolContext, ToolInfo,
};
use serde_json::{json, Value};

use crate::cost::circuit_breaker::{should_failover, CircuitBreaker};
use crate::cost::sink::CostSink;
use crate::kernel_impl::hooks::HookManager;

// ---------------------------------------------------------------------------
// GlmChatModel
// ---------------------------------------------------------------------------

/// A ChatModel calling Zhipu GLM via its Anthropic-compatible Messages API.
#[derive(Clone)]
pub struct GlmChatModel {
    base_url: String,
    api_key: String,
    model: String,
    client: reqwest::Client,
    bound_tools: Vec<ToolInfo>,
    /// Shared upstream circuit breaker. When Some, every request is gated +
    /// its outcome recorded, so a failing GLM endpoint trips open instead of
    /// every agent turn hammering it. None = unprotected (tests / offline).
    circuit: Option<Arc<CircuitBreaker>>,
    /// Optional cost sink — records token usage + cost per completed request.
    /// None = untracked (tests / ad-hoc agents without a session).
    cost_sink: Option<Arc<dyn CostSink>>,
}

impl GlmChatModel {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            client: reqwest::Client::builder().timeout(std::time::Duration::from_secs(120)).build().unwrap_or_else(|_| reqwest::Client::new()),
            bound_tools: Vec::new(),
            circuit: None,
            cost_sink: None,
        }
    }

    pub fn bigmodel(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new("https://open.bigmodel.cn/api/anthropic", api_key, model)
    }

    /// Attach a shared circuit breaker. The same breaker should be shared
    /// across every GlmChatModel instance that targets the same upstream, so a
    /// trip in one agent is observed by all (see `shared_glm_circuit` below).
    pub fn with_circuit(mut self, circuit: Arc<CircuitBreaker>) -> Self {
        self.circuit = Some(circuit);
        self
    }

    /// Attach a cost sink that records token usage + cost per request.
    pub fn with_cost_sink(mut self, sink: Arc<dyn CostSink>) -> Self {
        self.cost_sink = Some(sink);
        self
    }

    fn build_body(&self, model: &str, messages: &[Message], opts: &ModelOptions, stream: bool) -> Value {
        let msgs: Vec<Value> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| match m.role {
                Role::Tool => {
                    // M5: Anthropic expects tool results as user-role messages with
                    // a tool_result content block (not assistant text).
                    json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": m.tool_call_id.as_deref().unwrap_or(""),
                            "content": m.content,
                        }],
                    })
                }
                Role::User => json!({ "role": "user", "content": m.content }),
                _ => {
                    // Assistant. When the prior turn carried reasoning, replay
                    // it as a leading `thinking` block so the model can build on
                    // it (Anthropic/GLM preserved thinking — the signature is
                    // required or the replayed block is rejected). Turns with no
                    // reasoning keep the original wire shape exactly.
                    match (
                        m.reasoning.as_ref().filter(|s| !s.is_empty()),
                        m.tool_calls.is_empty(),
                    ) {
                        (None, true) => json!({ "role": "assistant", "content": m.content }),
                        (None, false) => {
                            let mut content: Vec<Value> = vec![json!({"type":"text","text":m.content})];
                            for tc in &m.tool_calls {
                                let input: Value = serde_json::from_str(&tc.function.arguments)
                                    .unwrap_or(json!({}));
                                content.push(json!({
                                    "type": "tool_use",
                                    "id": tc.id,
                                    "name": tc.function.name,
                                    "input": input,
                                }));
                            }
                            json!({ "role": "assistant", "content": content })
                        }
                        (Some(thinking), _) => {
                            let mut content: Vec<Value> = Vec::new();
                            let mut block = json!({"type":"thinking","thinking": thinking});
                            if let Some(sig) =
                                m.reasoning_signature.as_ref().filter(|s| !s.is_empty())
                            {
                                block["signature"] = json!(sig);
                            }
                            content.push(block);
                            if !m.content.is_empty() {
                                content.push(json!({"type":"text","text": m.content}));
                            }
                            for tc in &m.tool_calls {
                                let input: Value = serde_json::from_str(&tc.function.arguments)
                                    .unwrap_or(json!({}));
                                content.push(json!({
                                    "type": "tool_use",
                                    "id": tc.id,
                                    "name": tc.function.name,
                                    "input": input,
                                }));
                            }
                            json!({ "role": "assistant", "content": content })
                        }
                    }
                }
            })
            .collect();
        let system: String = messages
            .iter()
            .filter(|m| m.role == Role::System)
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
            .join("\n");
        let mut max_tokens = opts.max_tokens.unwrap_or(4096);
        // Anthropic requires max_tokens > thinking.budget_tokens. Raise the
        // floor so a small caller-supplied max_tokens (or the 4096 default)
        // can't make the request 400 when thinking is on.
        if let Some(tc) = opts.thinking {
            if max_tokens <= tc.budget_tokens {
                max_tokens = tc.budget_tokens + 4096;
            }
        }
        let mut body = json!({
            "model": model,
            "messages": msgs,
            "max_tokens": max_tokens,
            "stream": stream,
        });
        if !system.is_empty() {
            body["system"] = Value::String(system);
        }
        if let Some(tc) = opts.thinking {
            body["thinking"] = json!({"type":"enabled","budget_tokens": tc.budget_tokens});
        }
        if let Some(t) = opts.temperature {
            body["temperature"] = json!(t);
        }
        if !self.bound_tools.is_empty() {
            let tools: Vec<Value> = self.bound_tools.iter().map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters_schema,
                })
            }).collect();
            body["tools"] = Value::Array(tools);
        }
        body
    }
}

#[async_trait]
impl ChatModel for GlmChatModel {
    async fn generate(&self, messages: &[Message], opts: &ModelOptions) -> Result<Message, Error> {
        let model = opts.model.clone().unwrap_or_else(|| self.model.clone());
        // Circuit breaker: gate the call and record the outcome.
        if let Some(cb) = &self.circuit {
            if !cb.allow_request(&self.base_url) {
                return Err(Error::Model(format!("upstream circuit open: {}", self.base_url)));
            }
            cb.on_attempt(&self.base_url);
        }
        let body = self.build_body(&model, messages, opts, false);
        let resp = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                if let Some(cb) = &self.circuit {
                    cb.record_failure(&self.base_url);
                }
                return Err(Error::Network(e.to_string()));
            }
        };
        let status = resp.status();
        if !status.is_success() {
            if should_failover(Some(status.as_u16()), false) {
                if let Some(cb) = &self.circuit {
                    cb.record_failure(&self.base_url);
                }
            }
            return Err(Error::Model(format!("GLM stream failed: {status}")));
        }
        let v: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                if let Some(cb) = &self.circuit {
                    cb.record_failure(&self.base_url);
                }
                return Err(Error::Model(format!("decode: {e}")));
            }
        };
        if let Some(cb) = &self.circuit {
            cb.record_success(&self.base_url);
        }
        // Cost: record token usage; cost is derived in the sink when 0.
        if let Some(sink) = &self.cost_sink {
            let (input, output) = usage_from_response(&v);
            sink.record(&model, input, output, 0.0);
        }
        decode_anthropic_message(&v)
    }

    fn stream(&self, messages: &[Message], opts: &ModelOptions) -> Result<MessageStream, Error> {
        let model_clone = self.clone();
        let messages = messages.to_vec();
        let opts = opts.clone();
        let s = async_stream::try_stream! {
            let model_name = opts.model.clone().unwrap_or_else(|| model_clone.model.clone());
            // Circuit breaker gate.
            if let Some(cb) = &model_clone.circuit {
                if !cb.allow_request(&model_clone.base_url) {
                    Err(Error::Model(format!("upstream circuit open: {}", model_clone.base_url)))?;
                }
                cb.on_attempt(&model_clone.base_url);
            }
            let body = model_clone.build_body(&model_name, &messages, &opts, true);
            let resp = model_clone.client
                .post(format!("{}/v1/messages", model_clone.base_url))
                .header("x-api-key", &model_clone.api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send()
                .await;
            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    if let Some(cb) = &model_clone.circuit { cb.record_failure(&model_clone.base_url); }
                    Err(Error::Network(e.to_string()))?
                }
            };
            let status = resp.status();
            if !status.is_success() {
                if should_failover(Some(status.as_u16()), false) {
                    if let Some(cb) = &model_clone.circuit { cb.record_failure(&model_clone.base_url); }
                }
                Err(Error::Model(format!("GLM stream failed: {status}")))?;
            }
            use futures::StreamExt;
            let mut byte_stream = resp.bytes_stream();
            let mut buf = String::new();
            // Accumulate tool_use blocks by Anthropic content_block index, then
            // reassemble into a terminal tool_calls Message on message_stop. Text
            // deltas are yielded inline for real token-by-token streaming. The
            // per-line decision lives in handle_sse_line (unit-testable, no HTTP).
            let mut tool_bufs: HashMap<u64, (String, String, String)> = HashMap::new();
            // Accumulates the thinking signature for THIS turn (signature_delta
            // chunks arrive out of band from the thinking_delta reasoning text).
            // Reset per request — one stream() call == one assistant turn.
            let mut sig_buf = String::new();
            // Accumulate token usage from message_start/message_delta so the
            // turn's cost is recorded when the stream completes.
            let mut usage_in: u32 = 0;
            let mut usage_out: u32 = 0;
            while let Some(chunk_res) = byte_stream.next().await {
                let bytes = match chunk_res {
                    Ok(b) => b,
                    Err(e) => {
                        if let Some(cb) = &model_clone.circuit { cb.record_failure(&model_clone.base_url); }
                        Err(Error::Network(e.to_string()))?
                    }
                };
                buf.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(nl) = buf.find('\n') {
                    let line = buf[..nl].trim().to_string();
                    buf.drain(..=nl);
                    if let Some((i, o)) = parse_usage(&line) {
                        usage_in = usage_in.saturating_add(i);
                        usage_out = usage_out.saturating_add(o);
                    }
                    if let Some(msg) = handle_sse_line(&line, &mut tool_bufs, &mut sig_buf) {
                        yield msg;
                    }
                }
            }
            // Stream consumed cleanly → upstream healthy + record the turn's cost.
            if let Some(cb) = &model_clone.circuit { cb.record_success(&model_clone.base_url); }
            if let Some(sink) = &model_clone.cost_sink {
                sink.record(&model_name, usage_in, usage_out, 0.0);
            }
        };
        Ok(Box::pin(s))
    }

    fn with_tools(&self, tools: &[ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
        let mut clone = self.clone();
        clone.bound_tools = tools.to_vec();
        Ok(Box::new(clone))
    }
}

/// Process-wide shared circuit breaker for GLM (Anthropic-compatible)
/// endpoints. Every ReactAgent built via `build_react_agent` taps the same
/// breaker so a sustained upstream outage trips the circuit for all sessions
/// at once, rather than each session rediscovering the failure and flooding a
/// down endpoint. State is keyed by base_url inside the breaker, so distinct
/// endpoints coexist under one instance. Lazily initialized on first use.
pub fn shared_glm_circuit() -> Arc<CircuitBreaker> {
    static CIRCUIT: std::sync::OnceLock<Arc<CircuitBreaker>> = std::sync::OnceLock::new();
    CIRCUIT
        .get_or_init(|| {
            Arc::new(CircuitBreaker::new(
                crate::cost::circuit_breaker::CircuitBreakerConfig::default(),
            ))
        })
        .clone()
}

/// Extract token usage from an Anthropic SSE line. `message_start` carries
/// `usage.input_tokens`; `message_delta` carries the cumulative
/// `usage.output_tokens`. Non-usage lines (and non-`data:` lines) return None.
/// Used to meter cost on the streaming path.
fn parse_usage(line: &str) -> Option<(u32, u32)> {
    let data = line.trim().strip_prefix("data: ")?;
    let ev: Value = serde_json::from_str(data).ok()?;
    match ev.get("type").and_then(|t| t.as_str())? {
        "message_start" => {
            let input = ev
                .get("message")
                .and_then(|m| m.get("usage"))
                .and_then(|u| u.get("input_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            Some((input, 0))
        }
        "message_delta" => {
            let output = ev
                .get("usage")
                .and_then(|u| u.get("output_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            Some((0, output))
        }
        _ => None,
    }
}

/// Extract usage from a non-streaming Anthropic response
/// (`usage.input_tokens` / `usage.output_tokens`). Returns (0, 0) if absent —
/// the sink still records the call with a derived/zero cost.
fn usage_from_response(v: &Value) -> (u32, u32) {
    let u = match v.get("usage") {
        Some(u) => u,
        None => return (0, 0),
    };
    let input = u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let output = u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    (input, output)
}

fn decode_anthropic_message(v: &Value) -> Result<Message, Error> {
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut reasoning_parts = Vec::new();
    let mut signature_parts = Vec::new();
    if let Some(arr) = v.get("content").and_then(|c| c.as_array()) {
        for block in arr {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        text_parts.push(t.to_string());
                    }
                }
                Some("tool_use") => {
                    let id = block.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                    let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                    let args = block.get("input").map(|i| i.to_string()).unwrap_or_else(|| "{}".to_string());
                    tool_calls.push(kernel_core::ToolCall {
                        id,
                        call_type: "function".into(),
                        function: kernel_core::FunctionCall { name, arguments: args },
                    });
                }
                Some("thinking") => {
                    // GLM Interleaved Thinking content block. Capture the trace
                    // + its signature (needed to preserve the block next turn).
                    if let Some(t) = block.get("thinking").and_then(|t| t.as_str()) {
                        reasoning_parts.push(t.to_string());
                    }
                    if let Some(s) = block.get("signature").and_then(|s| s.as_str()) {
                        signature_parts.push(s.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    Ok(Message {
        role: Role::Assistant,
        content: text_parts.join(""),
        tool_calls,
        tool_call_id: None,
        reasoning: if reasoning_parts.is_empty() {
            None
        } else {
            Some(reasoning_parts.join(""))
        },
        reasoning_signature: if signature_parts.is_empty() {
            None
        } else {
            Some(signature_parts.join(""))
        },
    })
}

/// Parse one SSE `data: <json>` line from an Anthropic Messages stream, mutate
/// the tool_use accumulator, and return any Message to yield. Returns None for
/// non-data lines, malformed JSON, and event types that carry no Message (ping,
/// message_start, content_block_stop). Text deltas become assistant Messages
/// immediately (real streaming); tool_use blocks accumulate and reassemble into
/// a terminal tool_calls Message on message_stop. Extracted from stream() so the
/// tool_use accumulation is unit-testable without HTTP.
fn handle_sse_line(
    line: &str,
    tool_bufs: &mut HashMap<u64, (String, String, String)>,
    sig_buf: &mut String,
) -> Option<Message> {
    let data = line.trim().strip_prefix("data: ")?;
    let ev: Value = serde_json::from_str(data).ok()?;
    match ev.get("type").and_then(|t| t.as_str())? {
        "content_block_start" => {
            if let Some(block) = ev.get("content_block") {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    let idx = ev.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                    let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    tool_bufs.insert(idx, (id, name, String::new()));
                }
            }
            None
        }
        "content_block_delta" => {
            let dt = ev.get("delta").and_then(|d| d.get("type")).and_then(|t| t.as_str());
            if dt == Some("text_delta") {
                ev.get("delta")
                    .and_then(|d| d.get("text"))
                    .and_then(|t| t.as_str())
                    .map(|t| Message::assistant(t.to_string()))
            } else if dt == Some("input_json_delta") {
                let idx = ev.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                if let Some(partial) = ev
                    .get("delta")
                    .and_then(|d| d.get("partial_json"))
                    .and_then(|p| p.as_str())
                {
                    if let Some(slot) = tool_bufs.get_mut(&idx) {
                        slot.2.push_str(partial);
                    }
                }
                None
            } else if dt == Some("thinking_delta") {
                // Stream the reasoning trace chunk-by-chunk so chat renders it
                // live. The caller reassembles the full reasoning from these
                // chunks; the signature arrives as a separate signature_delta
                // and is emitted once, on message_stop, via sig_buf.
                ev.get("delta")
                    .and_then(|d| d.get("thinking"))
                    .and_then(|t| t.as_str())
                    .map(|t| Message {
                        role: Role::Assistant,
                        content: String::new(),
                        tool_calls: Vec::new(),
                        tool_call_id: None,
                        reasoning: Some(t.to_string()),
                        reasoning_signature: None,
                    })
            } else if dt == Some("signature_delta") {
                if let Some(s) = ev
                    .get("delta")
                    .and_then(|d| d.get("signature"))
                    .and_then(|t| t.as_str())
                {
                    sig_buf.push_str(s);
                }
                None
            } else {
                None
            }
        }
        "message_stop" => {
            // Carry the turn's accumulated thinking signature out on the
            // terminal message — even with no tool calls, so a pure
            // reasoning+answer turn still preserves its signature for the next.
            let sig = if sig_buf.is_empty() {
                None
            } else {
                Some(sig_buf.clone())
            };
            if tool_bufs.is_empty() {
                return sig.map(|s| Message {
                    role: Role::Assistant,
                    content: String::new(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    reasoning: None,
                    reasoning_signature: Some(s),
                });
            }
            let mut idxs: Vec<u64> = tool_bufs.keys().copied().collect();
            idxs.sort();
            let tool_calls: Vec<kernel_core::ToolCall> = idxs
                .into_iter()
                .filter_map(|idx| tool_bufs.remove(&idx))
                .map(|(id, name, args)| kernel_core::ToolCall {
                    id,
                    call_type: "function".into(),
                    function: kernel_core::FunctionCall {
                        name,
                        arguments: if args.is_empty() { "{}".to_string() } else { args },
                    },
                })
                .collect();
            Some(Message {
                role: Role::Assistant,
                content: String::new(),
                tool_calls,
                tool_call_id: None,
                reasoning: None,
                reasoning_signature: sig,
            })
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// ToolRegistry
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, tool: impl Tool + 'static) -> Self {
        self.tools.push(Arc::new(tool));
        self
    }

    pub fn push(&mut self, tool: impl Tool + 'static) {
        self.tools.push(Arc::new(tool));
    }

    pub fn push_arc(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn infos(&self) -> Vec<ToolInfo> {
        self.tools.iter().map(|t| t.info()).collect()
    }

    pub fn find(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.info().name == name).cloned()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

// ---------------------------------------------------------------------------
// ReactAgent
// ---------------------------------------------------------------------------

pub struct ReactAgent {
    model: Arc<dyn ChatModel>,
    tools: ToolRegistry,
    hooks: Option<Arc<HookManager>>,
    max_steps: usize,
    system_prompt: String,
    /// Context passed to every tool invocation. Defaults to empty
    /// (`ToolContext::default()`) — set via [`with_context`] when the agent
    /// should operate in a specific working dir / conversation.
    ctx: ToolContext,
    /// Prior conversation turns, injected between the system prompt and the
    /// current task at the start of `run`/`run_loop`. Empty by default
    /// (single-turn); set via [`with_history`] when resuming a conversation so
    /// the model sees earlier user/assistant/tool turns as real `Message`s.
    history: Vec<Message>,
    /// Extended-thinking budget for GLM Interleaved Thinking. None = thinking
    /// off (the default for `new`); `build_react_agent` turns it on for glm-4.6.
    thinking: Option<kernel_core::ThinkingConfig>,
}

impl ReactAgent {
    pub fn new(
        model: impl ChatModel + 'static,
        tools: ToolRegistry,
        system_prompt: impl Into<String>,
    ) -> Self {
        Self {
            model: Arc::new(model),
            tools,
            hooks: None,
            max_steps: 12,
            system_prompt: system_prompt.into(),
            ctx: ToolContext::default(),
            history: Vec::new(),
            thinking: None,
        }
    }

    pub fn with_hooks(mut self, hooks: Arc<HookManager>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    pub fn with_max_steps(mut self, n: usize) -> Self {
        self.max_steps = n;
        self
    }

    /// Set the ToolContext forwarded to every tool invocation. Without this,
    /// file-scoped tools receive `working_dir = None` and cannot locate the
    /// project.
    pub fn with_context(mut self, ctx: ToolContext) -> Self {
        self.ctx = ctx;
        self
    }

    /// Inject prior conversation turns as `Message`s prepended (after the system
    /// prompt, before the current task) to the model's history on each run. This
    /// is the ReactAgent analog of the CLI path's prompt-prefix context
    /// injection — but structured (real user/assistant/tool turns, not a flat
    /// output_summary string). Symmetric to `with_context`: a pure builder that
    /// only stores; the actual splice happens in `run`/`run_loop`.
    pub fn with_history(mut self, history: Vec<Message>) -> Self {
        self.history = history;
        self
    }

    /// Enable GLM Interleaved Thinking for this agent's runs. When set, every
    /// model request carries `thinking: {enabled, budget_tokens}`, the
    /// reasoning trace streams live as `AgentEvent::Reasoning`, and prior-turn
    /// thinking is preserved across turns (signature replayed — see
    /// `build_body`). Only models that support extended thinking (glm-4.6)
    /// honor it; a model that doesn't may 400, so leave it unset then.
    pub fn with_thinking(mut self, budget_tokens: u32) -> Self {
        self.thinking = Some(kernel_core::ThinkingConfig { budget_tokens });
        self
    }

    pub async fn run_loop(&self, task: &str, opts: ModelOptions) -> Result<String, Error> {
        let infos = self.tools.infos();
        let model: Arc<dyn ChatModel> = if infos.is_empty() {
            Arc::clone(&self.model)
        } else {
            match self.model.with_tools(&infos) {
                Ok(b) => Arc::from(b),
                Err(e) => {
                    log::warn!("[ReactAgent] with_tools failed, proceeding without tools: {e}");
                    Arc::clone(&self.model)
                }
            }
        };

        let prior_history = self.history.clone();
        let mut history = Vec::with_capacity(2 + prior_history.len());
        history.push(Message::system(&self.system_prompt));
        history.extend(prior_history);
        history.push(Message::user(task));
        for _step in 0..self.max_steps {
            let resp = model.generate(&history, &opts).await?;
            history.push(resp.clone());
            if resp.tool_calls.is_empty() {
                return Ok(resp.content);
            }
            for call in &resp.tool_calls {
                let result = self.execute_tool_call(call, &self.ctx).await;
                history.push(Message {
                    role: Role::Tool,
                    content: result,
                    tool_calls: Vec::new(),
                    tool_call_id: Some(call.id.clone()),
                    reasoning: None,
                    reasoning_signature: None,
                });
            }
        }
        Err(Error::Agent(format!(
            "ReactAgent exceeded {} steps without a final answer",
            self.max_steps
        )))
    }

    async fn execute_tool_call(&self, call: &kernel_core::ToolCall, ctx: &ToolContext) -> String {
        if let Some(hooks) = &self.hooks {
            let action = crate::kernel_impl::hooks::Action::CallTool {
                tool: call.function.name.clone(),
                arguments: call.function.arguments.clone(),
            };
            if let Err(reason) = hooks.before(&action).await {
                return format!("[blocked by {}: {}]", reason.hook, reason.message);
            }
        }
        let result = match self.tools.find(&call.function.name) {
            Some(t) => t
                .invoke(&call.function.arguments, ctx)
                .await
                .unwrap_or_else(|e| format!("[tool error: {e}]")),
            None => format!("[unknown tool: {}]", call.function.name),
        };
        if let Some(hooks) = &self.hooks {
            let outcome = crate::kernel_impl::hooks::ActionOutcome {
                action: crate::kernel_impl::hooks::Action::CallTool {
                    tool: call.function.name.clone(),
                    arguments: call.function.arguments.clone(),
                },
                ok: !result.starts_with("[tool error"),
                diff: None,
                error: if result.starts_with('[') { Some(result.clone()) } else { None },
            };
            let findings = hooks.after(&outcome).await;
            for f in findings {
                log::warn!("[hook] {}: {}", f.rule, f.explanation);
            }
        }
        result
    }
}

#[async_trait]
impl kernel_core::Agent for ReactAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Transparent
    }
    fn capabilities(&self) -> AgentCaps {
        AgentCaps {
            interruptible: true,
            resumable: true,
            injectable_tools: true,
            read_only: self.tools.tools.iter().all(|t| t.is_read_only()),
        }
    }

    fn run(&self, input: AgentInput) -> Result<BoxStream<'static, Result<AgentEvent, kernel_core::Error>>, kernel_core::Error> {
        let model = Arc::clone(&self.model);
        let tools = self.tools.clone();
        let hooks = self.hooks.clone();
        let system_prompt = self.system_prompt.clone();
        let max_steps = self.max_steps;
        let ctx = self.ctx.clone();
        let prior_history = self.history.clone();
        let task = input.prompt;
        let model_opt = input.model;
        let thinking = self.thinking;

        let s = async_stream::try_stream! {
            let infos = tools.infos();
            let bound: Arc<dyn ChatModel> = if infos.is_empty() {
                model
            } else {
                match model.with_tools(&infos) {
                    Ok(b) => Arc::from(b),
                    Err(e) => {
                        log::warn!("[ReactAgent] with_tools failed in stream, no tools: {e}");
                        model
                    }
                }
            };
            let mut history = Vec::with_capacity(2 + prior_history.len());
            history.push(Message::system(&system_prompt));
            history.extend(prior_history.iter().cloned());
            history.push(Message::user(&task));
            let opts = ModelOptions { model: model_opt, thinking, ..Default::default() };
            let mut final_output = String::new();

            for _step in 0..max_steps {
                // Real streaming: consume the model's SSE stream, yielding each
                // text delta as a Token (chat renders token-by-token) while the
                // stream() helper accumulates tool_calls from content_block_start
                // + input_json_delta events. Text + tool_calls are reassembled
                // into one assistant Message for coherent next-turn history.
                use futures::StreamExt;
                let mut turn_stream = bound.stream(&history, &opts).map_err(Error::from)?;
                let mut turn_text = String::new();
                let mut turn_reasoning = String::new();
                let mut turn_tool_calls: Vec<kernel_core::ToolCall> = Vec::new();
                let mut turn_sig: Option<String> = None;
                while let Some(msg_res) = turn_stream.next().await {
                    let msg = msg_res.map_err(Error::from)?;
                    if !msg.content.is_empty() {
                        turn_text.push_str(&msg.content);
                        yield AgentEvent::Token(msg.content.clone());
                    }
                    // GLM Interleaved Thinking: stream the reasoning trace live
                    // (each thinking_delta chunk), and reassemble the full trace
                    // + its signature so the next turn can preserve the block.
                    if let Some(r) = msg.reasoning.as_ref().filter(|s| !s.is_empty()) {
                        turn_reasoning.push_str(r);
                        yield AgentEvent::Reasoning(r.clone());
                    }
                    if !msg.tool_calls.is_empty() {
                        turn_tool_calls = msg.tool_calls;
                    }
                    if let Some(s) = msg.reasoning_signature.as_ref().filter(|s| !s.is_empty()) {
                        turn_sig = Some(s.clone());
                    }
                }
                history.push(Message {
                    role: Role::Assistant,
                    content: turn_text.clone(),
                    tool_calls: turn_tool_calls.clone(),
                    tool_call_id: None,
                    reasoning: if turn_reasoning.is_empty() {
                        None
                    } else {
                        Some(turn_reasoning.clone())
                    },
                    reasoning_signature: turn_sig.clone(),
                });
                if turn_tool_calls.is_empty() {
                    final_output = turn_text;
                    yield AgentEvent::TurnBoundary;
                    break;
                }
                for call in &turn_tool_calls {
                    yield AgentEvent::ToolCall(kernel_core::ToolCallEvent {
                        tool: call.function.name.clone(),
                        arguments: call.function.arguments.clone(),
                        status: kernel_core::ToolCallStatus::Started,
                        result: None,
                    });
                    let blocked = if let Some(h) = &hooks {
                        let action = crate::kernel_impl::hooks::Action::CallTool {
                            tool: call.function.name.clone(),
                            arguments: call.function.arguments.clone(),
                        };
                        match h.before(&action).await {
                            Err(reason) => {
                                let blocked_msg =
                                    format!("[blocked by {}: {}]", reason.hook, reason.message);
                                yield AgentEvent::ToolCall(kernel_core::ToolCallEvent {
                                    tool: call.function.name.clone(),
                                    arguments: call.function.arguments.clone(),
                                    status: kernel_core::ToolCallStatus::Failed,
                                    result: Some(blocked_msg.clone()),
                                });
                                Some(blocked_msg)
                            }
                            Ok(()) => None,
                        }
                    } else {
                        None
                    };
                    let result = match blocked {
                        Some(b) => b,
                        None => match tools.find(&call.function.name) {
                            Some(t) => match t.invoke(&call.function.arguments, &ctx).await {
                                Ok(out) => {
                                    yield AgentEvent::ToolCall(kernel_core::ToolCallEvent {
                                        tool: call.function.name.clone(),
                                        arguments: call.function.arguments.clone(),
                                        status: kernel_core::ToolCallStatus::Succeeded,
                                        result: Some(out.clone()),
                                    });
                                    out
                                }
                                Err(e) => {
                                    let err = format!("[tool error: {e}]");
                                    yield AgentEvent::ToolCall(kernel_core::ToolCallEvent {
                                        tool: call.function.name.clone(),
                                        arguments: call.function.arguments.clone(),
                                        status: kernel_core::ToolCallStatus::Failed,
                                        result: Some(err.clone()),
                                    });
                                    err
                                }
                            },
                            None => format!("[unknown tool: {}]", call.function.name),
                        },
                    };
                    history.push(Message {
                        role: Role::Tool,
                        content: result,
                        tool_calls: Vec::new(),
                        tool_call_id: Some(call.id.clone()),
                        reasoning: None,
                        reasoning_signature: None,
                    });
                }
            }
            yield AgentEvent::Done(AgentOutcome {
                status: AgentRunStatus::Completed,
                files_changed: Vec::new(),
                exit_code: Some(0),
                output_summary: Some(final_output),
                // Transparent agent: honesty is enforced at the call level via
                // HookManager (each tool invocation inspectable before commit),
                // not via post-hoc diff audit. OpaqueAgent fills this instead.
                honesty: None,
            });
        };
        Ok(Box::pin(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_core::ToolInfo;

    #[test]
    fn decode_anthropic_text_block() {
        let v = json!({ "content": [ {"type": "text", "text": "hello"} ] });
        let m = decode_anthropic_message(&v).unwrap();
        assert_eq!(m.content, "hello");
        assert_eq!(m.role, Role::Assistant);
    }

    #[test]
    fn decode_anthropic_tool_use_block() {
        let v = json!({
            "content": [{
                "type": "tool_use",
                "id": "call_1",
                "name": "grep",
                "input": {"pattern": "foo"}
            }]
        });
        let m = decode_anthropic_message(&v).unwrap();
        assert_eq!(m.tool_calls.len(), 1);
        assert_eq!(m.tool_calls[0].function.name, "grep");
        assert_eq!(m.tool_calls[0].id, "call_1");
        assert!(m.tool_calls[0].function.arguments.contains("foo"));
    }

    #[test]
    fn build_body_injects_bound_tools() {
        let mut model = GlmChatModel::bigmodel("k", "glm-4.6");
        model.bound_tools = vec![ToolInfo {
            name: "grep".into(),
            description: "search".into(),
            parameters_schema: json!({"type": "object"}),
        }];
        let body = model.build_body("glm-4.6", &[Message::user("hi")], &ModelOptions::default(), false);
        assert_eq!(body["tools"][0]["name"], "grep");
    }

    #[test]
    fn build_body_omits_tools_when_empty() {
        let model = GlmChatModel::bigmodel("k", "glm-4.6");
        let body = model.build_body("glm-4.6", &[Message::user("hi")], &ModelOptions::default(), false);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn with_tools_returns_bound_clone() {
        let model = GlmChatModel::bigmodel("k", "glm-4.6");
        let _bound = model.with_tools(&[ToolInfo {
            name: "x".into(),
            description: "y".into(),
            parameters_schema: json!({}),
        }]).unwrap();
        let body_orig = model.build_body("m", &[Message::user("a")], &ModelOptions::default(), false);
        assert!(body_orig.get("tools").is_none(), "original stays unbound");
    }

    struct EchoTool;
    #[async_trait]
    impl Tool for EchoTool {
        fn info(&self) -> ToolInfo {
            ToolInfo {
                name: "echo".into(),
                description: "echo the argument back".into(),
                parameters_schema: json!({"type":"object","properties":{"text":{"type":"string"}}}),
            }
        }
        async fn invoke(&self, args: &str, _ctx: &ToolContext) -> Result<String, Error> {
            Ok(format!("echo:{args}"))
        }
    }

    #[test]
    fn registry_finds_by_name() {
        let reg = ToolRegistry::new().with(EchoTool);
        assert!(reg.find("echo").is_some());
        assert!(reg.find("nope").is_none());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn sse_text_delta_yields_assistant_message() {
        let mut bufs = HashMap::new();
        let mut sig = String::new();
        let line = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#;
        let m = handle_sse_line(line, &mut bufs, &mut sig).unwrap();
        assert_eq!(m.content, "hi");
        assert!(m.tool_calls.is_empty());
        assert!(bufs.is_empty(), "text delta must not touch the tool accumulator");
    }

    #[test]
    fn sse_accumulates_tool_use_across_split_json_deltas() {
        let mut bufs = HashMap::new();
        let mut sig = String::new();
        // content_block_start opens a tool_use block at index 1.
        let start = r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_9","name":"read_file"}}"#;
        assert!(handle_sse_line(start, &mut bufs, &mut sig).is_none(), "start yields nothing");
        // input_json_delta arrives in two fragments — Anthropic streams partial JSON.
        let d1 = r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"/a"}}"#;
        let d2 = r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":".txt\"}"}}"#;
        assert!(handle_sse_line(d1, &mut bufs, &mut sig).is_none(), "json delta yields nothing");
        assert!(handle_sse_line(d2, &mut bufs, &mut sig).is_none(), "json delta yields nothing");
        // message_stop reassembles the terminal tool_calls Message.
        let m = handle_sse_line(r#"data: {"type":"message_stop"}"#, &mut bufs, &mut sig).unwrap();
        assert_eq!(m.content, "");
        assert_eq!(m.tool_calls.len(), 1);
        let call = &m.tool_calls[0];
        assert_eq!(call.id, "call_9");
        assert_eq!(call.function.name, "read_file");
        assert_eq!(call.function.arguments, r#"{"path":"/a.txt"}"#);
    }

    #[test]
    fn sse_message_stop_without_tools_yields_none() {
        // A pure-text turn has no tool_use blocks; message_stop yields nothing
        // and the run loop treats the ended stream as a turn boundary.
        let mut bufs = HashMap::new();
        let mut sig = String::new();
        assert!(handle_sse_line(r#"data: {"type":"message_stop"}"#, &mut bufs, &mut sig).is_none());
    }

    #[test]
    fn sse_multiple_tool_calls_preserve_index_order() {
        let mut bufs = HashMap::new();
        let mut sig = String::new();
        // Two tool_use blocks, opened out of index order (1 then 0).
        let s1 = r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"b","name":"second"}}"#;
        let s0 = r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"a","name":"first"}}"#;
        handle_sse_line(s1, &mut bufs, &mut sig);
        handle_sse_line(s0, &mut bufs, &mut sig);
        handle_sse_line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#,
            &mut bufs,
            &mut sig,
        );
        handle_sse_line(
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#,
            &mut bufs,
            &mut sig,
        );
        let m = handle_sse_line(r#"data: {"type":"message_stop"}"#, &mut bufs, &mut sig).unwrap();
        assert_eq!(m.tool_calls.len(), 2);
        // Reassembled in index order regardless of arrival order.
        assert_eq!(m.tool_calls[0].id, "a");
        assert_eq!(m.tool_calls[1].id, "b");
    }

    // --- v1.1: stream run must fill the real tool output into ToolCallEvent.result ---

    /// A scripted ChatModel: each `stream()` call emits the next Message from a
    /// fixed script. Lets us drive the ReactAgent ReAct loop without a live LLM
    /// and assert that real tool output is carried in the Succeeded event.
    #[derive(Clone)]
    struct ScriptedModel {
        script: Arc<Vec<Message>>,
        call: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl ScriptedModel {
        fn new(script: Vec<Message>) -> Self {
            Self {
                script: Arc::new(script),
                call: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl ChatModel for ScriptedModel {
        async fn generate(&self, _: &[Message], _: &ModelOptions) -> Result<Message, Error> {
            Err(Error::Unsupported("ScriptedModel: drive via stream()".into()))
        }
        fn stream(&self, _: &[Message], _: &ModelOptions) -> Result<MessageStream, Error> {
            let idx = self.call.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let msg = self.script.get(idx).cloned().unwrap_or_else(|| Message {
                role: Role::Assistant,
                content: String::new(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                reasoning: None,
                reasoning_signature: None,
            });
            Ok(Box::pin(futures::stream::once(async move { Ok(msg) })))
        }
        fn with_tools(&self, _: &[ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
            Ok(Box::new(self.clone()))
        }
    }

    #[tokio::test]
    async fn run_fills_real_tool_output_into_succeeded_event() {
        use futures::StreamExt;
        use kernel_core::Agent;
        // turn 0: model calls `echo`; turn 1: bare text ends the ReAct loop.
        let call_msg = Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![kernel_core::ToolCall {
                id: "call_1".into(),
                call_type: "function".into(),
                function: kernel_core::FunctionCall {
                    name: "echo".into(),
                    arguments: r#"{"text":"hi"}"#.into(),
                },
            }],
            tool_call_id: None,
            reasoning: None,
            reasoning_signature: None,
        };
        let end_msg = Message::assistant("done");
        let model = ScriptedModel::new(vec![call_msg, end_msg]);
        let reg = ToolRegistry::new().with(EchoTool);
        let agent = ReactAgent::new(model, reg, "sys");
        let mut s = agent
            .run(kernel_core::AgentInput {
                prompt: "go".into(),
                working_dir: None,
                model: None,
                resume_from: None,
            })
            .unwrap();
        let mut succeeded: Option<String> = None;
        while let Some(ev) = s.next().await {
            if let Ok(kernel_core::AgentEvent::ToolCall(tc)) = ev {
                if tc.status == kernel_core::ToolCallStatus::Succeeded {
                    succeeded = tc.result;
                }
            }
        }
        // EchoTool.invoke returns `echo:{args}` — the event must carry that real
        // output, proving the v1.1 fill (not the old empty-status placeholder).
        assert_eq!(succeeded.as_deref(), Some(r#"echo:{"text":"hi"}"#));
    }

    // --- v1.1: reasoning 双协议贯通 (GLM Interleaved + Preserved Thinking) ---

    #[test]
    fn sse_thinking_delta_streams_reasoning_and_carries_signature_on_stop() {
        let mut bufs = HashMap::new();
        let mut sig = String::new();
        // thinking_delta streams the reasoning trace chunk-by-chunk, each chunk
        // yielded as a Message carrying reasoning (content empty, no tool_calls).
        let d1 = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"step "}}"#;
        let d2 = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"one"}}"#;
        let m1 = handle_sse_line(d1, &mut bufs, &mut sig).unwrap();
        assert_eq!(m1.reasoning.as_deref(), Some("step "));
        assert!(m1.content.is_empty());
        assert!(m1.tool_calls.is_empty());
        let m2 = handle_sse_line(d2, &mut bufs, &mut sig).unwrap();
        assert_eq!(m2.reasoning.as_deref(), Some("one"));
        // signature_delta accumulates silently into sig_buf — no Message yielded.
        let sd = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-123"}}"#;
        assert!(handle_sse_line(sd, &mut bufs, &mut sig).is_none());
        assert_eq!(sig, "sig-123");
        // message_stop carries the accumulated signature even with no tools — so
        // a pure reasoning+answer turn still preserves its signature next turn.
        let stop = handle_sse_line(r#"data: {"type":"message_stop"}"#, &mut bufs, &mut sig).unwrap();
        assert!(stop.tool_calls.is_empty());
        assert_eq!(stop.reasoning_signature.as_deref(), Some("sig-123"));
    }

    #[test]
    fn decode_anthropic_thinking_block_into_reasoning_and_signature() {
        let v = json!({
            "content": [
                {"type":"thinking","thinking":"let me reason","signature":"abc"},
                {"type":"text","text":"the answer"}
            ]
        });
        let m = decode_anthropic_message(&v).unwrap();
        assert_eq!(m.content, "the answer");
        assert_eq!(m.reasoning.as_deref(), Some("let me reason"));
        assert_eq!(m.reasoning_signature.as_deref(), Some("abc"));
        // A thinking block with no signature decodes reasoning-only.
        let v2 = json!({"content":[{"type":"thinking","thinking":"unsigned"},{"type":"text","text":"x"}]});
        let m2 = decode_anthropic_message(&v2).unwrap();
        assert_eq!(m2.reasoning.as_deref(), Some("unsigned"));
        assert!(m2.reasoning_signature.is_none());
    }

    // --- v1.1 Task 3: model orchestration (usage extraction → cost) ---

    #[test]
    fn parse_usage_extracts_message_start_input_and_delta_output() {
        let start = r#"data: {"type":"message_start","message":{"usage":{"input_tokens":42}}}"#;
        assert_eq!(parse_usage(start), Some((42, 0)));
        let delta = r#"data: {"type":"message_delta","usage":{"output_tokens":128}}"#;
        assert_eq!(parse_usage(delta), Some((0, 128)));
        // Non-usage event types → None.
        assert_eq!(parse_usage(r#"data: {"type":"content_block_delta"}"#), None);
        // Non-data lines → None.
        assert_eq!(parse_usage("event: ping"), None);
        assert_eq!(parse_usage(""), None);
    }

    #[test]
    fn usage_from_response_reads_usage_object() {
        let v = json!({"usage":{"input_tokens":10,"output_tokens":20}});
        assert_eq!(usage_from_response(&v), (10, 20));
        // Missing usage → (0, 0), not an error.
        let v2 = json!({"content":[]});
        assert_eq!(usage_from_response(&v2), (0, 0));
    }

    #[test]
    fn glm_model_attaches_circuit_and_cost_sink_builders() {
        use crate::cost::circuit_breaker::CircuitBreakerConfig;
        use crate::cost::sink::NullCostSink;
        use std::time::Duration;
        let m = GlmChatModel::bigmodel("k", "glm-4.6")
            .with_circuit(std::sync::Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
                failure_threshold: 1,
                cooldown: Duration::from_secs(60),
                half_open_max: 1,
            })))
            .with_cost_sink(std::sync::Arc::new(NullCostSink));
        assert!(m.circuit.is_some());
        assert!(m.cost_sink.is_some());
    }

    #[test]
    fn shared_glm_circuit_returns_same_instance() {
        // The breaker must be a process-wide singleton so a trip in one agent
        // is observed by all — two calls must hand back the *same* Arc.
        let a = shared_glm_circuit();
        let b = shared_glm_circuit();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn build_body_enables_thinking_and_replays_preserved_thinking_block() {
        use kernel_core::{Message, ModelOptions, Role, ThinkingConfig};
        let model = GlmChatModel::bigmodel("k", "glm-4.6");
        // thinking on: body carries the thinking param, and max_tokens is raised
        // above budget (caller's 1024 < budget 2000 → 2000 + 4096).
        let opts = ModelOptions {
            thinking: Some(ThinkingConfig { budget_tokens: 2000 }),
            max_tokens: Some(1024),
            ..Default::default()
        };
        let body = model.build_body("glm-4.6", &[Message::user("hi")], &opts, true);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 2000);
        assert!(
            body["max_tokens"].as_u64().unwrap() > 2000,
            "max_tokens must exceed budget: {}",
            body["max_tokens"]
        );
        // preserved: an assistant turn that carried reasoning replays it as a
        // leading thinking block (with signature) before the text answer.
        let prior = vec![Message {
            role: Role::Assistant,
            content: "ans".into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning: Some("thought".into()),
            reasoning_signature: Some("sig9".into()),
        }];
        let body2 = model.build_body("glm-4.6", &prior, &ModelOptions::default(), false);
        let assistant = &body2["messages"][0];
        assert_eq!(assistant["role"], "assistant");
        assert_eq!(assistant["content"][0]["type"], "thinking");
        assert_eq!(assistant["content"][0]["thinking"], "thought");
        assert_eq!(assistant["content"][0]["signature"], "sig9");
        assert_eq!(assistant["content"][1]["type"], "text");
        assert_eq!(assistant["content"][1]["text"], "ans");
        // No-reasoning assistant keeps the original string-content shape.
        let plain = model.build_body(
            "glm-4.6",
            &[Message::assistant("hi")],
            &ModelOptions::default(),
            false,
        );
        assert_eq!(plain["messages"][0]["content"], "hi");
    }
}
