//! `ChatModel` impl speaking the **Anthropic Messages API**
//! (`POST {base}/v1/messages`, `x-api-key`, `anthropic-version`).
//!
//! Despite the historical name (`GlmChatModel`), this impl is fully generic:
//! base_url / api_key / model are all injected, with no provider hardcoded.
//! It works against any Anthropic-compatible endpoint (Z.AI's GLM endpoint,
//! Anthropic itself, any proxy that mirrors the Messages API). Renamed from
//! `GlmChatModel` in the multi-protocol refactor — the name now describes the
//! wire protocol, not one vendor.
//!
//! Cross-cutting concerns (HTTP client, circuit breaker, cost/trace sinks,
//! timing, session attribution) live in [`ChatModelShared`]; this module owns
//! only the Anthropic-specific wire shape: `build_body`, the SSE event parse
//! (`handle_sse_line` / `parse_usage`), and response decode
//! (`decode_anthropic_message` / `usage_from_response`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use kernel_core::{
    ChatModel, CostAccumulator, Error, Message, MessageStream, ModelOptions, Role,
};
use serde_json::{json, Value};

use crate::cost::circuit_breaker::{should_failover, CircuitBreaker};
use crate::cost::pricing;
use crate::cost::sink::CostSink;
use crate::kernel_impl::chat_model_shared::ChatModelShared;
use crate::trace::{redact_secrets, truncate, TraceSink};

/// A `ChatModel` calling an Anthropic-compatible Messages API. The base_url,
/// api_key, and model are all injected — nothing about GLM/Z.AI is hardcoded
/// (the old `GlmChatModel` name was a misnomer; the wire is plain Anthropic).
/// All cross-cutting state is held in [`ChatModelShared`].
#[derive(Clone)]
pub struct AnthropicChatModel {
    pub(crate) shared: ChatModelShared,
}

impl AnthropicChatModel {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            shared: ChatModelShared::new(base_url, api_key, model),
        }
    }

    /// Test convenience constructor hardcoding the Z.AI (Zhipu) Anthropic-
    /// compatible endpoint. Production paths use `new` with a resolved
    /// endpoint from `providers.toml`; this exists so the ~15 historical test
    /// sites read clearly.
    pub fn bigmodel(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new("https://open.bigmodel.cn/api/anthropic", api_key, model)
    }

    pub fn with_circuit(self, circuit: Arc<CircuitBreaker>) -> Self {
        Self {
            shared: self.shared.with_circuit(circuit),
        }
    }

    pub fn with_cost_sink(self, sink: Arc<dyn CostSink>) -> Self {
        Self {
            shared: self.shared.with_cost_sink(sink),
        }
    }

    pub fn with_trace_sink(self, sink: Arc<dyn TraceSink>) -> Self {
        Self {
            shared: self.shared.with_trace_sink(sink),
        }
    }

    pub fn with_timing_checker(self, checker: Arc<crate::trace::TimingChecker>) -> Self {
        Self {
            shared: self.shared.with_timing_checker(checker),
        }
    }

    pub fn with_session_id(self, session_id: Option<String>) -> Self {
        Self {
            shared: self.shared.with_session_id(session_id),
        }
    }

    pub(crate) fn build_body(
        &self,
        model: &str,
        messages: &[Message],
        opts: &ModelOptions,
        stream: bool,
    ) -> Value {
        // M5 + parallel-tool-use fix: Anthropic requires ALL tool_results for one
        // assistant turn to live in a SINGLE user message (an array of
        // tool_result blocks), and messages must strictly alternate user/
        // assistant. Our internal history stores one Role::Tool Message per
        // executed call (see the run loop), so a turn that issued N parallel
        // tool_use calls yields N consecutive Tool Messages. Serializing each
        // into its own user message would emit N back-to-back user messages and
        // trip the provider's 400: "tool_use ids were found without tool_result
        // blocks immediately after". Merge consecutive Tool Messages into one
        // user message here, at the wire boundary — the internal
        // one-Message-per-call representation stays intact.
        let mut msgs: Vec<Value> = Vec::with_capacity(messages.len());
        let mut pending_tool_results: Vec<Value> = Vec::new();
        for m in messages.iter().filter(|m| m.role != Role::System) {
            if m.role == Role::Tool {
                pending_tool_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id.as_deref().unwrap_or(""),
                    "content": m.content,
                }));
                continue;
            }
            // A non-Tool message: flush any accumulated tool_results as one
            // user message before serializing the next turn.
            if !pending_tool_results.is_empty() {
                msgs.push(
                    json!({ "role": "user", "content": std::mem::take(&mut pending_tool_results) }),
                );
            }
            let entry = match m.role {
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
                            let mut content: Vec<Value> =
                                vec![json!({"type":"text","text":m.content})];
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
            };
            msgs.push(entry);
        }
        // Flush trailing tool_results: history can legitimately end on Tool
        // messages (the run loop appends them and re-invokes the model).
        if !pending_tool_results.is_empty() {
            msgs.push(
                json!({ "role": "user", "content": std::mem::take(&mut pending_tool_results) }),
            );
        }
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
        if !self.shared.bound_tools.is_empty() {
            let tools: Vec<Value> = self
                .shared
                .bound_tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters_schema,
                    })
                })
                .collect();
            body["tools"] = Value::Array(tools);
        }
        body
    }
}

#[async_trait]
impl ChatModel for AnthropicChatModel {
    fn model_id(&self) -> &str {
        // The resolved id the provider handed back (after model_mapping). The
        // ReactAgent router reads this as the base model when the caller didn't
        // pass one in AgentInput.model, so a user who picked a non-flagship id
        // is routed against that id — not a hardcoded flagship constant.
        &self.shared.model
    }

    async fn generate(&self, messages: &[Message], opts: &ModelOptions) -> Result<Message, Error> {
        let model = opts.model.clone().unwrap_or_else(|| self.shared.model.clone());
        // Circuit breaker: gate the call and record the outcome.
        self.shared.admit_or_err()?;
        let body = self.build_body(&model, messages, opts, false);
        let req_body = truncate(&body.to_string(), 32_000);
        let t0 = Instant::now();
        let resp = self
            .shared
            .client
            .post(format!("{}/v1/messages", self.shared.base_url))
            .header("x-api-key", &self.shared.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                self.shared.record_trace(
                    &model,
                    None,
                    Some("network"),
                    &req_body,
                    None,
                    t0.elapsed().as_millis() as u64,
                    None,
                    None,
                    None,
                    None,
                );
                if let Some(cb) = &self.shared.circuit {
                    cb.record_failure(&self.shared.base_url);
                }
                return Err(Error::Network(e.to_string()));
            }
        };
        // headers received = first-byte for the non-stream path. TTFB is
        // the model "thinking" time (send → first response signal); the body
        // download (resp.json below) is the stream_ms phase.
        let t_first = Instant::now();
        let ttfb_ms = t_first.duration_since(t0).as_millis() as u64;
        let status = resp.status();
        if !status.is_success() {
            if should_failover(Some(status.as_u16()), false) {
                if let Some(cb) = &self.shared.circuit {
                    cb.record_failure(&self.shared.base_url);
                }
            } else if let Some(cb) = &self.shared.circuit {
                // Non-failover 4xx (caller error) is neither success nor an
                // upstream failure: release the HalfOpen probe slot try_admit
                // took. Without this, under half_open_max=1 a single 400 during
                // the probe wedges the circuit in HalfOpen.
                cb.record_probe_inconclusive(&self.shared.base_url);
            }
            // Read the error body BEFORE it's dropped — this is the actual
            // reason (quota, schema, model-not-found).
            let err_body = redact_secrets(&resp.text().await.unwrap_or_default());
            log::warn!(
                "[llm] {} {} -> {}: {}",
                model,
                self.shared.base_url,
                status,
                truncate(&err_body, 500)
            );
            self.shared.record_trace(
                &model,
                Some(status.as_u16()),
                Some("non_2xx"),
                &req_body,
                Some(&truncate(&err_body, 8_192)),
                t0.elapsed().as_millis() as u64,
                None,
                None,
                Some(ttfb_ms),
                None,
            );
            return Err(Error::Model(format!("LLM call failed: {status}")));
        }
        let v: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                self.shared.record_trace(
                    &model,
                    Some(status.as_u16()),
                    Some("decode"),
                    &req_body,
                    None,
                    t0.elapsed().as_millis() as u64,
                    None,
                    None,
                    Some(ttfb_ms),
                    Some(t_first.elapsed().as_millis() as u64),
                );
                if let Some(cb) = &self.shared.circuit {
                    cb.record_failure(&self.shared.base_url);
                }
                return Err(Error::Model(format!("decode: {e}")));
            }
        };
        if let Some(cb) = &self.shared.circuit {
            cb.record_success(&self.shared.base_url);
        }
        // Cost + trace: record token usage; cost is derived in the sink when 0.
        let usage = usage_from_response(&v);
        if let Some(sink) = &self.shared.cost_sink {
            sink.record(&model, usage, 0.0);
        }
        // Trace: clean 2xx — store the raw response body (truncated) so the full
        // request↔response evidence is one query away.
        let resp_body = serde_json::to_string(&v).unwrap_or_default();
        self.shared.record_trace(
            &model,
            Some(status.as_u16()),
            None,
            &req_body,
            Some(&truncate(&resp_body, 32_000)),
            t0.elapsed().as_millis() as u64,
            Some(usage.input),
            Some(usage.output),
            Some(ttfb_ms),
            Some(t_first.elapsed().as_millis() as u64),
        );
        decode_anthropic_message(&v)
    }

    fn stream(&self, messages: &[Message], opts: &ModelOptions) -> Result<MessageStream, Error> {
        let model_clone = self.clone();
        let messages = messages.to_vec();
        let opts = opts.clone();
        let s = async_stream::try_stream! {
            let model_name = opts.model.clone().unwrap_or_else(|| model_clone.shared.model.clone());
            // Circuit breaker gate.
            model_clone.shared.admit_or_err()?;
            let body = model_clone.build_body(&model_name, &messages, &opts, true);
            let req_body = truncate(&body.to_string(), 32_000);
            let t0 = Instant::now();
            let resp = model_clone.shared.client
                .post(format!("{}/v1/messages", model_clone.shared.base_url))
                .header("x-api-key", &model_clone.shared.api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send()
                .await;
            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    model_clone.shared.record_trace(&model_name, None, Some("network"), &req_body, None, t0.elapsed().as_millis() as u64, None, None, None, None);
                    if let Some(cb) = &model_clone.shared.circuit { cb.record_failure(&model_clone.shared.base_url); }
                    Err(Error::Network(e.to_string()))?
                }
            };
            // headers received = first-byte for the non_2xx branch. The
            // streaming branch re-stamps ttfb_at on the FIRST byte chunk.
            let t_first = Instant::now();
            let status = resp.status();
            // 消费 resp:非 2xx 读 error body 再终止流;2xx 取字节流。两 arm 各自
            // move resp(互斥),用 match 而非 if + 块外 use——try_stream! 宏的 ? 让
            // 编译器无法证明 if 块必 return,块外 resp.bytes_stream() 会报
            // use-after-move。match 把 resp 的消费收敛到一处。
            use futures::StreamExt;
            let mut byte_stream = match status.is_success() {
                true => resp.bytes_stream(),
                false => {
                    if should_failover(Some(status.as_u16()), false) {
                        if let Some(cb) = &model_clone.shared.circuit { cb.record_failure(&model_clone.shared.base_url); }
                    } else if let Some(cb) = &model_clone.shared.circuit {
                        cb.record_probe_inconclusive(&model_clone.shared.base_url);
                    }
                    let err_body = redact_secrets(&resp.text().await.unwrap_or_default());
                    log::warn!(
                        "[llm] {} {} -> {}: {}",
                        model_name, model_clone.shared.base_url, status, truncate(&err_body, 500)
                    );
                    model_clone.shared.record_trace(
                        &model_name,
                        Some(status.as_u16()),
                        Some("non_2xx"),
                        &req_body,
                        Some(&truncate(&err_body, 8_192)),
                        t0.elapsed().as_millis() as u64,
                        None,
                        None,
                        Some(t_first.duration_since(t0).as_millis() as u64),
                        None,
                    );
                    Err(Error::Model(format!("LLM stream failed: {status}")))?;
                    unreachable!("non_2xx arm always returns via ? above")
                }
            };
            let mut buf = String::new();
            // Parallel accumulator for the raw SSE stream — unlike `buf` this is
            // never drained, so it holds the full wire response body for the
            // trace. Capped at ~40 KB while accumulating so a long stream can't
            // balloon memory.
            let mut resp_body_buf = String::new();
            // Accumulate tool_use blocks by Anthropic content_block index, then
            // reassemble into a terminal tool_calls Message on message_stop.
            let mut tool_bufs: HashMap<u64, (String, String, String)> = HashMap::new();
            // Accumulates the thinking signature for THIS turn.
            let mut sig_buf = String::new();
            // Accumulate token usage from message_start/message_delta.
            let mut usage = pricing::TokenUsage::default();
            // stamp ttfb on the FIRST streamed byte chunk.
            let mut ttfb_at: Option<Instant> = None;
            while let Some(chunk_res) = byte_stream.next().await {
                let bytes = match chunk_res {
                    Ok(b) => b,
                    Err(e) => {
                        model_clone.shared.record_trace(&model_name, Some(status.as_u16()), Some("stream"), &req_body, None, t0.elapsed().as_millis() as u64, Some(usage.input), Some(usage.output), ttfb_at.map(|t| t.duration_since(t0).as_millis() as u64), ttfb_at.map(|t| t.elapsed().as_millis() as u64));
                        if let Some(cb) = &model_clone.shared.circuit { cb.record_failure(&model_clone.shared.base_url); }
                        Err(Error::Network(e.to_string()))?
                    }
                };
                if ttfb_at.is_none() {
                    ttfb_at = Some(Instant::now());
                }
                buf.push_str(&String::from_utf8_lossy(&bytes));
                if resp_body_buf.len() < 40_000 {
                    resp_body_buf.push_str(&String::from_utf8_lossy(&bytes));
                }
                while let Some(nl) = buf.find('\n') {
                    let line = buf[..nl].trim().to_string();
                    buf.drain(..=nl);
                    if let Some(delta) = parse_usage(&line) {
                        usage = usage.saturating_add(delta);
                    }
                    if let Some(msg) = handle_sse_line(&line, &mut tool_bufs, &mut sig_buf) {
                        yield msg;
                    }
                }
            }
            // Stream consumed cleanly → upstream healthy + record the turn's cost.
            if let Some(cb) = &model_clone.shared.circuit { cb.record_success(&model_clone.shared.base_url); }
            if let Some(sink) = &model_clone.shared.cost_sink {
                sink.record(&model_name, usage, 0.0);
            }
            let ttfb_ms = ttfb_at.map(|t| t.duration_since(t0).as_millis() as u64);
            let stream_ms = ttfb_at.map(|t| t.elapsed().as_millis() as u64);
            model_clone.shared.record_trace(&model_name, Some(status.as_u16()), None, &req_body, Some(&truncate(&resp_body_buf, 32_000)), t0.elapsed().as_millis() as u64, Some(usage.input), Some(usage.output), ttfb_ms, stream_ms);
        };
        Ok(Box::pin(s))
    }

    fn with_tools(&self, tools: &[kernel_core::ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
        let mut clone = self.clone();
        clone.shared.bound_tools = tools.to_vec();
        Ok(Box::new(clone))
    }

    /// Fork this model with a counting cost sink wrapping the parent's DB
    /// sink, so a dispatched sub-agent's LLM calls are tallied into a per-
    /// dispatch accumulator the SubAgentTool reads after the child run — while
    /// still landing in cost_records (attribution preserved via the inner sink).
    fn fork_with_counting_cost(
        &self,
    ) -> Option<(Arc<dyn ChatModel>, Arc<CostAccumulator>)> {
        let accumulator = Arc::new(CostAccumulator::new());
        let counting = Arc::new(crate::cost::sink::CountingCostSink::new(
            self.shared.cost_sink.clone(),
            Arc::clone(&accumulator),
        )) as Arc<dyn CostSink>;
        let forked = self.clone().with_cost_sink(counting);
        Some((Arc::new(forked) as Arc<dyn ChatModel>, accumulator))
    }
}

/// Process-wide shared circuit breaker for Anthropic-compatible endpoints.
/// Every ReactAgent built via `build_react_agent` taps the same breaker so a
/// sustained upstream outage trips the circuit for all sessions at once.
/// State is keyed by base_url inside the breaker, so distinct endpoints
/// coexist under one instance. Lazily initialized on first use.
pub fn shared_anthropic_circuit() -> Arc<CircuitBreaker> {
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
/// `usage.input_tokens` (+ the prompt-cache tier on real Anthropic);
/// `message_delta` carries the cumulative `usage.output_tokens` AND — on GLM —
/// the real `usage.input_tokens`. Reading BOTH fields on message_delta + the
/// caller's `saturating_add` yields the correct input for either provider
/// without double-counting.
pub(crate) fn parse_usage(line: &str) -> Option<pricing::TokenUsage> {
    let data = line.trim().strip_prefix("data: ")?;
    let ev: Value = serde_json::from_str(data).ok()?;
    match ev.get("type").and_then(|t| t.as_str())? {
        "message_start" => {
            let usage = ev.get("message").and_then(|m| m.get("usage"));
            let input = read_u32(usage, "input_tokens");
            let cache_read = read_u32(usage, "cache_read_input_tokens");
            let cache_write = read_u32(usage, "cache_creation_input_tokens");
            Some(pricing::TokenUsage {
                input,
                output: 0,
                cache_read,
                cache_write,
            })
        }
        "message_delta" => {
            let usage = ev.get("usage");
            let input = read_u32(usage, "input_tokens");
            let output = read_u32(usage, "output_tokens");
            Some(pricing::TokenUsage {
                input,
                output,
                cache_read: 0,
                cache_write: 0,
            })
        }
        _ => None,
    }
}

/// Read an optional u64→u32 usage field from a JSON object. Centralized so the
/// branches above stay readable.
fn read_u32(obj: Option<&Value>, key: &str) -> u32 {
    obj.and_then(|u| u.get(key))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32
}

/// Extract usage from a non-streaming Anthropic response. Returns an all-zero
/// `TokenUsage` if no usage object is present — the sink still records the call.
pub(crate) fn usage_from_response(v: &Value) -> pricing::TokenUsage {
    let u = match v.get("usage") {
        Some(u) => u,
        None => return pricing::TokenUsage::default(),
    };
    pricing::TokenUsage {
        input: read_u32(Some(u), "input_tokens"),
        output: read_u32(Some(u), "output_tokens"),
        cache_read: read_u32(Some(u), "cache_read_input_tokens"),
        cache_write: read_u32(Some(u), "cache_creation_input_tokens"),
    }
}

pub(crate) fn decode_anthropic_message(v: &Value) -> Result<Message, Error> {
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
                    let id = block
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args = block
                        .get("input")
                        .map(|i| i.to_string())
                        .unwrap_or_else(|| "{}".to_string());
                    tool_calls.push(kernel_core::ToolCall {
                        id,
                        call_type: "function".into(),
                        function: kernel_core::FunctionCall { name, arguments: args },
                    });
                }
                Some("thinking") => {
                    // Interleaved Thinking content block. Capture the trace +
                    // its signature (needed to preserve the block next turn).
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
/// non-data lines, malformed JSON, and event types that carry no Message. Text
/// deltas become assistant Messages immediately (real streaming); tool_use
/// blocks accumulate and reassemble into a terminal tool_calls Message on
/// message_stop. Extracted from stream() so the tool_use accumulation is
/// unit-testable without HTTP.
pub(crate) fn handle_sse_line(
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
                    let id = block
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    tool_bufs.insert(idx, (id, name, String::new()));
                }
            }
            None
        }
        "content_block_delta" => {
            let dt = ev
                .get("delta")
                .and_then(|d| d.get("type"))
                .and_then(|t| t.as_str());
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
                // live. The signature arrives as a separate signature_delta and
                // is emitted once, on message_stop, via sig_buf.
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
            // terminal message — even with no tool calls.
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
                        arguments: if args.is_empty() {
                            "{}".to_string()
                        } else {
                            args
                        },
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
