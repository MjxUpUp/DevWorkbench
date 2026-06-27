//! `ChatModel` impl speaking the **OpenAI Chat Completions API**
//! (`POST {base}/v1/chat/completions`, `Authorization: Bearer`, streaming via
//! `stream_options.include_usage`). Works against any OpenAI-compatible
//! endpoint: OpenAI itself, DeepSeek, OpenRouter, Together, vLLM, etc. The
//! base_url / api_key / model are all injected — nothing about one vendor is
//! hardcoded.
//!
//! Cross-cutting concerns (HTTP client, circuit breaker, cost/trace sinks,
//! timing, session attribution) live in [`ChatModelShared`]; this module owns
//! only the OpenAI-specific wire shape: `build_body`, the SSE delta parse
//! (`handle_openai_sse_line`), and response decode (`decode_openai_message` /
//! `usage_from_openai_response`).

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

/// A `ChatModel` calling an OpenAI-compatible Chat Completions endpoint. The
/// base_url, api_key, and model are all injected — works for OpenAI, DeepSeek,
/// OpenRouter, or any provider that mirrors the Chat Completions API. All
/// cross-cutting state is held in [`ChatModelShared`].
#[derive(Clone)]
pub struct OpenAIChatModel {
    pub(crate) shared: ChatModelShared,
}

impl OpenAIChatModel {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            shared: ChatModelShared::new(base_url, api_key, model),
        }
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

    /// Resolve the Chat Completions POST URL from a base URL. Trims a trailing
    /// `/`; if the base already ends with a `/v<digits>` version segment
    /// (`/v1`, `/v4`, …), append `/chat/completions` directly; otherwise insert
    /// `/v1/chat/completions`. Centralized so the URL rule is unit-testable
    /// and identical for generate + stream.
    ///
    /// The version check is `/v<digits>`, not just `/v1`: GLM's coding-paas
    /// endpoint is `…/paas/v4`, so a `/v1`-only check would wrongly produce
    /// `…/paas/v4/v1/chat/completions` (HTTP 404). OpenRouter `…/api/v1` and
    /// bare hosts like DeepSeek `api.deepseek.com` / OpenAI `api.openai.com`
    /// (which get `/v1` inserted) are unaffected.
    pub fn chat_completions_url(base: &str) -> String {
        let trimmed = base.trim_end_matches('/');
        // Last path segment is /v<digits>? GLM uses /v4; previously only /v1 matched.
        let has_version = trimmed.rsplit_once('/').is_some_and(|(_, last)| {
            last.strip_prefix('v')
                .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
        });
        if has_version {
            format!("{trimmed}/chat/completions")
        } else {
            format!("{trimmed}/v1/chat/completions")
        }
    }

    /// Build the Chat Completions request body. Key shape differences vs the
    /// Anthropic Messages API:
    /// - `system` is a normal first message `{role:"system",content}`, NOT a
    ///   top-level field;
    /// - each tool result is its OWN `{role:"tool",tool_call_id,content}`
    ///   message (the API tolerates consecutive tool messages for parallel
    ///   calls, so they are NOT merged);
    /// - an assistant turn with tool calls carries `tool_calls:[{id,type:
    ///   "function",function:{name,arguments}}]` where `arguments` is a STRING
    ///   (the JSON-encoded args), not an object;
    /// - tools wrap each entry in `{type:"function",function:{name,description,
    ///   parameters}}` + a top-level `tool_choice:"auto"`;
    /// - streaming adds `stream_options:{include_usage:true}` so the final
    ///   chunk carries usage.
    pub(crate) fn build_body(
        &self,
        model: &str,
        messages: &[Message],
        opts: &ModelOptions,
        stream: bool,
    ) -> Value {
        let mut msgs: Vec<Value> = Vec::with_capacity(messages.len());
        for m in messages {
            match m.role {
                Role::System => {
                    msgs.push(json!({ "role": "system", "content": m.content }));
                }
                Role::User => {
                    msgs.push(json!({ "role": "user", "content": m.content }));
                }
                Role::Tool => {
                    // One message per tool result, tied back by tool_call_id.
                    // Consecutive tool messages are legal here (parallel function
                    // call results), so they stay unmerged — opposite of the
                    // Anthropic Messages API, which requires them collapsed.
                    msgs.push(json!({
                        "role": "tool",
                        "tool_call_id": m.tool_call_id.as_deref().unwrap_or(""),
                        "content": m.content,
                    }));
                }
                Role::Assistant => {
                    if m.tool_calls.is_empty() {
                        msgs.push(json!({ "role": "assistant", "content": m.content }));
                    } else {
                        // assistant turn that requested tools: content + tool_calls.
                        // `arguments` is a STRING (JSON-encoded), matching the
                        // wire shape the stream accumulates fragments into.
                        let tool_calls: Vec<Value> = m
                            .tool_calls
                            .iter()
                            .map(|tc| {
                                json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.function.name,
                                        "arguments": tc.function.arguments,
                                    }
                                })
                            })
                            .collect();
                        msgs.push(json!({
                            "role": "assistant",
                            "content": m.content,
                            "tool_calls": tool_calls,
                        }));
                    }
                }
            }
        }
        let mut body = json!({
            "model": model,
            "messages": msgs,
            "max_tokens": opts.max_tokens.unwrap_or(4096),
            "stream": stream,
        });
        // OpenAI Chat Completions has no native thinking block; opts.thinking is
        // intentionally ignored (o-series reasoning content isn't exposed on the
        // standard API, and DeepSeek-R1's reasoning is surfaced separately).
        if stream {
            body["stream_options"] = json!({ "include_usage": true });
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
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters_schema,
                        }
                    })
                })
                .collect();
            body["tools"] = Value::Array(tools);
            body["tool_choice"] = json!("auto");
        }
        body
    }
}

#[async_trait]
impl ChatModel for OpenAIChatModel {
    fn model_id(&self) -> &str {
        &self.shared.model
    }

    async fn generate(&self, messages: &[Message], opts: &ModelOptions) -> Result<Message, Error> {
        let model = opts.model.clone().unwrap_or_else(|| self.shared.model.clone());
        self.shared.admit_or_err()?;
        let body = self.build_body(&model, messages, opts, false);
        let req_body = truncate(&body.to_string(), 32_000);
        let t0 = Instant::now();
        let url = Self::chat_completions_url(&self.shared.base_url);
        let resp = self
            .shared
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.shared.api_key))
            .json(&body)
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                self.shared.record_trace(
                    &model, None, Some("network"), &req_body, None,
                    t0.elapsed().as_millis() as u64, None, None, None, None,
                );
                if let Some(cb) = &self.shared.circuit {
                    cb.record_failure(&self.shared.base_url);
                }
                return Err(Error::Network(e.to_string()));
            }
        };
        let t_first = Instant::now();
        let ttfb_ms = t_first.duration_since(t0).as_millis() as u64;
        let status = resp.status();
        if !status.is_success() {
            if should_failover(Some(status.as_u16()), false) {
                if let Some(cb) = &self.shared.circuit {
                    cb.record_failure(&self.shared.base_url);
                }
            } else if let Some(cb) = &self.shared.circuit {
                // Non-failover 4xx (caller error): release the HalfOpen probe
                // slot admit_or_err took, mirroring the Anthropic impl.
                cb.record_probe_inconclusive(&self.shared.base_url);
            }
            let err_body = redact_secrets(&resp.text().await.unwrap_or_default());
            log::warn!(
                "[llm] {} {} -> {}: {}",
                model, self.shared.base_url, status, truncate(&err_body, 500)
            );
            self.shared.record_trace(
                &model, Some(status.as_u16()), Some("non_2xx"), &req_body,
                Some(&truncate(&err_body, 8_192)), t0.elapsed().as_millis() as u64,
                None, None, Some(ttfb_ms), None,
            );
            return Err(Error::Model(format!("LLM call failed: {status}")));
        }
        let v: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                self.shared.record_trace(
                    &model, Some(status.as_u16()), Some("decode"), &req_body, None,
                    t0.elapsed().as_millis() as u64, None, None, Some(ttfb_ms),
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
        let usage = usage_from_openai_response(&v);
        if let Some(sink) = &self.shared.cost_sink {
            sink.record(&model, usage, 0.0);
        }
        let resp_body = serde_json::to_string(&v).unwrap_or_default();
        self.shared.record_trace(
            &model, Some(status.as_u16()), None, &req_body,
            Some(&truncate(&resp_body, 32_000)), t0.elapsed().as_millis() as u64,
            Some(usage.input), Some(usage.output), Some(ttfb_ms),
            Some(t_first.elapsed().as_millis() as u64),
        );
        decode_openai_message(&v)
    }

    fn stream(&self, messages: &[Message], opts: &ModelOptions) -> Result<MessageStream, Error> {
        let model_clone = self.clone();
        let messages = messages.to_vec();
        let opts = opts.clone();
        let s = async_stream::try_stream! {
            let model_name = opts.model.clone().unwrap_or_else(|| model_clone.shared.model.clone());
            model_clone.shared.admit_or_err()?;
            let body = model_clone.build_body(&model_name, &messages, &opts, true);
            let req_body = truncate(&body.to_string(), 32_000);
            let t0 = Instant::now();
            let url = Self::chat_completions_url(&model_clone.shared.base_url);
            let resp = model_clone.shared.client
                .post(&url)
                .header("Authorization", format!("Bearer {}", model_clone.shared.api_key))
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
            let t_first = Instant::now();
            let status = resp.status();
            // 消费 resp:非 2xx 读 error body 再终止流;2xx 取字节流。两 arm 各自
            // move resp(互斥),用 match 而非 if + 块外 use——try_stream! 的 ? 让
            // 编译器无法证明 if 块必 return,块外 resp.bytes_stream() 会 use-after-move。
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
                        &model_name, Some(status.as_u16()), Some("non_2xx"), &req_body,
                        Some(&truncate(&err_body, 8_192)), t0.elapsed().as_millis() as u64,
                        None, None, Some(t_first.duration_since(t0).as_millis() as u64), None,
                    );
                    Err(Error::Model(format!("LLM stream failed: {status}")))?;
                    unreachable!("non_2xx arm always returns via ? above")
                }
            };
            let mut buf = String::new();
            let mut resp_body_buf = String::new();
            // tool_call delta accumulator keyed by the chunk's tool_calls[].index:
            // (id, name, arguments). The id + function.name arrive on the first
            // delta for that index; subsequent deltas append string fragments to
            // arguments (which is NOT valid JSON until the call finishes).
            let mut tool_bufs: HashMap<u64, (String, String, String)> = HashMap::new();
            let mut usage = pricing::TokenUsage::default();
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
                    // [DONE] sentinel → stream end. Flush any tool_calls that
                    // accumulated without an explicit finish_reason (some
                    // gateways elide it) so the run loop still gets its message.
                    if line == "data: [DONE]" {
                        if let Some(msg) = flush_tool_calls(&mut tool_bufs) {
                            yield msg;
                        }
                        break;
                    }
                    // Usage rides on the final chunk (choices empty).
                    if let Some(u) = usage_from_openai_sse(&line) {
                        usage = u;
                    }
                    if let Some(msg) = handle_openai_sse_line(&line, &mut tool_bufs) {
                        yield msg;
                    }
                }
            }
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

    /// Fork this model with a counting cost sink wrapping the parent's DB sink,
    /// so a dispatched sub-agent's LLM calls are tallied into a per-dispatch
    /// accumulator the SubAgentTool reads after the child run — while still
    /// landing in cost_records (attribution preserved via the inner sink).
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

/// Process-wide shared circuit breaker for OpenAI-compatible endpoints.
/// Separate from the Anthropic breaker so one protocol's outage doesn't trip
/// the other. State is keyed by base_url inside the breaker, so distinct
/// endpoints coexist under one instance. Lazily initialized on first use.
pub fn shared_openai_circuit() -> Arc<CircuitBreaker> {
    static CIRCUIT: std::sync::OnceLock<Arc<CircuitBreaker>> = std::sync::OnceLock::new();
    CIRCUIT
        .get_or_init(|| {
            Arc::new(CircuitBreaker::new(
                crate::cost::circuit_breaker::CircuitBreakerConfig::default(),
            ))
        })
        .clone()
}

/// Read an optional u64→u32 usage field from a JSON object. Centralized so the
/// branches stay readable.
fn read_u32(obj: Option<&Value>, key: &str) -> u32 {
    obj.and_then(|u| u.get(key))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32
}

/// Extract usage from a non-streaming OpenAI response. `prompt_tokens`→input,
/// `completion_tokens`→output, `prompt_tokens_details.cached_tokens`→cache_read
/// (OpenAI's prompt-cache tier; absent on DeepSeek). cache_write is always 0
/// (OpenAI has no separate write tier). Returns all-zero if no usage object.
pub(crate) fn usage_from_openai_response(v: &Value) -> pricing::TokenUsage {
    let u = match v.get("usage") {
        Some(u) => u,
        None => return pricing::TokenUsage::default(),
    };
    let cache_read = u
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0) as u32;
    pricing::TokenUsage {
        input: read_u32(Some(u), "prompt_tokens"),
        output: read_u32(Some(u), "completion_tokens"),
        cache_read,
        cache_write: 0,
    }
}

/// Extract usage from the final streaming chunk (the one with empty `choices`
/// and a top-level `usage`, present because we sent `stream_options.include_usage`).
/// Returns None for any non-usage chunk. Mirrors [`usage_from_openai_response`].
fn usage_from_openai_sse(line: &str) -> Option<pricing::TokenUsage> {
    let data = line.trim().strip_prefix("data: ")?;
    let ev: Value = serde_json::from_str(data).ok()?;
    let u = ev.get("usage")?;
    let cache_read = u
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0) as u32;
    Some(pricing::TokenUsage {
        input: read_u32(Some(u), "prompt_tokens"),
        output: read_u32(Some(u), "completion_tokens"),
        cache_read,
        cache_write: 0,
    })
}

/// Decode a non-streaming Chat Completions response into a `Message`. Reads
/// `choices[0].message` (content + tool_calls); arguments stay as the raw JSON
/// string the API returns. Returns an error if the response shape is unexpected
/// (no choices) so a malformed upstream surfaces loudly instead of an empty msg.
pub(crate) fn decode_openai_message(v: &Value) -> Result<Message, Error> {
    let msg = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .ok_or_else(|| Error::Model("OpenAI response missing choices[0].message".into()))?;
    let content = msg
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let mut tool_calls = Vec::new();
    if let Some(tcs) = msg.get("tool_calls").and_then(|t| t.as_array()) {
        for tc in tcs {
            let id = tc
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("")
                .to_string();
            let func = tc.get("function").cloned().unwrap_or(json!({}));
            let name = func
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            // arguments is a JSON-encoded string; default to "{}" when absent so
            // a parameterless call still parses downstream.
            let args = func
                .get("arguments")
                .and_then(|a| a.as_str())
                .unwrap_or("{}")
                .to_string();
            tool_calls.push(kernel_core::ToolCall {
                id,
                call_type: "function".into(),
                function: kernel_core::FunctionCall { name, arguments: args },
            });
        }
    }
    Ok(Message {
        role: Role::Assistant,
        content,
        tool_calls,
        tool_call_id: None,
        reasoning: None,
        reasoning_signature: None,
    })
}

/// Reassemble accumulated tool_calls (keyed by chunk index) into a terminal
/// tool_calls Message, sorted by index so parallel calls arrive in declaration
/// order. Empty arguments default to "{}". Returns None when nothing
/// accumulated (so a non-tool stream never yields a spurious empty message).
fn flush_tool_calls(tool_bufs: &mut HashMap<u64, (String, String, String)>) -> Option<Message> {
    if tool_bufs.is_empty() {
        return None;
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
    if tool_calls.is_empty() {
        return None;
    }
    Some(Message {
        role: Role::Assistant,
        content: String::new(),
        tool_calls,
        tool_call_id: None,
        reasoning: None,
        reasoning_signature: None,
    })
}

/// Parse one SSE `data: <json>` line from a Chat Completions stream, mutate the
/// tool_call accumulator, and return any Message to yield:
/// - `delta.content` → an assistant text Message immediately (real streaming);
/// - `delta.tool_calls[]` → accumulate by `index` (id + function.name on first
///   delta, function.arguments fragments appended after);
/// - `finish_reason == "tool_calls"` → reassemble + return a terminal
///   tool_calls Message (the caller's `[DONE]` guard flushes any that slipped
///   through without an explicit finish_reason).
/// Returns None for non-data lines, `[DONE]`, malformed JSON, and chunks that
/// carry no Message. Extracted from stream() so the tool_call accumulation is
/// unit-testable without HTTP.
pub(crate) fn handle_openai_sse_line(
    line: &str,
    tool_bufs: &mut HashMap<u64, (String, String, String)>,
) -> Option<Message> {
    let data = line.trim().strip_prefix("data: ")?;
    if data == "[DONE]" {
        return None;
    }
    let ev: Value = serde_json::from_str(data).ok()?;
    let choice = ev
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())?;
    let delta = choice.get("delta");
    // Text delta → yield immediately.
    if let Some(content) = delta
        .and_then(|d| d.get("content"))
        .and_then(|c| c.as_str())
    {
        if !content.is_empty() {
            return Some(Message::assistant(content.to_string()));
        }
    }
    // tool_calls delta: accumulate by index.
    if let Some(tcs) = delta
        .and_then(|d| d.get("tool_calls"))
        .and_then(|t| t.as_array())
    {
        for tc in tcs {
            let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
            let slot = tool_bufs
                .entry(idx)
                .or_insert_with(|| (String::new(), String::new(), String::new()));
            if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                slot.0 = id.to_string();
            }
            if let Some(func) = tc.get("function") {
                if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                    slot.1.push_str(name);
                }
                if let Some(args) = func.get("arguments").and_then(|a| a.as_str()) {
                    slot.2.push_str(args);
                }
            }
        }
    }
    // finish_reason == "tool_calls" → reassemble + emit the terminal Message.
    if choice
        .get("finish_reason")
        .and_then(|f| f.as_str())
        == Some("tool_calls")
    {
        return flush_tool_calls(tool_bufs);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_core::{FunctionCall, ToolCall, ToolInfo};

    const NONSTREAM_TEXT: &str = include_str!("../../tests/fixtures/openai/nonstream_text.json");
    const STREAM_TEXT: &str = include_str!("../../tests/fixtures/openai/stream_text.sse");
    const STREAM_TOOL_USE: &str = include_str!("../../tests/fixtures/openai/stream_tool_use.sse");
    const STREAM_TOOL_USE_FRAG: &str =
        include_str!("../../tests/fixtures/openai/stream_tool_use_fragmented.sse");

    fn assistant_tool_call(id: &str, name: &str, args: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: id.into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: name.into(),
                    arguments: args.into(),
                },
            }],
            tool_call_id: None,
            reasoning: None,
            reasoning_signature: None,
        }
    }

    fn tool_result(id: &str, content: &str) -> Message {
        Message {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(id.into()),
            reasoning: None,
            reasoning_signature: None,
        }
    }

    // ---- chat_completions_url ----

    #[test]
    fn url_inserts_v1_for_bare_base() {
        assert_eq!(
            OpenAIChatModel::chat_completions_url("https://api.deepseek.com"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        // Trailing slash is trimmed before the /v1 check.
        assert_eq!(
            OpenAIChatModel::chat_completions_url("https://api.openai.com/"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn url_appends_when_base_ends_with_v1() {
        // OpenRouter (…/api/v1) and self-hosted gateways already include /v1.
        assert_eq!(
            OpenAIChatModel::chat_completions_url("https://openrouter.ai/api/v1"),
            "https://openrouter.ai/api/v1/chat/completions"
        );
    }

    #[test]
    fn url_appends_for_non_v1_version_segment() {
        // GLM's coding-paas endpoint pins /v4 — append /chat/completions
        // directly, NOT insert another /v1 (would yield /v4/v1/… → HTTP 404).
        assert_eq!(
            OpenAIChatModel::chat_completions_url("https://open.bigmodel.cn/api/coding/paas/v4"),
            "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions"
        );
        // Trailing slash is trimmed before the version check, so /v4/ still works.
        assert_eq!(
            OpenAIChatModel::chat_completions_url("https://open.bigmodel.cn/api/coding/paas/v4/"),
            "https://open.bigmodel.cn/api/coding/paas/v4/chat/completions"
        );
    }

    // ---- build_body wire shape ----

    #[test]
    fn build_body_system_is_first_message_not_top_level() {
        // Opposite of Anthropic: the system prompt is a normal messages[0], and
        // there is NO top-level `system` field.
        let m = OpenAIChatModel::new("https://x", "k", "deepseek-chat");
        let body = m.build_body(
            "deepseek-chat",
            &[Message::system("you are helpful"), Message::user("hi")],
            &ModelOptions::default(),
            false,
        );
        let msgs = body.get("messages").and_then(|m| m.as_array()).unwrap();
        assert_eq!(
            msgs[0].get("role").and_then(|r| r.as_str()),
            Some("system")
        );
        assert_eq!(
            msgs[0].get("content").and_then(|c| c.as_str()),
            Some("you are helpful")
        );
        assert!(
            body.get("system").is_none(),
            "OpenAI must NOT emit a top-level system field"
        );
    }

    #[test]
    fn build_body_tool_results_are_separate_messages_not_merged() {
        // Opposite of Anthropic: each tool result is its own role:tool message
        // (OpenAI tolerates consecutive tool messages for parallel calls).
        let m = OpenAIChatModel::new("https://x", "k", "deepseek-chat");
        let assistant = assistant_tool_call("call_1", "f", "{}");
        let body = m.build_body(
            "deepseek-chat",
            &[
                assistant,
                tool_result("call_1", "r1"),
                tool_result("call_2", "r2"),
            ],
            &ModelOptions::default(),
            false,
        );
        let msgs = body.get("messages").and_then(|m| m.as_array()).unwrap();
        let tool_msgs: Vec<&Value> = msgs
            .iter()
            .filter(|v| v.get("role").and_then(|r| r.as_str()) == Some("tool"))
            .collect();
        assert_eq!(tool_msgs.len(), 2, "each tool result must be its own message");
        assert_eq!(
            tool_msgs[0].get("tool_call_id").and_then(|t| t.as_str()),
            Some("call_1")
        );
        assert_eq!(
            tool_msgs[1].get("tool_call_id").and_then(|t| t.as_str()),
            Some("call_2")
        );
    }

    #[test]
    fn build_body_assistant_tool_calls_arguments_are_string() {
        // arguments must be a JSON STRING, not an object — matches the wire
        // shape the stream accumulates fragments into.
        let m = OpenAIChatModel::new("https://x", "k", "deepseek-chat");
        let body = m.build_body(
            "deepseek-chat",
            &[assistant_tool_call("call_1", "f", "{\"a\":1}")],
            &ModelOptions::default(),
            false,
        );
        let args = body
            .get("messages")
            .and_then(|m| m.as_array())
            .unwrap()[0]
            .get("tool_calls")
            .and_then(|t| t.as_array())
            .unwrap()[0]
            .get("function")
            .and_then(|f| f.get("arguments"))
            .unwrap();
        assert_eq!(
            args.as_str(),
            Some("{\"a\":1}"),
            "arguments must be a JSON string, not an object"
        );
    }

    #[test]
    fn build_body_tools_use_function_wrapper_and_auto_choice() {
        // tools wrap each entry in {type:"function",function:{...}} and the body
        // carries tool_choice:"auto" — both required for the API to call tools.
        let mut m = OpenAIChatModel::new("https://x", "k", "deepseek-chat");
        m.shared.bound_tools = vec![ToolInfo {
            name: "get_weather".into(),
            description: "weather".into(),
            parameters_schema: json!({"type":"object"}),
        }];
        let body = m.build_body(
            "deepseek-chat",
            &[Message::user("hi")],
            &ModelOptions::default(),
            false,
        );
        let tool = &body.get("tools").and_then(|t| t.as_array()).unwrap()[0];
        assert_eq!(tool.get("type").and_then(|t| t.as_str()), Some("function"));
        assert_eq!(
            tool.get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str()),
            Some("get_weather")
        );
        assert_eq!(
            body.get("tool_choice").and_then(|c| c.as_str()),
            Some("auto")
        );
    }

    #[test]
    fn build_body_stream_adds_include_usage_and_no_thinking() {
        // Streaming must request usage on the final chunk; thinking is ignored
        // (OpenAI has no native thinking block on the standard API).
        let m = OpenAIChatModel::new("https://x", "k", "deepseek-chat");
        let body = m.build_body(
            "deepseek-chat",
            &[Message::user("hi")],
            &ModelOptions::default(),
            true,
        );
        assert_eq!(body.get("stream").and_then(|s| s.as_bool()), Some(true));
        assert_eq!(
            body.get("stream_options")
                .and_then(|o| o.get("include_usage"))
                .and_then(|u| u.as_bool()),
            Some(true)
        );
        assert!(body.get("thinking").is_none());
    }

    // ---- decode_openai_message ----

    #[test]
    fn decode_nonstream_text() {
        let v: Value = serde_json::from_str(NONSTREAM_TEXT).unwrap();
        let msg = decode_openai_message(&v).unwrap();
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content, "Hello from OpenAI-compatible API");
        assert!(msg.tool_calls.is_empty());
    }

    #[test]
    fn decode_nonstream_tool_calls_arguments_kept_as_string() {
        let v = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "get_weather", "arguments": "{\"city\":\"Beijing\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let msg = decode_openai_message(&v).unwrap();
        assert_eq!(msg.tool_calls.len(), 1);
        assert_eq!(msg.tool_calls[0].id, "call_1");
        assert_eq!(msg.tool_calls[0].function.name, "get_weather");
        // arguments stay as the raw JSON string the API returns.
        assert_eq!(msg.tool_calls[0].function.arguments, "{\"city\":\"Beijing\"}");
    }

    #[test]
    fn decode_missing_choices_is_error_not_empty_message() {
        // A malformed upstream (no choices) must surface as an error, not a
        // silent empty assistant message.
        let err = decode_openai_message(&json!({"id":"x"})).unwrap_err();
        assert!(matches!(err, Error::Model(_)));
    }

    // ---- usage ----

    #[test]
    fn usage_from_response_reads_prompt_completion_and_cache() {
        let v = json!({
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "total_tokens": 150,
                "prompt_tokens_details": { "cached_tokens": 30 }
            }
        });
        let u = usage_from_openai_response(&v);
        assert_eq!(u.input, 100);
        assert_eq!(u.output, 50);
        assert_eq!(u.cache_read, 30);
        assert_eq!(u.cache_write, 0);
    }

    #[test]
    fn usage_from_response_missing_usage_is_zero() {
        let u = usage_from_openai_response(&json!({"choices":[]}));
        assert_eq!(u.input, 0);
        assert_eq!(u.output, 0);
    }

    #[test]
    fn usage_from_response_without_cache_details_is_zero_cache() {
        // DeepSeek returns no prompt_tokens_details — cache must be 0, not error.
        let v = json!({"usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}});
        let u = usage_from_openai_response(&v);
        assert_eq!(u.input, 5);
        assert_eq!(u.output, 3);
        assert_eq!(u.cache_read, 0);
    }

    #[test]
    fn usage_from_final_chunk_reads_prompt_completion_and_cache() {
        let line = r#"data: {"id":"x","choices":[],"usage":{"prompt_tokens":8,"completion_tokens":2,"total_tokens":10,"prompt_tokens_details":{"cached_tokens":1}}}"#;
        let u = usage_from_openai_sse(line).unwrap();
        assert_eq!(u.input, 8);
        assert_eq!(u.output, 2);
        assert_eq!(u.cache_read, 1);
    }

    #[test]
    fn usage_from_sse_non_usage_chunk_is_none() {
        // A normal content-delta chunk carries no usage → None.
        let line = r#"data: {"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":null}]}"#;
        assert!(usage_from_openai_sse(line).is_none());
    }

    // ---- streaming SSE parse (handle_openai_sse_line + flush_tool_calls) ----

    /// Drive the SSE line parser over a full stream and collect yielded
    /// messages. Mirrors how the real stream() loop splits on newlines + calls
    /// the handler, so a fixture exercises the exact production path.
    fn parse_sse(sse: &str) -> Vec<Message> {
        let mut tool_bufs = HashMap::new();
        let mut out = Vec::new();
        for line in sse.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line == "data: [DONE]" {
                if let Some(m) = flush_tool_calls(&mut tool_bufs) {
                    out.push(m);
                }
                break;
            }
            // usage is captured into the loop's accumulator in production; the
            // harness reads it here only to exercise parse-ability.
            let _ = usage_from_openai_sse(line);
            if let Some(m) = handle_openai_sse_line(line, &mut tool_bufs) {
                out.push(m);
            }
        }
        out
    }

    #[test]
    fn stream_text_deltas_concatenate() {
        let msgs = parse_sse(STREAM_TEXT);
        let text: String = msgs
            .iter()
            .filter(|m| m.role == Role::Assistant && !m.content.is_empty())
            .map(|m| m.content.clone())
            .collect();
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn stream_single_tool_call_assembles_valid_json() {
        // The id + name arrive on the first delta; the arguments fragment on the
        // second. They must concatenate into valid JSON on finish_reason.
        let msgs = parse_sse(STREAM_TOOL_USE);
        let tool_msg = msgs
            .iter()
            .find(|m| !m.tool_calls.is_empty())
            .expect("a tool_calls message");
        assert_eq!(tool_msg.tool_calls.len(), 1);
        let tc = &tool_msg.tool_calls[0];
        assert_eq!(tc.id, "call_1");
        assert_eq!(tc.function.name, "get_weather");
        let parsed: Value =
            serde_json::from_str(&tc.function.arguments).expect("arguments are valid JSON");
        assert_eq!(parsed.get("city").and_then(|c| c.as_str()), Some("Beijing"));
    }

    #[test]
    fn stream_fragmented_multi_index_tool_calls_assemble_in_order() {
        // Two parallel calls (index 0 + 1), each with arguments split across
        // multiple deltas. The reassembled message must order by index and each
        // arguments string must be valid JSON.
        let msgs = parse_sse(STREAM_TOOL_USE_FRAG);
        let tool_msg = msgs
            .iter()
            .find(|m| !m.tool_calls.is_empty())
            .expect("a tool_calls message");
        assert_eq!(tool_msg.tool_calls.len(), 2);
        // index 0 first.
        assert_eq!(tool_msg.tool_calls[0].id, "call_a");
        assert_eq!(tool_msg.tool_calls[0].function.name, "search");
        let a: Value = serde_json::from_str(&tool_msg.tool_calls[0].function.arguments).unwrap();
        assert_eq!(a.get("q").and_then(|q| q.as_str()), Some("rust"));
        // index 1 second.
        assert_eq!(tool_msg.tool_calls[1].id, "call_b");
        assert_eq!(tool_msg.tool_calls[1].function.name, "read");
        let b: Value = serde_json::from_str(&tool_msg.tool_calls[1].function.arguments).unwrap();
        assert_eq!(b.get("path").and_then(|p| p.as_str()), Some("src/main.rs"));
    }
}
