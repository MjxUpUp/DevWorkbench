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
use std::time::Instant;
use tokio::sync::Semaphore;

use async_trait::async_trait;
use futures::stream::BoxStream;
use kernel_core::{
    AgentCaps, AgentEvent, AgentInput, AgentKind, AgentOutcome, AgentRunStatus, ChatModel,
    CostAccumulator, CostTally, Error, Message, MessageStream, ModelOptions, Role, Tool,
    ToolContext, ToolInfo,
};
use serde_json::{json, Value};

use crate::cost::circuit_breaker::{should_failover, CircuitBreaker};
use crate::cost::pricing;
use crate::cost::sink::CostSink;
use crate::kernel_impl::hooks::HookManager;
use crate::kernel_impl::llm_recovery::{
    classify_llm_error, fatal_user_message, retry_delay, should_retry, FatalReason, LlmErrorKind,
    MAX_ATTEMPTS,
};
use crate::trace::{redact_secrets, truncate, LlmTrace, TraceSink};

/// Injectable audit callback signature (project audit: cargo check + assertion
/// weakening scan). Shared by the config field, the builder, and test stubs.
type AuditFn = Arc<dyn Fn(&std::path::Path, &str) -> Value + Send + Sync>;
/// Per-step model router callback: (history, base_model) -> chosen model_id.
type ModelRouterFn = Arc<dyn Fn(&[Message], &str) -> String + Send + Sync>;

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
    /// Optional trace sink — records the request/response of every LLM HTTP
    /// call to `llm_traces`. None = untraced (tests / ad-hoc agents). The
    /// observability layer: keeps the real build_body + error body on disk so a
    /// failed session's root cause is queryable, instead of being lost to a
    /// bare status string.
    trace_sink: Option<Arc<dyn TraceSink>>,
    /// B3 optional timing checker — flags slow LLM turns (total latency or
    /// time-to-first-byte over threshold) so a hung/stalled model call is
    /// surfaced as a warn log instead of silently inflating latency. None =
    /// unchecked (tests / ad-hoc agents). Shared across with_tools clones.
    timing_checker: Option<Arc<crate::trace::TimingChecker>>,
    /// The session id this model is serving (for trace attribution). Set by
    /// build_react_agent from the driver's session id; None for ad-hoc/test
    /// agents. Passed to trace_sink.record_llm_call so traces join the session.
    session_id: Option<String>,
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
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            bound_tools: Vec::new(),
            circuit: None,
            cost_sink: None,
            trace_sink: None,
            timing_checker: None,
            session_id: None,
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

    /// Attach a trace sink that records every LLM HTTP call (request body,
    /// HTTP status, error body on non-2xx, latency, tokens) to `llm_traces`.
    pub fn with_trace_sink(mut self, sink: Arc<dyn TraceSink>) -> Self {
        self.trace_sink = Some(sink);
        self
    }

    /// Attach a TimingChecker (B3) that flags slow LLM turns. The checker is
    /// invoked once per recorded call with the turn's latency + ttfb; a
    /// threshold crossing is logged at warn. Pass a `disabled()` checker to
    /// attach the seam without flagging (useful in tests).
    pub fn with_timing_checker(mut self, checker: Arc<crate::trace::TimingChecker>) -> Self {
        self.timing_checker = Some(checker);
        self
    }

    /// Set the session id this model serves, so traces attribute to the right
    /// session row. None for ad-hoc/test agents (traces still record, with a
    /// null session_id).
    pub fn with_session_id(mut self, session_id: Option<String>) -> Self {
        self.session_id = session_id;
        self
    }

    /// Record one LLM call to the trace sink (if attached). Centralizes
    /// `LlmTrace` construction so generate/stream stay readable; no-op when no
    /// sink is attached (tests / ad-hoc agents). B3: also runs the attached
    /// TimingChecker so a slow turn is flagged at warn (independent of whether
    /// a trace sink is attached — timing health surfaces either way).
    ///
    /// `ttfb_ms` = request-send → first response signal (None when the call
    /// never reached a first byte). `stream_ms` = first-byte → completion (None
    /// when there was no streaming/output phase).
    #[allow(clippy::too_many_arguments)]
    fn record_trace(
        &self,
        model: &str,
        status_code: Option<u16>,
        error_kind: Option<&str>,
        req_body: &str,
        resp_body: Option<&str>,
        latency_ms: u64,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
        ttfb_ms: Option<u64>,
        stream_ms: Option<u64>,
    ) {
        // B3: flag slow turns before persisting. Checked regardless of the sink
        // (timing health is observability any attached agent should get).
        if let Some(checker) = &self.timing_checker {
            if let Some(w) = checker.check(latency_ms, ttfb_ms) {
                log::warn!(
                    "[timing] {model} {}: {}",
                    w.kind,
                    w.message
                );
            }
        }
        if let Some(sink) = &self.trace_sink {
            sink.record_llm_call(
                self.session_id.as_deref(),
                LlmTrace {
                    model: model.to_string(),
                    base_url: self.base_url.clone(),
                    status_code,
                    error_kind: error_kind.map(str::to_string),
                    req_body: req_body.to_string(),
                    resp_body: resp_body.map(str::to_string),
                    latency_ms: Some(latency_ms),
                    input_tokens,
                    output_tokens,
                    ttfb_ms,
                    stream_ms,
                },
            );
        }
    }

    fn build_body(
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
        if !self.bound_tools.is_empty() {
            let tools: Vec<Value> = self
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
impl ChatModel for GlmChatModel {
    fn model_id(&self) -> &str {
        // The resolved id the provider handed back (after model_mapping). The
        // ReactAgent router reads this as the base model when the caller didn't
        // pass one in AgentInput.model, so a user who picked glm-5.2 is routed
        // against glm-5.2 — not the hardcoded STRONG_MODEL flagship. Without
        // this, the chat path (react_chat_driver builds AgentInput{model:None})
        // fell back to STRONG_MODEL (glm-4.6) and overwrote every GLM-family
        // turn's opts.model with it (session 7f51a5d2: 401, the user's Z.AI key
        // has no glm-4.6).
        &self.model
    }

    async fn generate(&self, messages: &[Message], opts: &ModelOptions) -> Result<Message, Error> {
        let model = opts.model.clone().unwrap_or_else(|| self.model.clone());
        // Circuit breaker: gate the call and record the outcome.
        if let Some(cb) = &self.circuit {
            if !cb.try_admit(&self.base_url) {
                return Err(Error::Model(format!(
                    "upstream circuit open: {}",
                    self.base_url
                )));
            }
        }
        let body = self.build_body(&model, messages, opts, false);
        let req_body = truncate(&body.to_string(), 32_000);
        let t0 = Instant::now();
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
                self.record_trace(
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
                if let Some(cb) = &self.circuit {
                    cb.record_failure(&self.base_url);
                }
                return Err(Error::Network(e.to_string()));
            }
        };
        // B3: headers received = first-byte for the non-stream path. TTFB is
        // the model "thinking" time (send → first response signal); the body
        // download (resp.json below) is the stream_ms phase.
        let t_first = Instant::now();
        let ttfb_ms = t_first.duration_since(t0).as_millis() as u64;
        let status = resp.status();
        if !status.is_success() {
            if should_failover(Some(status.as_u16()), false) {
                if let Some(cb) = &self.circuit {
                    cb.record_failure(&self.base_url);
                }
            } else if let Some(cb) = &self.circuit {
                // Non-failover 4xx (caller error) is neither success nor an
                // upstream failure: release the HalfOpen probe slot on_attempt
                // took. Without this, under half_open_max=1 a single 400 during
                // the probe wedges the circuit in HalfOpen (record_success is
                // skipped by the early return below, so half_open_inflight leaks).
                cb.record_probe_inconclusive(&self.base_url);
            }
            // Read the error body BEFORE it's dropped — this is the actual
            // reason (quota, schema, model-not-found) that was previously lost
            // to `format!("GLM stream failed: {status}")`.
            let err_body = redact_secrets(&resp.text().await.unwrap_or_default());
            log::warn!(
                "[llm] {} {} -> {}: {}",
                model,
                self.base_url,
                status,
                truncate(&err_body, 500)
            );
            self.record_trace(
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
            return Err(Error::Model(format!("GLM stream failed: {status}")));
        }
        let v: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                self.record_trace(
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
                if let Some(cb) = &self.circuit {
                    cb.record_failure(&self.base_url);
                }
                return Err(Error::Model(format!("decode: {e}")));
            }
        };
        if let Some(cb) = &self.circuit {
            cb.record_success(&self.base_url);
        }
        // Cost + trace: record token usage; cost is derived in the sink when 0.
        let usage = usage_from_response(&v);
        if let Some(sink) = &self.cost_sink {
            sink.record(&model, usage, 0.0);
        }
        // Trace: clean 2xx — store the raw response body (truncated) so the full
        // request↔response evidence is one query away. Industry norm is to record
        // success and failure symmetrically (see 2026-06-19 trace observability
        // research); the decoded message below is a separate concern.
        let resp_body = serde_json::to_string(&v).unwrap_or_default();
        self.record_trace(
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
            let model_name = opts.model.clone().unwrap_or_else(|| model_clone.model.clone());
            // Circuit breaker gate.
            if let Some(cb) = &model_clone.circuit {
                if !cb.try_admit(&model_clone.base_url) {
                    Err(Error::Model(format!("upstream circuit open: {}", model_clone.base_url)))?;
                }
            }
            let body = model_clone.build_body(&model_name, &messages, &opts, true);
            let req_body = truncate(&body.to_string(), 32_000);
            let t0 = Instant::now();
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
                    model_clone.record_trace(&model_name, None, Some("network"), &req_body, None, t0.elapsed().as_millis() as u64, None, None, None, None);
                    if let Some(cb) = &model_clone.circuit { cb.record_failure(&model_clone.base_url); }
                    Err(Error::Network(e.to_string()))?
                }
            };
            // B3: headers received = first-byte for the non_2xx branch. The
            // streaming branch re-stamps ttfb_at on the FIRST byte chunk (a
            // closer-to-true first-output signal than header receipt).
            let t_first = Instant::now();
            let status = resp.status();
            // 消费 resp:非 2xx 读 error body 再终止流;2xx 取字节流。两 arm 各自
            // move resp(互斥),用 match 而非 if + 块外 use——try_stream! 宏的 ? 让
            // 编译器无法证明 if 块必 return,块外 resp.bytes_stream() 会报
            // use-after-move(resp.text() 已 move resp)。match 把 resp 的消费收敛
            // 到一处,编译器一眼看到它被消费一次。
            use futures::StreamExt;
            let mut byte_stream = match status.is_success() {
                true => resp.bytes_stream(),
                false => {
                    if should_failover(Some(status.as_u16()), false) {
                        if let Some(cb) = &model_clone.circuit { cb.record_failure(&model_clone.base_url); }
                    } else if let Some(cb) = &model_clone.circuit {
                        // Non-failover 4xx: release the HalfOpen probe slot (see
                        // generate()'s matching branch) so a caller-error response
                        // doesn't wedge the breaker in HalfOpen under half_open_max=1.
                        cb.record_probe_inconclusive(&model_clone.base_url);
                    }
                    // Read the error body BEFORE it's dropped — same fix as generate().
                    let err_body = redact_secrets(&resp.text().await.unwrap_or_default());
                    log::warn!(
                        "[llm] {} {} -> {}: {}",
                        model_name, model_clone.base_url, status, truncate(&err_body, 500)
                    );
                    model_clone.record_trace(
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
                    Err(Error::Model(format!("GLM stream failed: {status}")))?;
                    unreachable!("non_2xx arm always returns via ? above")
                }
            };
            let mut buf = String::new();
            // Parallel accumulator for the raw SSE stream — unlike `buf` this is
            // never drained, so it holds the full wire response body for the
            // trace. Industry norm: record success responses verbatim, symmetric
            // with the error path (see 2026-06-19 trace observability research).
            // Capped at ~40 KB while accumulating so a long stream can't balloon
            // memory; the tail is already past the 32 KB trace cap anyway.
            let mut resp_body_buf = String::new();
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
            let mut usage = pricing::TokenUsage::default();
            // B3: stamp ttfb on the FIRST streamed byte chunk (the true
            // first-output signal for a streaming call). None until then.
            let mut ttfb_at: Option<Instant> = None;
            while let Some(chunk_res) = byte_stream.next().await {
                let bytes = match chunk_res {
                    Ok(b) => b,
                    Err(e) => {
                        model_clone.record_trace(&model_name, Some(status.as_u16()), Some("stream"), &req_body, None, t0.elapsed().as_millis() as u64, Some(usage.input), Some(usage.output), ttfb_at.map(|t| t.duration_since(t0).as_millis() as u64), ttfb_at.map(|t| t.elapsed().as_millis() as u64));
                        if let Some(cb) = &model_clone.circuit { cb.record_failure(&model_clone.base_url); }
                        Err(Error::Network(e.to_string()))?
                    }
                };
                // First successful chunk → record first-byte time (TTFB).
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
            if let Some(cb) = &model_clone.circuit { cb.record_success(&model_clone.base_url); }
            if let Some(sink) = &model_clone.cost_sink {
                sink.record(&model_name, usage, 0.0);
            }
            // Trace: clean 2xx — store the raw SSE stream (truncated) for full
            // request↔response evidence; symmetric with the error path (which
            // stores the error body). See 2026-06-19 trace observability research.
            // B3 timing: ttfb = send → first chunk; stream = first chunk → now.
            let ttfb_ms = ttfb_at.map(|t| t.duration_since(t0).as_millis() as u64);
            let stream_ms = ttfb_at.map(|t| t.elapsed().as_millis() as u64);
            model_clone.record_trace(&model_name, Some(status.as_u16()), None, &req_body, Some(&truncate(&resp_body_buf, 32_000)), t0.elapsed().as_millis() as u64, Some(usage.input), Some(usage.output), ttfb_ms, stream_ms);
        };
        Ok(Box::pin(s))
    }

    fn with_tools(&self, tools: &[ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
        let mut clone = self.clone();
        clone.bound_tools = tools.to_vec();
        Ok(Box::new(clone))
    }

    /// C2: fork this model with a counting cost sink wrapping the parent's DB
    /// sink, so a dispatched sub-agent's LLM calls are tallied into a per-
    /// dispatch accumulator the SubAgentTool reads after the child run — while
    /// still landing in cost_records (attribution preserved via the inner sink).
    /// The fork shares circuit/trace/timing/session_id (all Arc) with the parent,
    /// only the cost sink is swapped, so a fan-out's per-child cost is visible on
    /// the multi-agent board without losing the dashboard total.
    fn fork_with_counting_cost(
        &self,
    ) -> Option<(std::sync::Arc<dyn ChatModel>, std::sync::Arc<CostAccumulator>)> {
        let accumulator = std::sync::Arc::new(CostAccumulator::new());
        let counting = std::sync::Arc::new(crate::cost::sink::CountingCostSink::new(
            self.cost_sink.clone(),
            std::sync::Arc::clone(&accumulator),
        )) as std::sync::Arc<dyn crate::cost::sink::CostSink>;
        let forked = self.clone().with_cost_sink(counting);
        Some((std::sync::Arc::new(forked) as std::sync::Arc<dyn ChatModel>, accumulator))
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
/// `usage.input_tokens` (+ the prompt-cache tiers on real Anthropic);
/// `message_delta` carries the cumulative `usage.output_tokens` AND — on GLM —
/// the real `usage.input_tokens`. Standard Anthropic reports authoritative
/// input on message_start; GLM puts a 0 placeholder there and reports input on
/// message_delta. Reading BOTH fields on message_delta + the caller's
/// `saturating_add` yields the correct input for either provider (standard =
/// start_input + 0, GLM = 0 + delta_input) without double-counting.
///
/// B5: also reads `cache_read_input_tokens` / `cache_creation_input_tokens`
/// from message_start (these only appear there). GLM doesn't emit them → 0.
/// Non-usage / non-`data:` lines → None. Used to meter cost on the streaming
/// path.
fn parse_usage(line: &str) -> Option<pricing::TokenUsage> {
    let data = line.trim().strip_prefix("data: ")?;
    let ev: Value = serde_json::from_str(data).ok()?;
    match ev.get("type").and_then(|t| t.as_str())? {
        "message_start" => {
            let usage = ev.get("message").and_then(|m| m.get("usage"));
            let input = read_u32(usage, "input_tokens");
            // Cache tiers are reported once, on message_start (Anthropic).
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

/// Read an optional u64→u32 usage field from a JSON object (which may be null
/// or absent). Centralized so the two branches above stay readable.
fn read_u32(obj: Option<&Value>, key: &str) -> u32 {
    obj.and_then(|u| u.get(key))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32
}

/// Extract usage from a non-streaming Anthropic response
/// (`usage.input_tokens` / `usage.output_tokens` + cache tiers). Returns an
/// all-zero TokenUsage if no usage object is present — the sink still records
/// the call with a derived/zero cost.
fn usage_from_response(v: &Value) -> pricing::TokenUsage {
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
                        function: kernel_core::FunctionCall {
                            name,
                            arguments: args,
                        },
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

    /// Return a new registry holding only the read-only tools (v2.0 T2: the
    /// child agent dispatched by [`SubAgentTool`] gets the investigation tools
    /// but NOT the mutators — and not the dispatcher itself, which bounds
    /// recursion at depth 1: a child cannot dispatch a grandchild).
    pub fn read_only_subset(&self) -> ToolRegistry {
        ToolRegistry {
            tools: self
                .tools
                .iter()
                .filter(|t| t.is_read_only())
                .cloned()
                .collect(),
        }
    }

    /// Return a new registry holding only the tools whose name starts with one
    /// of the `allowed` prefixes (D1 `tools_allow`). A tool is kept iff SOME
    /// non-empty prefix is a prefix of its name; empty/blank prefixes are
    /// ignored (a `""` entry would otherwise match every name and silently
    /// defeat the allowlist). Callers gate on a non-empty `allowed` — passing
    /// `&[]` here keeps everything, matching "empty allowlist = inherit".
    ///
    /// This is the named-spec analogue of [`ToolRegistry::read_only_subset`]:
    /// that narrows by capability (read-only), this narrows by a declared
    /// name-prefix allowlist. Applied to a `read_only_subset` it's an
    /// intersection (read-only AND name-matching), so a child bound to
    /// `tools_allow: ["skill__web_search"]` gets only that tool even if the
    /// read-only set is larger.
    pub fn restrict_to_prefixes(&self, allowed: &[String]) -> ToolRegistry {
        let prefixes: Vec<&str> = allowed
            .iter()
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        if prefixes.is_empty() {
            return self.clone();
        }
        ToolRegistry {
            tools: self
                .tools
                .iter()
                .filter(|t| prefixes.iter().any(|p| t.info().name.starts_with(p)))
                .cloned()
                .collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// SubAgent dispatch tool (v2.0 T2)
// ---------------------------------------------------------------------------

/// A tool that dispatches a self-contained subtask to a child ReactAgent.
///
/// The parent delegates work to keep its own context lean; the child runs with
/// a FRESH history (so the parent's accumulated turns neither bleed into nor
/// overflow the child's window), a focused worker prompt, and a READ-ONLY tool
/// subset. Read-only only means the child can investigate but not mutate — and
/// cannot dispatch further subagents (`SubAgentTool` is itself not read-only,
/// so `read_only_subset` excludes it), bounding recursion at depth 1.
///
/// This is the structural complement to context auto-compaction (v1.3 C1):
/// compaction compresses ONE agent's history; subagent dispatch SPLITS work
/// across independent contexts. Both attack the long-task context-overflow
/// root cause — compaction from inside one run, dispatch across runs.
/// The anonymous worker prompt used when no `{subagent: name}` is given (or the
/// name doesn't match a loaded spec). Extracted so the named path can override
/// it without duplicating the text.
fn default_worker_prompt() -> &'static str {
    "你是子任务执行 agent。专注完成给定的单一子任务,给出简洁结论。\
     你只有只读工具(搜索/读取),不能修改文件、不能再派发子 agent。"
}

pub struct SubAgentTool {
    model: Arc<dyn ChatModel>,
    read_only_tools: ToolRegistry,
    max_steps: usize,
    /// Named sub-agent specs (D1). `{subagent: "name"}` matching one of these
    /// runs the child with that spec's system_prompt instead of
    /// [default_worker_prompt], so the agent can delegate to a specialist by
    /// name. Empty = anonymous-worker-only (the v2.0 T2 behavior).
    named: Vec<crate::kernel_impl::subagent_spec::SubAgentSpec>,
    /// C2/D3 subagent concurrency limiter. A parent that fans out multiple
    /// `dispatch_subagent` calls in ONE turn runs them concurrently (see
    /// [`ReactAgent`]'s `execute_call_set`); this Semaphore bounds how many
    /// child ReactAgents run at once, so a 10-way fan-out can't exhaust the
    /// model rate budget. `new` defaults to a wide permit count (tests stay
    /// unaffected); production injects a bounded handle via
    /// [`SubAgentTool::new_with_concurrency`].
    concurrency: Arc<Semaphore>,
}

impl SubAgentTool {
    /// `read_only_tools` should be the parent registry's read-only subset —
    /// pass `registry.read_only_subset()` so the child can't mutate or recurse.
    /// `named` are the loaded named sub-agent specs (empty = anonymous-only).
    ///
    /// Concurrency defaults to effectively unlimited — fine for unit tests,
    /// which don't fan out. Production wires a bounded Semaphore via
    /// [`SubAgentTool::new_with_concurrency`] from `build_react_agent`.
    pub fn new(
        model: Arc<dyn ChatModel>,
        read_only_tools: ToolRegistry,
        max_steps: usize,
        named: Vec<crate::kernel_impl::subagent_spec::SubAgentSpec>,
    ) -> Self {
        Self::new_with_concurrency(
            model,
            read_only_tools,
            max_steps,
            named,
            Arc::new(Semaphore::new(64)),
        )
    }

    /// Same as [`new`] but with an explicit subagent concurrency limiter. The
    /// Semaphore is `Arc`-shared, so multiple concurrent `dispatch_subagent`
    /// invocations in one turn contend on the SAME handle — that's the whole
    /// point of C2/D3: a parent fanning out N sub-tasks is capped at `permits`
    /// in-flight children, the rest queue on the permit.
    pub fn new_with_concurrency(
        model: Arc<dyn ChatModel>,
        read_only_tools: ToolRegistry,
        max_steps: usize,
        named: Vec<crate::kernel_impl::subagent_spec::SubAgentSpec>,
        concurrency: Arc<Semaphore>,
    ) -> Self {
        Self {
            model,
            read_only_tools,
            max_steps,
            named,
            concurrency,
        }
    }

    /// Build the child's tool registry for a dispatch. A non-empty `tools_allow`
    /// narrows the read-only subset to the matching name-prefixes (D1); an empty
    /// list inherits the full read-only subset (the anonymous-worker behaviour).
    /// Extracted from [`SubAgentTool::invoke`] so the D1 narrowing is
    /// unit-testable in isolation, without driving a model run. Warns when an
    /// explicit allowlist matches nothing — the child would then run toolless,
    /// which is almost certainly a spec typo, not intent.
    fn child_tool_registry(&self, tools_allow: &[String]) -> ToolRegistry {
        let restricted = self.read_only_tools.restrict_to_prefixes(tools_allow);
        // restrict_to_prefixes returns the full set when no non-empty prefix is
        // given (empty allowlist = inherit). Only warn when an EXPLICIT non-empty
        // allowlist still matched nothing — that's the typo case worth surfacing.
        let has_real_prefix = tools_allow.iter().any(|s| !s.is_empty());
        if has_real_prefix && restricted.is_empty() {
            log::warn!(
                "[subagent] tools_allow {tools_allow:?} matched no read-only tools; \
                 child runs toolless — likely a spec typo"
            );
        }
        restricted
    }
}

/// Format the C2 per-dispatch cost footer appended to a dispatch_subagent
/// result. Empty (no footer) when the tally is `None` (the model can't fork —
/// test/ad-hoc models) or all-zero (the child made no tracked LLM calls). The
/// exact `📊 子 agent 用量: A→B tok · $C` shape is the wire contract the frontend
/// `extractDispatches` regex parses, so it's a pure fn to unit-test in isolation.
fn format_cost_line(tally: Option<CostTally>) -> String {
    match tally {
        Some(t) if t.input_tokens + t.output_tokens > 0 => format!(
            "\n\n📊 子 agent 用量: {}→{} tok · ${:.4}",
            t.input_tokens, t.output_tokens, t.cost_usd
        ),
        _ => String::new(),
    }
}

#[async_trait]
impl Tool for SubAgentTool {
    fn info(&self) -> ToolInfo {
        // List named sub-agents (if any) so the model knows WHO it can delegate
        // to by name — without this the {subagent: "name"} parameter is useless.
        let named_list = if self.named.is_empty() {
            String::from("(无命名子 agent — 不传 subagent 则派给匿名 worker)")
        } else {
            self.named
                .iter()
                .map(|s| format!("- {}: {}", s.name, s.description))
                .collect::<Vec<_>>()
                .join("\n")
        };
        ToolInfo {
            name: "dispatch_subagent".into(),
            description: format!(
                "把一个独立、自包含的子任务派给子 agent 执行并返回结论。用于拆分长任务、隔离上下文：子 agent 拥有全新历史与只读工具,不可改文件、不可再派发。可选 {{subagent: name}} 指定命名子 agent(用其专用 system_prompt),不指定或名称不匹配则派给匿名 worker。可用命名子 agent:\n{named_list}"
            ),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "派给子 agent 的自包含子任务" },
                    "subagent": { "type": "string", "description": "可选:命名子 agent 名称(见 description 列表);不指定则匿名 worker" }
                },
                "required": ["task"]
            }),
        }
    }

    async fn invoke(&self, args: &str, ctx: &ToolContext) -> Result<String, Error> {
        let parsed = serde_json::from_str::<serde_json::Value>(args).ok();
        let task = parsed
            .as_ref()
            .and_then(|v| v.get("task").and_then(|t| t.as_str()).map(str::to_owned))
            .ok_or_else(|| Error::Agent("dispatch_subagent 需要参数 {task: string}".into()))?;
        if task.trim().is_empty() {
            return Err(Error::Agent("dispatch_subagent 的 task 不能为空".into()));
        }
        let requested = parsed.as_ref().and_then(|v| {
            v.get("subagent")
                .and_then(|s| s.as_str())
                .map(str::to_owned)
        });
        // Resolve the matched named spec (system_prompt + tools_allow). A
        // matching name whose system_prompt is blank — or an unknown name —
        // degrades to the anonymous worker, so a typo never stalls the dispatch.
        // Clone BOTH owned fields in one pass so no borrow of self.named is held
        // across the awaited run_loop (an async borrow of self across an await
        // point is rejected by the borrow checker).
        let (worker_prompt, tools_allow): (String, Vec<String>) = requested
            .as_ref()
            .and_then(|name| self.named.iter().find(|s| &s.name == name))
            .filter(|s| !s.system_prompt.trim().is_empty())
            .map(|s| (s.system_prompt.clone(), s.tools_allow.clone()))
            .unwrap_or_else(|| (default_worker_prompt().to_string(), Vec::new()));
        // D1 tools_allow enforcement: a named spec may narrow the child's tools
        // to a name-prefix allowlist (e.g. only skill__web_search + read_file).
        // An anonymous worker, or a spec with an empty list, inherits the full
        // read-only subset.
        let child_tools = self.child_tool_registry(&tools_allow);
        // C2: fork the model with a per-dispatch counting cost sink when the
        // model supports it (production GlmChatModel), so this child's LLM cost
        // is tallied into an accumulator we read after the run and append to the
        // tool result — the per-dispatch cost visibility the multi-agent board
        // surfaces. Test/ad-hoc models return None and run cost-blind (unchanged).
        let (child_model, accumulator) = match self.model.fork_with_counting_cost() {
            Some((m, acc)) => (m, Some(acc)),
            None => (Arc::clone(&self.model), None),
        };
        let child =
            ReactAgent::new_shared(child_model, child_tools, worker_prompt.as_str())
                .with_context(ctx.clone())
                .with_max_steps(self.max_steps);
        // "Model half" of sub-agent dispatch — 机器科层制: 贵模型只在裁决节点,
        // 不干杂活。fork_with_counting_cost 只换成本计数器、不改 model id,所以
        // 此前子 agent 每一轮都克隆父的旗舰模型。这里按任务类型提前分类,给子
        // agent 挂对应逐轮 router,让劳动轮跑 glm-4-flash:
        //   CheapOnly(明确杂活:搜索/读取/抽取) → 全程 flash
        //   Routed(含推理关键词或歧义)         → 挂 route_step(首轮 strong + 回声轮 cheap)
        // 非 glm-4.6 子 agent(glm-5.2/claude/deepseek)返回 None → 不挂 router,
        // 子 agent 用自身模型均匀跑,与 executor.rs wire-time 的 is_glm_family 守门
        // 及 route_step 自身 base guard 对称,规避把 GLM id 灌进异端点的 400
        // (executor.rs:454-460)。裁决仍在 main:降档错了产出弱结论会被 main 抓回重派。
        let child = match crate::kernel_impl::model_router::dispatch_tier_for(
            self.model.model_id(),
            &task,
        ) {
            Some(crate::kernel_impl::model_router::DispatchTier::CheapOnly) => child
                .with_model_router(Arc::new(
                    crate::kernel_impl::model_router::force_cheap_router,
                )),
            Some(crate::kernel_impl::model_router::DispatchTier::Routed) => child
                .with_model_router(Arc::new(crate::kernel_impl::model_router::route_step)),
            None => child,
        };
        // C2/D3: hold a concurrency permit for the whole child run so a parent
        // that fans out multiple dispatch_subagent calls in one turn is bounded.
        // The Semaphore is Arc-shared across concurrent invocations, so the Nth
        // in-flight child blocks here until an earlier one finishes — acquired
        // before run_loop and dropped (`_permit` scope) exactly when it returns.
        let _permit = self
            .concurrency
            .acquire()
            .await
            .expect("subagent concurrency semaphore should never be closed");
        match child.run_loop(&task, ModelOptions::default()).await {
            Ok(out) => {
                let cost_line = format_cost_line(
                    accumulator.as_deref().map(kernel_core::CostAccumulator::tally),
                );
                Ok(format!("[子 agent 结论] {out}{cost_line}"))
            }
            Err(e) => {
                // Surface the failure as a tool result, not an error, so the
                // parent can adapt (retry differently / do it inline) instead
                // of aborting its whole run on one bad subtask.
                log::warn!("[subagent] dispatch failed for task '{task}': {e}");
                Ok(format!("[子 agent 失败: {e}]"))
            }
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Tool execution for run/stream turns (C2/D3 subagent concurrency)
// ---------------------------------------------------------------------------

/// One tool call's outcome within a run/stream turn. Extracted from the
/// stream's per-call block so the C2/D3 concurrency path can collect outcomes
/// without driving the model, and so the result/event shape is unit-testable
/// in isolation. `events` holds the Succeeded/Failed ToolCallEvent(s) the
/// stream yields AFTER the call's Started event (Started is the caller's job,
/// emitted before any execution).
#[derive(Debug, Clone)]
struct CallOutcome {
    call_id: String,
    result: String,
    events: Vec<kernel_core::ToolCallEvent>,
    file_changed: Option<std::path::PathBuf>,
}

/// Execute a single tool call (before-hook → invoke → outcome events). The
/// extracted body of the run/stream per-call loop so it can run concurrently
/// for dispatch_subagent without re-driving the model. Pure of yield: RETURNS
/// events (Started is the caller's job); the stream re-yields them in order.
async fn execute_one_call(
    tools: &ToolRegistry,
    call: &kernel_core::ToolCall,
    ctx: &ToolContext,
    hooks: &Option<Arc<HookManager>>,
) -> CallOutcome {
    let mut events: Vec<kernel_core::ToolCallEvent> = Vec::new();
    // Classify once: the before-hook uses it for the veto, and — on a
    // successful write — we re-match it below to emit a per-write FileChanged.
    let action = crate::kernel_impl::hooks::classify_action(
        &call.function.name,
        &call.function.arguments,
    );
    let blocked = if let Some(h) = hooks.as_ref() {
        match h.before(&action).await {
            Err(reason) => {
                let blocked_msg = format!("[blocked by {}: {}]", reason.hook, reason.message);
                events.push(kernel_core::ToolCallEvent {
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
    let (result, file_changed) = match blocked {
        Some(b) => (b, None),
        None => match tools.find(&call.function.name) {
            Some(t) => match t.invoke(&call.function.arguments, ctx).await {
                Ok(out) => {
                    events.push(kernel_core::ToolCallEvent {
                        tool: call.function.name.clone(),
                        arguments: call.function.arguments.clone(),
                        status: kernel_core::ToolCallStatus::Succeeded,
                        result: Some(out.clone()),
                    });
                    let fc = match &action {
                        crate::kernel_impl::hooks::Action::WriteFile { path, .. } => {
                            Some(std::path::PathBuf::from(path))
                        }
                        _ => None,
                    };
                    (out, fc)
                }
                Err(e) => {
                    let err = format!("[tool error: {e}]");
                    events.push(kernel_core::ToolCallEvent {
                        tool: call.function.name.clone(),
                        arguments: call.function.arguments.clone(),
                        status: kernel_core::ToolCallStatus::Failed,
                        result: Some(err.clone()),
                    });
                    (err, None)
                }
            },
            None => (format!("[unknown tool: {}]", call.function.name), None),
        },
    };
    CallOutcome {
        call_id: call.id.clone(),
        result,
        events,
        file_changed,
    }
}

/// Execute every tool call in a turn, returning outcomes in ORIGINAL call
/// order (tool_result blocks must pair with tool_use by id — Anthropic). The
/// C2/D3 concurrency path: when ≥2 calls are dispatch_subagent, those fan out
/// concurrently (bounded by SubAgentTool's Arc-shared Semaphore, acquired
/// inside each invoke); every other call stays serial so AssertionGuard's
/// git-diff capture around writes stays sound. With ≤1 dispatch_subagent this
/// is plain serial — zero behavioural change vs the old inline loop.
async fn execute_call_set(
    tools: &ToolRegistry,
    calls: &[kernel_core::ToolCall],
    ctx: &ToolContext,
    hooks: &Option<Arc<HookManager>>,
) -> Vec<CallOutcome> {
    let dispatch_positions: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter(|(_, c)| c.function.name == "dispatch_subagent")
        .map(|(i, _)| i)
        .collect();
    let mut outcomes: Vec<Option<CallOutcome>> = (0..calls.len()).map(|_| None).collect();
    if dispatch_positions.len() > 1 {
        // Fan out the dispatch_subagent calls concurrently. Each holds a permit
        // from SubAgentTool's Semaphore for the whole child run, so the parent
        // is capped at `permits` in-flight children regardless of fan-out width.
        let dispatch_calls: Vec<(usize, kernel_core::ToolCall)> = dispatch_positions
            .iter()
            .map(|&i| (i, calls[i].clone()))
            .collect();
        let futs = dispatch_calls.iter().map(|(i, c)| async move {
            let o = execute_one_call(tools, c, ctx, hooks).await;
            (*i, o)
        });
        for (i, o) in futures::future::join_all(futs).await {
            outcomes[i] = Some(o);
        }
        // Run the remaining (non-dispatch) calls serially — writes included, so
        // AssertionGuard sees a clean pre/post git-diff window per write.
        for (i, call) in calls.iter().enumerate() {
            if outcomes[i].is_none() {
                outcomes[i] = Some(execute_one_call(tools, call, ctx, hooks).await);
            }
        }
    } else {
        for (i, call) in calls.iter().enumerate() {
            outcomes[i] = Some(execute_one_call(tools, call, ctx, hooks).await);
        }
    }
    outcomes
        .into_iter()
        .map(|o| o.expect("every call position is filled in both branches"))
        .collect()
}

// ---------------------------------------------------------------------------
// Subagent status contract (C2/D3) — deer-flow subagent_status_contract.json
// ---------------------------------------------------------------------------

/// Terminal status of a dispatched sub-agent's tool result. Mirrors deer-flow's
/// cross-language `subagent_status` contract (completed / failed / cancelled /
/// timed_out / polling_timed_out) so the frontend board can color a dispatch by
/// outcome regardless of which agent family produced the text. Parsed from the
/// tool-result prefix by [`parse_subagent_status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    PollingTimedOut,
}

/// Parse a dispatch_subagent tool result into its terminal status. Recognizes
/// BOTH this project's own prefixes (`[子 agent 结论]` / `[子 agent 失败 …]`,
/// produced by [`SubAgentTool::invoke`]) AND deer-flow's `Task Succeeded` /
/// `Task failed` / `Task cancelled` / `Task timed out` / `Task polling timed
/// out` prefixes (so a future claude `task` tool output maps to the same enum).
/// Returns `None` for non-terminal streaming fragments ("Investigating …") —
/// the deer-flow contract marks those `expected_status: null`.
pub fn parse_subagent_status(content: &str) -> Option<SubagentStatus> {
    let trimmed = content.trim();
    // This project's dispatch_subagent prefixes (SubAgentTool::invoke).
    if trimmed.starts_with("[子 agent 结论]") {
        return Some(SubagentStatus::Completed);
    }
    if trimmed.starts_with("[子 agent 失败") {
        return Some(SubagentStatus::Failed);
    }
    // deer-flow Task-tool prefixes (subagent_status_contract.json cases).
    // `polling timed out` MUST be checked before `timed out` (more specific).
    if trimmed.starts_with("Task Succeeded") {
        return Some(SubagentStatus::Completed);
    }
    if trimmed.starts_with("Task polling timed out") {
        return Some(SubagentStatus::PollingTimedOut);
    }
    if trimmed.starts_with("Task timed out") {
        return Some(SubagentStatus::TimedOut);
    }
    if trimmed.starts_with("Task cancelled") {
        return Some(SubagentStatus::Cancelled);
    }
    if trimmed.starts_with("Task failed") {
        return Some(SubagentStatus::Failed);
    }
    None
}

// ---------------------------------------------------------------------------
// ReactAgent
// ---------------------------------------------------------------------------

pub struct ReactAgent {
    model: Arc<dyn ChatModel>,
    tools: ToolRegistry,
    hooks: Option<Arc<HookManager>>,
    max_steps: usize,
    /// Max self-verify attempts (v1.2 T7): after convergence, run an honesty
    /// audit (cargo check + assertion weakening); on failure, feed findings
    /// back and let the agent self-repair, up to this many times. 0 = off.
    max_verify: usize,
    /// Injectable audit fn (tests stub it; production leaves None → uses
    /// honesty::audit_project). Signature matches audit_project.
    audit_fn: Option<AuditFn>,
    /// Per-step model router (v1.2 T9). If set, before each `stream` call the
    /// loop asks it `(&history, base_model) -> model_id` and overrides
    /// `opts.model`. Same-provider routing (glm-4.6 ↔ glm-4-flash), so
    /// endpoint/key stay constant. None = single fixed model (the old behavior).
    model_router: Option<ModelRouterFn>,
    /// Cost budget hard-limit check (v1.2 T10). If set, called at the top of
    /// every turn; returning true halts the run gracefully
    /// (`FatalReason::Budget`) before spending another LLM call. None = unlimited.
    budget_check: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// Context auto-compaction threshold (v1.3 C1). When set, each turn first
    /// estimates the history's token count; if it exceeds this, the middle
    /// turns are summarized into one message (system + summary + recent tail).
    /// None = never compact (unbounded growth, the old behavior).
    max_context_tokens: Option<usize>,
    /// How many recent turns to keep verbatim when compacting. Defaults to 6
    /// (~3 full user/assistant/tool rounds) so the model still sees the live
    /// tool results it's reacting to.
    compact_keep_recent: usize,
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
        // Delegate so the field-init lives in one place (new_shared).
        Self::new_shared(Arc::new(model), tools, system_prompt)
    }

    /// Build from an already-shared model handle (v2.0 T2): subagent dispatch
    /// reuses the parent's `Arc<dyn ChatModel>` instead of re-wrapping an owned
    /// model. Same defaults as [`new`].
    pub fn new_shared(
        model: Arc<dyn ChatModel>,
        tools: ToolRegistry,
        system_prompt: impl Into<String>,
    ) -> Self {
        Self {
            model,
            tools,
            hooks: None,
            max_steps: 12,
            max_verify: 0,
            audit_fn: None,
            model_router: None,
            budget_check: None,
            max_context_tokens: None,
            compact_keep_recent: 6,
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

    /// Enable post-convergence self-verification (v1.2 T7). On each convergence
    /// up to `n` times, run the honesty audit; failure feeds findings back and
    /// the agent self-repairs on the next loop iteration. 0 (default) = off.
    pub fn with_max_verify(mut self, n: usize) -> Self {
        self.max_verify = n;
        self
    }

    /// Inject a custom audit function (tests). Production leaves this unset so
    /// the agent uses `honesty::audit_project`.
    pub fn with_audit_fn(mut self, f: AuditFn) -> Self {
        self.audit_fn = Some(f);
        self
    }

    /// Enable per-step model routing (v1.2 T9). Before each turn, the router is
    /// called with the current history + base model and its return value
    /// overrides `opts.model` for that turn. Production wires
    /// [`crate::kernel_impl::model_router::route_step`] (rule-based glm-4-flash
    /// for low-stakes turns); tests inject a stub.
    pub fn with_model_router(mut self, f: ModelRouterFn) -> Self {
        self.model_router = Some(f);
        self
    }

    /// Enable the cost-budget hard limit (v1.2 T10). The closure is called at
    /// the top of each turn; if it returns true the run halts gracefully with a
    /// `FatalReason::Budget` message instead of making another LLM call.
    /// Production wires `cost::agentfare::is_budget_exhausted` over the DB.
    pub fn with_budget_check(mut self, f: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        self.budget_check = Some(f);
        self
    }

    /// Enable context auto-compaction (v1.3 C1). When the history's estimated
    /// token count exceeds `max_tokens`, the middle turns are summarized into
    /// one message, keeping `keep_recent` recent turns verbatim. Summarization
    /// runs on the raw (tool-less) model so it can't fire tool calls, and a
    /// summarizer failure is swallowed (skips that round) to avoid data loss.
    pub fn with_context_compaction(mut self, max_tokens: usize, keep_recent: usize) -> Self {
        self.max_context_tokens = Some(max_tokens);
        self.compact_keep_recent = keep_recent;
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
        // D2 lifecycle: dispatch UserPromptSubmit BEFORE the user message enters
        // history; any contexts the user hooks return are appended to the prompt
        // as additional context (claude-code additionalContext injection). A
        // missing HookManager (no hooks) skips straight to the plain prompt.
        let mut full_task = task.to_string();
        if let Some(hooks) = &self.hooks {
            // D2 lifecycle: dispatch UserPromptSubmit BEFORE the user message
            // enters history. Ok(ctxs) → stdout injected as additional context
            // (claude-code additionalContext). Err → v2 exit-2 block: a user hook
            // refused the prompt; don't enter the turn, return the reason so the
            // user sees why their prompt was refused.
            match hooks
                .dispatch_event(&crate::kernel_impl::hooks::HookEvent::UserPromptSubmit {
                    prompt: task.to_string(),
                })
                .await
            {
                Ok(ctxs) if !ctxs.is_empty() => {
                    full_task.push_str("\n\n[user-hook context]\n");
                    full_task.push_str(&ctxs.join("\n---\n"));
                }
                Ok(_) => {}
                Err(reason) => {
                    return Ok(format!(
                        "[用户钩子阻止本轮提交 · {}] {}",
                        reason.hook, reason.message
                    ));
                }
            }
        }
        history.push(Message::user(&full_task));
        let result: Result<String, Error> = async {
            for _step in 0..self.max_steps {
                let mut resp = model.generate(&history, &opts).await?;
                // B6 tool-call-repair (generate path) — same plain-text
                // promotion as the streaming run() path. run_loop is the entry
                // used by sub-agents (dispatch_subagent), so weak-model
                // plain-text tool calls must be repaired here too, not only in
                // the streaming chat path.
                if resp.tool_calls.is_empty() && !resp.content.is_empty() {
                    let allowlist: Vec<String> =
                        self.tools.infos().iter().map(|t| t.name.clone()).collect();
                    if let Some(repaired) =
                        crate::kernel_impl::tool_call_repair::repair_plain_text_tool_calls(
                            &resp.content,
                            Some(&allowlist),
                        )
                    {
                        log::info!(
                            "[ReactAgent/run_loop] repaired {} leaked plain-text tool call(s)",
                            repaired.len()
                        );
                        resp.tool_calls = repaired;
                    }
                }
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
        .await;
        // D2 lifecycle: dispatch Stop once on run termination (converged or
        // step-limited), regardless of outcome. Stop hooks run for side effects
        // (notifications); their output is ignored by the manager.
        if let Some(hooks) = &self.hooks {
            let summary = match &result {
                Ok(s) => s.clone(),
                Err(e) => e.to_string(),
            };
            // Stop dispatch is best-effort: a hook's exit-2 cannot "un-stop" a
            // run, so the Err is intentionally dropped (the run already ended).
            let _ = hooks
                .dispatch_event(&crate::kernel_impl::hooks::HookEvent::Stop { summary })
                .await;
        }
        result
    }

    /// Collect the list of files changed since the last commit (uncommitted working
    /// tree changes). Best-effort — returns an empty vec on failure.
    fn git_changed_files(working_dir: &Option<String>) -> Vec<String> {
        let Some(dir) = working_dir.as_deref() else {
            return Vec::new();
        };
        let mut cmd = std::process::Command::new("git");
        cmd.args(["diff", "--name-only"]).current_dir(dir);
        // CREATE_NO_WINDOW — 本函数在 AgentEvent::Done(Completed) 时调用（即
        // 对话完成的瞬间），缺这个标志 Windows 会为 git.exe 分配一个新控制台
        // 窗口，闪一下黑框。与 git.rs/honesty.rs/pty.rs 保持一致。
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let Ok(out) = cmd.output() else {
            return Vec::new();
        };
        if !out.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect()
    }

    /// Capture `git diff` in the working directory so the AssertionGuard can scan
    /// write_file outcomes for assertion weakening. Best-effort — a missing git
    /// repo or spawn failure returns None (no diff → no weakening scan).
    fn capture_git_diff(working_dir: &Option<String>) -> Option<String> {
        let dir = working_dir.as_deref()?;
        let mut cmd = std::process::Command::new("git");
        cmd.args(["diff", "--no-color"]).current_dir(dir);
        // CREATE_NO_WINDOW — 本函数在每次 WriteFile 工具调用前后触发，缺标志
        // 会闪 git 黑框。与 git.rs/honesty.rs/pty.rs 保持一致。
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let out = cmd.output().ok()?;
        if out.status.success() {
            Some(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            None
        }
    }

    async fn execute_tool_call(&self, call: &kernel_core::ToolCall, ctx: &ToolContext) -> String {
        // Classify the tool name+args into an Action variant so Plan mode can
        // block writes/commands and AssertionGuard can scan diffs. Previously
        // every tool was Action::CallTool, making WriteFile/RunCommand dead
        // paths and the associated guards (Plan, Assertion, Task) empty shells.
        let action = crate::kernel_impl::hooks::classify_action(
            &call.function.name,
            &call.function.arguments,
        );
        // Capture a pre-write diff so the post-hook can detect assertion weakening
        // even when the diff is cumulative across several writes in one turn.
        let pre_diff = if matches!(&action, crate::kernel_impl::hooks::Action::WriteFile { .. }) {
            Self::capture_git_diff(&ctx.working_dir)
        } else {
            None
        };

        if let Some(hooks) = &self.hooks {
            if let Err(reason) = hooks.before(&action).await {
                return format!("[blocked by {}: {}]", reason.hook, reason.message);
            }
            // v2 PreToolUse user-hook dispatch: a user hook (exit 2) refusing
            // this tool call short-circuits before the tool runs — the block
            // reason becomes the tool result, mirroring the built-in gate above.
            // (claude-code PreToolUse semantics.)
            if let Err(reason) = hooks
                .dispatch_event(&crate::kernel_impl::hooks::HookEvent::PreToolUse {
                    tool: call.function.name.clone(),
                    arguments: call.function.arguments.clone(),
                })
                .await
            {
                return format!("[blocked by {}: {}]", reason.hook, reason.message);
            }
        }
        // v2.0 C6: dry-run simulation. In DryRun mode the gate lets every action
        // through (DryRun blocks nothing); HERE is where side-effecting tools are
        // intercepted and return a simulated result instead of landing. Read-only
        // tools run for real so the agent plans against actual file contents /
        // search hits — a dry-run that couldn't read the project is useless.
        if self
            .hooks
            .as_ref()
            .map(|h| h.mode().is_dry_run())
            .unwrap_or(false)
        {
            if let Some(tool) = self.tools.find(&call.function.name) {
                if !tool.is_read_only() {
                    let preview: String = call.function.arguments.chars().take(200).collect();
                    return format!(
                        "[dry-run] 预演未执行 {}({preview}) — 此为预览，切换真实模式以落地改动",
                        call.function.name
                    );
                }
            }
        }
        let mut result = match self.tools.find(&call.function.name) {
            Some(t) => t
                .invoke(&call.function.arguments, ctx)
                .await
                .unwrap_or_else(|e| format!("[tool error: {e}]")),
            None => format!("[unknown tool: {}]", call.function.name),
        };
        // v2 PostToolUse user-hook dispatch: observation only — the tool already
        // ran, so a hook's exit-2 is logged inside the hook and dropped here
        // (never blocks retroactively). Best-effort.
        if let Some(hooks) = &self.hooks {
            let _ = hooks
                .dispatch_event(&crate::kernel_impl::hooks::HookEvent::PostToolUse {
                    tool: call.function.name.clone(),
                    arguments: call.function.arguments.clone(),
                    result: result.clone(),
                })
                .await;
        }
        if let Some(hooks) = &self.hooks {
            let post_diff =
                if matches!(&action, crate::kernel_impl::hooks::Action::WriteFile { .. }) {
                    Self::capture_git_diff(&ctx.working_dir).or(pre_diff)
                } else {
                    None
                };
            let outcome = crate::kernel_impl::hooks::ActionOutcome {
                action,
                ok: !result.starts_with("[tool error"),
                diff: post_diff,
                error: if result.starts_with('[') {
                    Some(result.clone())
                } else {
                    None
                },
            };
            let findings = hooks.after(&outcome).await;
            if !findings.is_empty() {
                // Surface the finding to the MODEL, not just the operator log.
                // The honesty moat only works if the agent SEES its own weakening
                // and self-corrects; a log line the model never reads changes
                // nothing. Appended to the tool result so it lands in the next
                // tool_result block the model consumes.
                let banner = findings
                    .iter()
                    .map(|f| format!("  - [{}] {} (evidence: {})", f.rule, f.explanation, f.evidence))
                    .collect::<Vec<_>>()
                    .join("\n");
                log::warn!("[hook] assertion-weakening detected:\n{banner}");
                result = format!(
                    "{result}\n\n--- HonestyGuard: assertion weakening detected ---\n{banner}\nA test assertion was weakened (t.Fatal->t.Log, assert!->debug_assert!, unwrap->unwrap_or, #[ignore]/t.Skip added, etc.). This is dishonest. Restore the strong assertion; a weakened test will fail the honesty audit."
                );
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

    fn run(
        &self,
        input: AgentInput,
    ) -> Result<BoxStream<'static, Result<AgentEvent, kernel_core::Error>>, kernel_core::Error>
    {
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
        let max_verify = self.max_verify;
        let audit_fn = self.audit_fn.clone();
        let model_router = self.model_router.clone();
        let budget_check = self.budget_check.clone();
        let max_context_tokens = self.max_context_tokens;
        let compact_keep_recent = self.compact_keep_recent;

        let s = async_stream::try_stream! {
            let infos = tools.infos();
            let bound: Arc<dyn ChatModel> = if infos.is_empty() {
                Arc::clone(&model)
            } else {
                match model.with_tools(&infos) {
                    Ok(b) => Arc::from(b),
                    Err(e) => {
                        log::warn!("[ReactAgent] with_tools failed in stream, no tools: {e}");
                        Arc::clone(&model)
                    }
                }
            };
            let mut history = Vec::with_capacity(2 + prior_history.len());
            history.push(Message::system(&system_prompt));
            history.extend(prior_history.iter().cloned());
            // D2 lifecycle: dispatch UserPromptSubmit BEFORE the user message
            // enters history; user-hook stdout (exit 0) is appended to the prompt
            // as additional context (claude-code additionalContext injection).
            let mut full_task = task.clone();
            if let Some(h) = hooks.as_ref() {
                // Ok(ctxs) → inject stdout as context. Err → v2 exit-2 block: a
                // user hook refused the prompt; end the stream with the block
                // reason (no turn entered, no model call).
                match h
                    .dispatch_event(&crate::kernel_impl::hooks::HookEvent::UserPromptSubmit {
                        prompt: task.clone(),
                    })
                    .await
                {
                    Ok(ctxs) if !ctxs.is_empty() => {
                        full_task.push_str("\n\n[user-hook context]\n");
                        full_task.push_str(&ctxs.join("\n---\n"));
                    }
                    Ok(_) => {}
                    Err(reason) => {
                        let msg = format!(
                            "[用户钩子阻止本轮提交 · {}] {}",
                            reason.hook, reason.message
                        );
                        yield AgentEvent::Token(msg.clone());
                        yield AgentEvent::Done(AgentOutcome {
                            status: AgentRunStatus::Completed,
                            files_changed: Vec::new(),
                            exit_code: Some(0),
                            output_summary: Some(msg),
                            honesty: None,
                        });
                        return;
                    }
                }
            }
            history.push(Message::user(&full_task));
            // T9: base model for per-step routing. opts.model is overridden each
            // turn when a router is wired; base_model is the "no routing" default
            // (also what route_step falls back to when the turn is high-stakes).
            // Fall back to the ChatModel's OWN resolved id (the model the user
            // picked + the provider resolved), NOT a hardcoded flagship. The chat
            // path builds AgentInput{model:None} (the resolved id already lives
            // inside GlmChatModel), so a blanket STRONG_MODEL fallback routed
            // every GLM-family turn against glm-4.6 — picking glm-5.2 then sent
            // glm-4.6 (session 7f51a5d2, 2026-06-21: 401, the user's Z.AI key has
            // no glm-4.6). A ChatModel that doesn't expose an id (test stubs)
            // returns "" → keep the legacy STRONG_MODEL fallback there.
            let base_model = model_opt.clone().unwrap_or_else(|| {
                let mid = model.model_id();
                if mid.is_empty() {
                    crate::kernel_impl::model_router::STRONG_MODEL.to_string()
                } else {
                    mid.to_string()
                }
            });
            let mut opts = ModelOptions { model: model_opt, thinking, ..Default::default() };
            let mut final_output = String::new();
            // C7: track why the loop ended so the terminal Done is honest —
            // converged (model gave a final answer), degraded (unrecoverable
            // LLM error → graceful message), or neither (hit max_steps).
            let mut converged = false;
            let mut degraded: Option<FatalReason> = None;
            // T7 self-verify: how many audit-and-feed-back cycles have run.
            let mut verify_count = 0u32;
            // D1(b): consecutive summarizer failures this run. Feed to maybe_compact
            // so compaction suspends (not infinite-retries) after repeated errors.
            let mut compact_consecutive_failures = 0u32;

            for _step in 0..max_steps {
                // T10 hard budget limit: halt before spending another turn if the
                // monthly budget is already exhausted. Fires on turn 0 too, so a
                // run that starts over-budget never makes an LLM call.
                if budget_check.as_ref().map(|c| c()).unwrap_or(false) {
                    degraded = Some(FatalReason::Budget);
                    break;
                }
                // T9 per-step routing: ask the router (if wired) which model fits
                // this turn given the conversation so far, and override opts.model
                // for this single stream call. Same provider → endpoint/key are
                // constant; only the model id in the request body changes.
                if let Some(router) = model_router.as_ref() {
                    opts.model = Some(router(&history, &base_model));
                }
                // v1.3 C1: if the history has grown past the compaction
                // threshold, compress its middle into one summary message
                // before this turn's LLM call. Summarization uses the RAW model
                // (no tools bound) so it can't fire tool calls; a summarizer
                // error is swallowed (skip this round, retry next turn) rather
                // than truncating and losing information mid-run.
                if let Some(max_tok) = max_context_tokens {
                    let _ = crate::kernel_impl::context_compact::maybe_compact(
                        &mut history,
                        model.as_ref(),
                        &opts,
                        max_tok,
                        compact_keep_recent,
                        &mut compact_consecutive_failures,
                    )
                    .await;
                }
                // Real streaming: consume the model's SSE stream, yielding each
                // text delta as a Token (chat renders token-by-token) while the
                // stream() helper accumulates tool_calls from content_block_start
                // + input_json_delta events. Text + tool_calls are reassembled
                // into one assistant Message for coherent next-turn history.
                use futures::StreamExt;
                // C7 tool-call recovery: retry transient LLM send failures
                // (network/5xx/429) with exponential backoff; fatal errors
                // (circuit open / quota / auth / 4xx) degrade at once. The
                // breaker inside GlmChatModel records each attempt, so a run
                // of retries naturally trips the circuit.
                let mut attempt = 1u32;
                let turn_stream = loop {
                    match bound.stream(&history, &opts) {
                        Ok(s) => break Ok(s),
                        Err(e) => {
                            let err = e;
                            if should_retry(&err, attempt) {
                                log::warn!(
                                    "[ReactAgent] transient LLM error, retry {}/{}: {}",
                                    attempt,
                                    MAX_ATTEMPTS,
                                    err
                                );
                                tokio::time::sleep(retry_delay(attempt)).await;
                                attempt += 1;
                                continue;
                            }
                            break Err(match classify_llm_error(&err) {
                                LlmErrorKind::Fatal(r) => r,
                                LlmErrorKind::Retryable => FatalReason::Generic,
                            });
                        }
                    }
                };
                let mut turn_stream = match turn_stream {
                    Ok(s) => s,
                    Err(reason) => {
                        degraded = Some(reason);
                        break;
                    }
                };
                let mut turn_text = String::new();
                let mut turn_reasoning = String::new();
                let mut turn_tool_calls: Vec<kernel_core::ToolCall> = Vec::new();
                let mut turn_sig: Option<String> = None;
                while let Some(msg_res) = turn_stream.next().await {
                    let msg = match msg_res {
                        Ok(m) => m,
                        Err(e) => {
                            // Mid-stream drop: tokens already emitted, can't
                            // cleanly retry the partial turn → degrade.
                            let err = e;
                            degraded = Some(match classify_llm_error(&err) {
                                LlmErrorKind::Fatal(r) => r,
                                LlmErrorKind::Retryable => FatalReason::Generic,
                            });
                            break;
                        }
                    };
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
                if degraded.is_some() {
                    break;
                }
                // B6 tool-call-repair: weak models (GLM / DeepSeek) sometimes
                // leak a tool call as plain text (`[name]{...}`, `<function=...>`,
                // Harmony `commentary to=... code {...}`) instead of a structured
                // tool_use block. When the turn produced no structured tool_calls,
                // scan the assembled text and promote any leaked calls so the loop
                // executes them instead of terminating on a half-finished action.
                // The allowlist restricts promotion to advertised tool names so a
                // prompt-injected plain-text call cannot invoke an unknown tool.
                if turn_tool_calls.is_empty() && !turn_text.is_empty() {
                    // `infos` is the owned ToolInfo vec cloned at the top of the
                    // stream (this block must be 'static — it cannot borrow self).
                    let allowlist: Vec<String> =
                        infos.iter().map(|t| t.name.clone()).collect();
                    if let Some(repaired) =
                        crate::kernel_impl::tool_call_repair::repair_plain_text_tool_calls(
                            &turn_text,
                            Some(&allowlist),
                        )
                    {
                        log::info!(
                            "[ReactAgent] repaired {} leaked plain-text tool call(s) into structured tool_use",
                            repaired.len()
                        );
                        turn_tool_calls = repaired;
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
                    // T7 self-verify gate: after convergence, run the honesty
                    // audit (cargo check + assertion weakening). On failure,
                    // feed the findings back as a user turn so the agent
                    // self-repairs on the next loop iteration (bounded by
                    // max_verify). spawn_blocking keeps the blocking cargo
                    // check off the async stream driver.
                    if (verify_count as usize) < max_verify {
                        if let Some(pp) = ctx.working_dir.as_ref() {
                            let pp_path = std::path::PathBuf::from(pp);
                            let claim = final_output.clone();
                            let audit_fn_clone = audit_fn.clone();
                            let audit_val = tokio::task::spawn_blocking(move || {
                                match audit_fn_clone {
                                    Some(f) => f(&pp_path, &claim),
                                    None => crate::kernel_impl::honesty::audit_project(
                                        &pp_path, &claim,
                                    ),
                                }
                            })
                            .await
                            // Fail-closed on every default. A panicked audit task
                            // OR a malformed/missing `status` field must NOT be
                            // treated as a pass — the whole point of the honesty
                            // audit is to catch assertion-weakening, so defaulting
                            // to "passed" (the old behavior) silently bypassed it.
                            // audit_project returns status="passed" only when zero
                            // Error-severity findings surface; anything else fails.
                            .unwrap_or_else(|_| serde_json::json!({
                                "status": "failed",
                                "findings": "audit task panicked — treat as failure",
                            }));
                            let passed = audit_val
                                .get("status")
                                .and_then(|s| s.as_str())
                                .map(|s| s == "passed")
                                .unwrap_or(false);
                            if !passed {
                                verify_count += 1;
                                let findings = audit_val
                                    .get("findings")
                                    .map(|f| f.to_string())
                                    .unwrap_or_else(|| audit_val.to_string());
                                history.push(Message {
                                    role: Role::User,
                                    content: format!(
                                        "自验证发现问题（cargo check / 断言弱化），请修复后重新完成：\n{findings}"
                                    ),
                                    tool_calls: Vec::new(),
                                    tool_call_id: None,
                                    reasoning: None,
                                    reasoning_signature: None,
                                });
                                // Don't set converged: continue the for-loop so
                                // the next iteration re-streams with the fed-back
                                // user turn now appended to history. (A `break`
                                // here would wrongly terminate the run — there is
                                // no enclosing stream-consumption loop at this
                                // point; the inner while already ended.)
                                continue;
                            }
                        }
                    }
                    converged = true;
                    yield AgentEvent::TurnBoundary;
                    break;
                }
                // C2/D3 subagent concurrency: when ≥2 calls this turn are
                // dispatch_subagent, fan them out concurrently (bounded by
                // SubAgentTool's Semaphore); the rest stay serial. Outcomes are
                // emitted + appended to history in ORIGINAL call order so
                // tool_result blocks pair with tool_use by id (Anthropic).
                let dispatch_count = turn_tool_calls
                    .iter()
                    .filter(|c| c.function.name == "dispatch_subagent")
                    .count();
                if dispatch_count > 1 {
                    // Concurrent path: emit Started for all calls up front, then
                    // run dispatch_subagent calls concurrently + the rest serially
                    // (see execute_call_set). Result events arrive after the whole
                    // set settles, in call order — the subagent board thus sees
                    // all dispatches start together and finish as permits release.
                    for call in &turn_tool_calls {
                        yield AgentEvent::ToolCall(kernel_core::ToolCallEvent {
                            tool: call.function.name.clone(),
                            arguments: call.function.arguments.clone(),
                            status: kernel_core::ToolCallStatus::Started,
                            result: None,
                        });
                    }
                    let outcomes = execute_call_set(&tools, &turn_tool_calls, &ctx, &hooks).await;
                    for o in &outcomes {
                        for ev in &o.events {
                            yield AgentEvent::ToolCall(ev.clone());
                        }
                        if let Some(p) = &o.file_changed {
                            yield AgentEvent::FileChanged(p.clone());
                        }
                        history.push(Message {
                            role: Role::Tool,
                            content: o.result.clone(),
                            tool_calls: Vec::new(),
                            tool_call_id: Some(o.call_id.clone()),
                            reasoning: None,
                            reasoning_signature: None,
                        });
                    }
                } else {
                    // Serial path (≤1 dispatch_subagent): Started→result per call,
                    // preserving the legacy interleaved event order — zero regression.
                    for call in &turn_tool_calls {
                        yield AgentEvent::ToolCall(kernel_core::ToolCallEvent {
                            tool: call.function.name.clone(),
                            arguments: call.function.arguments.clone(),
                            status: kernel_core::ToolCallStatus::Started,
                            result: None,
                        });
                        let o = execute_one_call(&tools, call, &ctx, &hooks).await;
                        for ev in &o.events {
                            yield AgentEvent::ToolCall(ev.clone());
                        }
                        if let Some(p) = &o.file_changed {
                            yield AgentEvent::FileChanged(p.clone());
                        }
                        history.push(Message {
                            role: Role::Tool,
                            content: o.result.clone(),
                            tool_calls: Vec::new(),
                            tool_call_id: Some(o.call_id.clone()),
                            reasoning: None,
                            reasoning_signature: None,
                        });
                    }
                }
            }
            // D2 lifecycle: dispatch Stop ONCE at run termination, regardless of
            // terminal status (degraded / max-steps / completed). Stop hooks run
            // for side effects (notifications, cleanup); the summary reflects how
            // the run ended so a hook can branch on success vs failure.
            if let Some(h) = hooks.as_ref() {
                let summary = if let Some(reason) = &degraded {
                    fatal_user_message(*reason).to_string()
                } else if !converged {
                    format!(
                        "Reached the {max_steps}-step tool-call limit without a final answer.",
                    )
                } else {
                    final_output.clone()
                };
                // Best-effort: a Stop hook's exit-2 cannot un-stop the run, so
                // drop the Err (the run already ended).
                let _ = h
                    .dispatch_event(&crate::kernel_impl::hooks::HookEvent::Stop { summary })
                    .await;
            }
            // Honest terminal status: degraded (graceful LLM failure), max-steps
            // (no convergence), or completed (model gave a final answer). Never
            // report Completed when the run actually failed.
            if let Some(reason) = degraded {
                yield AgentEvent::Done(AgentOutcome {
                    status: AgentRunStatus::Failed,
                    files_changed: Vec::new(),
                    exit_code: Some(1),
                    output_summary: Some(fatal_user_message(reason).to_string()),
                    honesty: None,
                });
            } else if !converged {
                yield AgentEvent::Done(AgentOutcome {
                    status: AgentRunStatus::Failed,
                    files_changed: Vec::new(),
                    exit_code: Some(1),
                    output_summary: Some(format!(
                        "Reached the {max_steps}-step tool-call limit without a final answer.",
                    )),
                    honesty: None,
                });
            } else {
                yield AgentEvent::Done(AgentOutcome {
                    status: AgentRunStatus::Completed,
                    files_changed: Self::git_changed_files(&ctx.working_dir),
                    exit_code: Some(0),
                    output_summary: Some(final_output),
                    // Transparent agent: honesty is enforced at the call level via
                    // HookManager (each tool invocation inspectable before commit),
                    // not via post-hoc diff audit. OpaqueAgent fills this instead.
                    honesty: None,
                });
            }
        };
        Ok(Box::pin(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_core::ToolInfo;

    #[test]
    fn subagent_status_parses_project_prefixes() {
        // SubAgentTool::invoke stamps these prefixes on its tool result.
        assert_eq!(
            parse_subagent_status("[子 agent 结论] 调研完成,产出 3 页报告"),
            Some(SubagentStatus::Completed)
        );
        assert_eq!(
            parse_subagent_status("[子 agent 失败: model returned 400]"),
            Some(SubagentStatus::Failed)
        );
    }

    #[test]
    fn format_cost_line_renders_tally_in_the_wire_shape() {
        // The exact "📊 子 agent 用量: A→B tok · $C" shape is the contract the
        // frontend extractDispatches regex parses — guard it so a format drift
        // surfaces here, not as a silent blank board.
        let line = format_cost_line(Some(CostTally {
            input_tokens: 1234,
            output_tokens: 567,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: 0.0123,
        }));
        assert!(line.contains("1234→567 tok"), "token split rendered: {line}");
        assert!(line.contains("$0.0123"), "cost rendered 4dp: {line}");
    }

    #[test]
    fn format_cost_line_suppresses_when_no_tracked_calls() {
        // None (model can't fork) or all-zero tally (child made no LLM call) →
        // empty string, so the board shows no spurious "0→0 tok · $0.0000".
        assert_eq!(format_cost_line(None), "");
        assert_eq!(
            format_cost_line(Some(CostTally::default())),
            "",
            "all-zero tally suppressed"
        );
    }

    #[test]
    fn subagent_status_parses_deerflow_contract_cases() {
        // deer-flow contracts/subagent_status_contract.json — both sides must
        // agree on every case. These literals come straight from that fixture.
        assert_eq!(
            parse_subagent_status(
                "Task Succeeded. Result: investigated and produced a 3-page report"
            ),
            Some(SubagentStatus::Completed)
        );
        assert_eq!(
            parse_subagent_status("Task failed. Error: underlying tool raised RuntimeError"),
            Some(SubagentStatus::Failed)
        );
        assert_eq!(
            parse_subagent_status("Task cancelled by user."),
            Some(SubagentStatus::Cancelled)
        );
        assert_eq!(
            parse_subagent_status("Task timed out. Error: 900 seconds"),
            Some(SubagentStatus::TimedOut)
        );
        assert_eq!(
            parse_subagent_status(
                "Task polling timed out after 15 minutes. This may indicate the background task is stuck. Status: RUNNING"
            ),
            Some(SubagentStatus::PollingTimedOut)
        );
        assert_eq!(
            parse_subagent_status("Task polling timed out after 1 minutes. Status: RUNNING"),
            Some(SubagentStatus::PollingTimedOut)
        );
        // Non-terminal streaming fragment → None (contract: expected_status null).
        assert_eq!(parse_subagent_status("Investigating ..."), None);
        // Whitespace tolerance (streaming prepends/appends newlines).
        assert_eq!(
            parse_subagent_status("  Task Succeeded. Result: ok  "),
            Some(SubagentStatus::Completed)
        );
        assert_eq!(
            parse_subagent_status("  Task cancelled by user.\n"),
            Some(SubagentStatus::Cancelled)
        );
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Test ChatModel that records how many generate() calls overlap in time.
    /// Each call sleeps 40ms while holding an in-flight counter; `max_seen` is
    /// the high-water mark. run_loop calls generate once per child, so max_seen
    /// == number of children that ran simultaneously. Used to PROVE the C2/D3
    /// Semaphore actually caps concurrency — not just that the code compiles.
    struct ConcurrentModel {
        in_flight: Arc<AtomicUsize>,
        max_seen: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl ChatModel for ConcurrentModel {
        async fn generate(&self, _: &[Message], _: &ModelOptions) -> Result<Message, Error> {
            let cur = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_seen.fetch_max(cur, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(Message {
                role: Role::Assistant,
                content: "done".into(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                reasoning: None,
                reasoning_signature: None,
            })
        }
        fn stream(&self, _: &[Message], _: &ModelOptions) -> Result<MessageStream, Error> {
            // run_loop uses generate, not stream; this model is generate-only.
            Err(Error::Unsupported("ConcurrentModel is generate-only".into()))
        }
        fn with_tools(&self, _: &[ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
            Ok(Box::new(ConcurrentModel {
                in_flight: Arc::clone(&self.in_flight),
                max_seen: Arc::clone(&self.max_seen),
            }))
        }
    }

    #[tokio::test]
    async fn execute_call_set_fans_out_dispatch_subagents() {
        // Semaphore(4) is wide enough that all 3 fan-out children run at once.
        // max in-flight generate must reach 3 — proving execute_call_set ran
        // them concurrently (the old serial loop would peak at 1).
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let model: Arc<dyn ChatModel> = Arc::new(ConcurrentModel {
            in_flight: Arc::clone(&in_flight),
            max_seen: Arc::clone(&max_seen),
        });
        let tool = SubAgentTool::new_with_concurrency(
            Arc::clone(&model),
            ToolRegistry::new(),
            4,
            Vec::new(),
            Arc::new(Semaphore::new(4)),
        );
        let mut reg = ToolRegistry::new();
        reg.push(tool);
        let calls = vec![
            probe_call("dispatch_subagent", r#"{"task":"a"}"#),
            probe_call("dispatch_subagent", r#"{"task":"b"}"#),
            probe_call("dispatch_subagent", r#"{"task":"c"}"#),
        ];
        let outcomes = execute_call_set(&reg, &calls, &ToolContext::default(), &None).await;
        assert_eq!(outcomes.len(), 3);
        assert_eq!(
            max_seen.load(Ordering::SeqCst),
            3,
            "3 fan-out children ran concurrently under Semaphore(4)"
        );
        // Outcomes preserve ORIGINAL call order — tool_result must pair with
        // tool_use by id (Anthropic), regardless of completion order.
        assert_eq!(outcomes[0].call_id, calls[0].id);
        assert_eq!(outcomes[1].call_id, calls[1].id);
        assert_eq!(outcomes[2].call_id, calls[2].id);
        // Each dispatched child converged → Completed status on its result.
        for o in &outcomes {
            assert_eq!(parse_subagent_status(&o.result), Some(SubagentStatus::Completed));
        }
    }

    #[tokio::test]
    async fn execute_call_set_semaphore_caps_concurrency() {
        // Semaphore(1) serializes the children even though execute_call_set
        // fans them out concurrently — the acquire inside SubAgentTool::invoke
        // is the gate. max in-flight generate must be 1, not 3.
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let model: Arc<dyn ChatModel> = Arc::new(ConcurrentModel {
            in_flight: Arc::clone(&in_flight),
            max_seen: Arc::clone(&max_seen),
        });
        let tool = SubAgentTool::new_with_concurrency(
            Arc::clone(&model),
            ToolRegistry::new(),
            4,
            Vec::new(),
            Arc::new(Semaphore::new(1)),
        );
        let mut reg = ToolRegistry::new();
        reg.push(tool);
        let calls = vec![
            probe_call("dispatch_subagent", r#"{"task":"a"}"#),
            probe_call("dispatch_subagent", r#"{"task":"b"}"#),
            probe_call("dispatch_subagent", r#"{"task":"c"}"#),
        ];
        let outcomes = execute_call_set(&reg, &calls, &ToolContext::default(), &None).await;
        assert_eq!(outcomes.len(), 3);
        assert_eq!(
            max_seen.load(Ordering::SeqCst),
            1,
            "Semaphore(1) serialized the fan-out children"
        );
    }

    #[tokio::test]
    async fn execute_call_set_serial_when_at_most_one_dispatch() {
        // ≤1 dispatch_subagent → serial branch (zero behavioural change). A
        // read-only echo tool returns its arguments; outcomes stay in call order.
        struct EchoTool;
        #[async_trait]
        impl Tool for EchoTool {
            fn info(&self) -> ToolInfo {
                ToolInfo {
                    name: "echo".into(),
                    description: "echo args".into(),
                    parameters_schema: serde_json::json!({"type": "object"}),
                }
            }
            async fn invoke(&self, args: &str, _: &ToolContext) -> Result<String, Error> {
                Ok(args.to_string())
            }
            fn is_read_only(&self) -> bool {
                true
            }
        }
        let mut reg = ToolRegistry::new();
        reg.push(EchoTool);
        let calls = vec![
            probe_call("echo", r#"{"v":"1"}"#),
            probe_call("echo", r#"{"v":"2"}"#),
        ];
        let outcomes = execute_call_set(&reg, &calls, &ToolContext::default(), &None).await;
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].call_id, calls[0].id);
        assert_eq!(outcomes[1].call_id, calls[1].id);
        // Each succeeded with one Succeeded event carrying the echoed args.
        assert_eq!(outcomes[0].events.len(), 1);
        assert_eq!(outcomes[0].events[0].status, kernel_core::ToolCallStatus::Succeeded);
    }

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
    fn git_changed_files_and_capture_diff_reflect_working_tree() {
        // 回归 guard:git_changed_files / capture_git_diff 是 assertion-weakening
        // 检测链(PostToolUse hooks 读 diff 判弱化)与 Done(Completed) 的
        // files_changed 的关键依赖,此前零覆盖。CREATE_NO_WINDOW 重构(Windows
        // 加 creation_flags)只改窗口行为、不改契约——此测试覆盖契约本身,确保
        // 重构没破坏函数:在真实 git repo 里制造一个未暂存修改,两个函数必须各自
        // 看到它。(窗口抑制行为本身属 OS 层,不可单测。)
        use std::process::Command;
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let g = |args: &[&str]| {
            let r = Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                r.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&r.stderr)
            );
        };
        g(&["init"]);
        g(&["config", "user.email", "t@t"]);
        g(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        g(&["add", "."]);
        g(&["commit", "-m", "init"]);
        // 制造未暂存修改 → git diff --name-only / --no-color 都应看到 a.txt
        std::fs::write(dir.join("a.txt"), "two").unwrap();
        let wd: Option<String> = Some(dir.to_string_lossy().into_owned());
        let changed = ReactAgent::git_changed_files(&wd);
        assert!(
            changed.iter().any(|f| f.ends_with("a.txt")),
            "git_changed_files 应含 a.txt: {:?}",
            changed
        );
        let diff =
            ReactAgent::capture_git_diff(&wd).expect("有 diff 时 capture_git_diff 返回 Some");
        assert!(diff.contains("a.txt"), "diff 应含 a.txt: {}", diff);
        // working_dir=None 早退,不调 git、不 panic
        assert!(ReactAgent::git_changed_files(&None).is_empty());
        assert!(ReactAgent::capture_git_diff(&None).is_none());
    }

    #[test]
    fn build_body_injects_bound_tools() {
        let mut model = GlmChatModel::bigmodel("k", "glm-4.6");
        model.bound_tools = vec![ToolInfo {
            name: "grep".into(),
            description: "search".into(),
            parameters_schema: json!({"type": "object"}),
        }];
        let body = model.build_body(
            "glm-4.6",
            &[Message::user("hi")],
            &ModelOptions::default(),
            false,
        );
        assert_eq!(body["tools"][0]["name"], "grep");
    }

    #[test]
    fn build_body_omits_tools_when_empty() {
        let model = GlmChatModel::bigmodel("k", "glm-4.6");
        let body = model.build_body(
            "glm-4.6",
            &[Message::user("hi")],
            &ModelOptions::default(),
            false,
        );
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn with_tools_returns_bound_clone() {
        let model = GlmChatModel::bigmodel("k", "glm-4.6");
        let _bound = model
            .with_tools(&[ToolInfo {
                name: "x".into(),
                description: "y".into(),
                parameters_schema: json!({}),
            }])
            .unwrap();
        let body_orig =
            model.build_body("m", &[Message::user("a")], &ModelOptions::default(), false);
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

    // --- v2.0 C6: dry-run execution-mode simulation ---

    use std::sync::Mutex;

    /// Tool that records every real `invoke` and reports a configurable
    /// read-only flag — lets a test prove dry-run simulates side effects while
    /// letting read-only tools run for real.
    struct ProbeTool {
        name: &'static str,
        read_only: bool,
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl Tool for ProbeTool {
        fn info(&self) -> ToolInfo {
            ToolInfo {
                name: self.name.into(),
                description: "probe".into(),
                parameters_schema: json!({"type":"object"}),
            }
        }
        async fn invoke(&self, args: &str, _ctx: &ToolContext) -> Result<String, Error> {
            self.calls.lock().unwrap().push(args.to_string());
            Ok(format!("invoked:{}:{}", self.name, args))
        }
        fn is_read_only(&self) -> bool {
            self.read_only
        }
    }

    fn probe_call(name: &str, args: &str) -> kernel_core::ToolCall {
        kernel_core::ToolCall {
            id: "c1".into(),
            call_type: "function".into(),
            function: kernel_core::FunctionCall {
                name: name.into(),
                arguments: args.into(),
            },
        }
    }

    /// A read-only ProbeTool with the given name and a throwaway call log — for
    /// registry-narrowing tests that only care about WHICH tools survive.
    fn read_only_probe(name: &'static str) -> ProbeTool {
        ProbeTool {
            name,
            read_only: true,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Two-phase model for the B6 repair test: turn 1 emits an assistant message
    /// carrying `first` as plain text with NO structured tool_calls (mirroring a
    /// weak model leaking a tool call as prose); turn 2 returns "done" to let the
    /// loop converge after the repaired call executes.
    #[derive(Clone)]
    struct TwoPhaseModel {
        first: String,
        turns: Arc<std::sync::Mutex<usize>>,
    }

    #[async_trait]
    impl ChatModel for TwoPhaseModel {
        async fn generate(&self, _: &[Message], _: &ModelOptions) -> Result<Message, Error> {
            let mut n = self.turns.lock().unwrap();
            *n += 1;
            let content = if *n == 1 {
                self.first.clone()
            } else {
                "done".to_string()
            };
            Ok(Message::assistant(content))
        }
        fn stream(&self, _: &[Message], _: &ModelOptions) -> Result<MessageStream, Error> {
            Err(Error::Unsupported("TwoPhaseModel: drive via generate".into()))
        }
        fn with_tools(&self, _: &[ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
            Ok(Box::new(self.clone()))
        }
    }

    #[tokio::test]
    async fn run_loop_repairs_leaked_plain_text_tool_call() {
        // B6: a weak model (GLM/DeepSeek) leaks the tool call as plain text
        // (content = `[probe]{...}`, tool_calls empty). The run_loop generate
        // path must repair it into a structured tool_call so `probe` is invoked
        // — not treat the leaked prose as the final answer and terminate early.
        let leaked = "[probe]\n{\"k\":\"v\"}\n[END_TOOL_REQUEST]";
        let model = TwoPhaseModel {
            first: leaked.into(),
            turns: Arc::new(std::sync::Mutex::new(0)),
        };
        let probe_calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let probe = ProbeTool {
            name: "probe",
            read_only: true,
            calls: probe_calls.clone(),
        };
        let agent = ReactAgent::new(model, ToolRegistry::new().with(probe), "sys");
        let out = agent
            .run_loop("do the thing", ModelOptions::default())
            .await;
        assert!(out.is_ok(), "run_loop should converge after repair: {out:?}");
        let calls = probe_calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "probe invoked exactly once after plain-text repair: {calls:?}"
        );
        assert!(
            calls[0].contains("\"k\":\"v\""),
            "repair preserved the JSON arguments verbatim: {}",
            calls[0]
        );
    }

    #[tokio::test]
    async fn dry_run_simulates_side_effects_runs_read_only_for_real() {
        let write_calls = Arc::new(Mutex::new(Vec::new()));
        let read_calls = Arc::new(Mutex::new(Vec::new()));
        let mut reg = ToolRegistry::new();
        reg.push(ProbeTool {
            name: "write_file",
            read_only: false,
            calls: write_calls.clone(),
        });
        reg.push(ProbeTool {
            name: "read_file",
            read_only: true,
            calls: read_calls.clone(),
        });

        let hooks = Arc::new(
            crate::kernel_impl::hooks::HookManager::new()
                .with_mode(crate::kernel_impl::hooks::PermissionMode::DryRun),
        );
        // execute_tool_call never drives the model, but ReactAgent::new needs one.
        let agent = ReactAgent::new(ScriptedModel::new(vec![]), reg, "sys").with_hooks(hooks);
        let ctx = ToolContext::default();

        let r1 = agent
            .execute_tool_call(&probe_call("write_file", r#"{"path":"a.rs"}"#), &ctx)
            .await;
        assert!(
            r1.contains("[dry-run]"),
            "side-effect tool simulated, got: {r1}"
        );
        assert!(
            write_calls.lock().unwrap().is_empty(),
            "write must NOT land in dry-run"
        );

        let r2 = agent
            .execute_tool_call(&probe_call("read_file", r#"{"path":"a.rs"}"#), &ctx)
            .await;
        assert!(
            !r2.contains("[dry-run]"),
            "read-only tool runs for real, got: {r2}"
        );
        assert_eq!(
            read_calls.lock().unwrap().len(),
            1,
            "read-only tool invoked once"
        );

        // Unknown tool in dry-run: find() is None so it falls through to the
        // stable "[unknown tool]" path — dry-run never invents execution.
        let r3 = agent
            .execute_tool_call(&probe_call("nope", "{}"), &ctx)
            .await;
        assert!(
            r3.contains("[unknown tool: nope]"),
            "unknown tool path unchanged: {r3}"
        );
    }

    #[tokio::test]
    async fn real_mode_lands_side_effecting_tool() {
        // Regression guard: the dry-run branch must NOT fire outside DryRun.
        let write_calls = Arc::new(Mutex::new(Vec::new()));
        let mut reg = ToolRegistry::new();
        reg.push(ProbeTool {
            name: "write_file",
            read_only: false,
            calls: write_calls.clone(),
        });

        let hooks = Arc::new(
            crate::kernel_impl::hooks::HookManager::new()
                .with_mode(crate::kernel_impl::hooks::PermissionMode::Default),
        );
        let agent = ReactAgent::new(ScriptedModel::new(vec![]), reg, "sys").with_hooks(hooks);
        let ctx = ToolContext::default();

        let r = agent
            .execute_tool_call(&probe_call("write_file", r#"{"path":"a.rs"}"#), &ctx)
            .await;
        assert!(!r.contains("[dry-run]"), "real mode must NOT simulate: {r}");
        assert_eq!(
            write_calls.lock().unwrap().len(),
            1,
            "write landed once in real mode"
        );
    }

    // --- v2.0 T2: subagent dispatch ---

    /// ChatModel whose `generate` returns a fixed assistant reply and whose
    /// `with_tools` returns a clone — lets a test drive a child ReactAgent's
    /// run_loop (which uses generate, not stream) with no real endpoint.
    #[derive(Clone)]
    struct GenModel {
        reply: String,
    }

    impl GenModel {
        fn new(reply: &str) -> Self {
            Self {
                reply: reply.to_string(),
            }
        }
    }

    #[async_trait]
    impl ChatModel for GenModel {
        async fn generate(&self, _: &[Message], _: &ModelOptions) -> Result<Message, Error> {
            Ok(Message::assistant(self.reply.clone()))
        }
        fn stream(&self, _: &[Message], _: &ModelOptions) -> Result<MessageStream, Error> {
            Err(Error::Unsupported("GenModel: drive via generate".into()))
        }
        fn with_tools(&self, _: &[ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
            Ok(Box::new(self.clone()))
        }
    }

    fn shared_gen_model(reply: &str) -> Arc<dyn ChatModel> {
        Arc::new(GenModel::new(reply))
    }

    #[tokio::test]
    async fn subagent_dispatches_child_and_returns_conclusion() {
        // Child has no tools → run_loop calls generate directly and returns the
        // reply as the final answer, which SubAgentTool wraps for the parent.
        let tool = SubAgentTool::new(
            shared_gen_model("子任务结论：找到 3 处"),
            ToolRegistry::new(),
            6,
            Vec::new(),
        );
        let out = tool
            .invoke(r#"{"task":"分析依赖"}"#, &ToolContext::default())
            .await
            .unwrap();
        assert!(out.contains("[子 agent 结论]"), "conclusion wrapped: {out}");
        assert!(out.contains("子任务结论"), "child answer surfaced: {out}");
    }

    #[tokio::test]
    async fn subagent_rejects_malformed_or_empty_task() {
        let tool = SubAgentTool::new(shared_gen_model("x"), ToolRegistry::new(), 6, Vec::new());
        let ctx = ToolContext::default();
        assert!(
            tool.invoke("not json", &ctx).await.is_err(),
            "non-JSON rejected"
        );
        assert!(
            tool.invoke(r#"{}"#, &ctx).await.is_err(),
            "missing task rejected"
        );
        assert!(
            tool.invoke(r#"{"task":""}"#, &ctx).await.is_err(),
            "empty task rejected"
        );
        assert!(
            tool.invoke(r#"{"task":"   "}"#, &ctx).await.is_err(),
            "blank task rejected"
        );
    }

    #[test]
    fn read_only_subset_keeps_readonly_drops_mutators_and_dispatcher() {
        // The child must get investigation tools only — mutators AND the
        // dispatcher itself are excluded, so it can't mutate or recurse.
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut reg = ToolRegistry::new();
        reg.push(ProbeTool {
            name: "read_file",
            read_only: true,
            calls: calls.clone(),
        });
        reg.push(ProbeTool {
            name: "write_file",
            read_only: false,
            calls,
        });
        reg.push(SubAgentTool::new(
            shared_gen_model("x"),
            ToolRegistry::new(),
            4,
            Vec::new(),
        ));

        let ro = reg.read_only_subset();
        assert_eq!(ro.len(), 1, "only the read-only tool survives");
        assert!(ro.find("read_file").is_some());
        assert!(ro.find("write_file").is_none(), "mutator dropped");
        assert!(
            ro.find("dispatch_subagent").is_none(),
            "dispatcher dropped → recursion bounded"
        );
    }

    #[tokio::test]
    async fn subagent_failure_surfaces_as_result_not_error() {
        // A child that errors must NOT propagate the error — the parent gets a
        // "[子 agent 失败]" tool result so it can adapt instead of aborting.
        struct FailModel;
        #[async_trait]
        impl ChatModel for FailModel {
            async fn generate(&self, _: &[Message], _: &ModelOptions) -> Result<Message, Error> {
                Err(Error::Unsupported("boom".into()))
            }
            fn stream(&self, _: &[Message], _: &ModelOptions) -> Result<MessageStream, Error> {
                Err(Error::Unsupported("boom".into()))
            }
            fn with_tools(&self, _: &[ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
                Ok(Box::new(FailModel))
            }
        }
        let tool = SubAgentTool::new(Arc::new(FailModel), ToolRegistry::new(), 4, Vec::new());
        let out = tool
            .invoke(r#"{"task":"x"}"#, &ToolContext::default())
            .await
            .unwrap();
        // Failure branch returns "[子 agent 失败: {e}]" — note ':' (it carries
        // the cause), NOT ']' like the success prefix.
        assert!(
            out.contains("[子 agent 失败:"),
            "failure surfaced, not propagated: {out}"
        );
    }

    #[test]
    fn info_lists_named_subagents_when_present() {
        // D1: when named specs are loaded, the tool's description must surface
        // their names so the model knows WHO it can delegate to by name, and the
        // schema must expose the {subagent} parameter. Empty named (the other
        // tests above) keeps the legacy anonymous-only description.
        use crate::kernel_impl::subagent_spec::SubAgentSpec;
        let spec = SubAgentSpec {
            name: "researcher".into(),
            description: "deep research".into(),
            system_prompt: "你是调研员".into(),
            tools_allow: vec![],
        };
        let tool = SubAgentTool::new(shared_gen_model("x"), ToolRegistry::new(), 4, vec![spec]);
        let info = tool.info();
        assert!(
            info.description.contains("researcher"),
            "named agent listed: {}",
            info.description
        );
        assert!(
            info.description.contains("deep research"),
            "description carried: {}",
            info.description
        );
        let props = info
            .parameters_schema
            .get("properties")
            .expect("schema has properties");
        assert!(
            props.get("subagent").is_some(),
            "{{subagent}} parameter present"
        );
    }

    #[tokio::test]
    async fn subagent_named_dispatch_runs_with_named_spec() {
        // {subagent: "researcher"} matching a loaded spec → the child runs under
        // the spec's system_prompt (overriding the anonymous worker). With a
        // no-tool child the run returns the model's reply; we assert the named
        // path SUCCEEDS and wraps the conclusion — not that the prompt text
        // reached the model (the GenModel mock ignores prompts, so that would be
        // a tautology against the mock, not against real behavior).
        use crate::kernel_impl::subagent_spec::SubAgentSpec;
        let spec = SubAgentSpec {
            name: "researcher".into(),
            description: "deep research".into(),
            system_prompt: "你是资深调研员,只给结论与依据".into(),
            tools_allow: vec![],
        };
        let tool = SubAgentTool::new(
            shared_gen_model("结论：ok"),
            ToolRegistry::new(),
            4,
            vec![spec],
        );
        let out = tool
            .invoke(
                r#"{"task":"调研X","subagent":"researcher"}"#,
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(
            out.contains("[子 agent 结论]"),
            "named dispatch wraps conclusion: {out}"
        );
        assert!(
            out.contains("结论：ok"),
            "named child answer surfaced: {out}"
        );
    }

    #[tokio::test]
    async fn subagent_unknown_name_falls_back_to_anonymous_worker() {
        // An unknown {subagent} name is NOT an error — it degrades to the
        // anonymous worker so the dispatch still succeeds (the agent never gets
        // stuck on a typo'd subagent name). named=[] here, so any name misses.
        let tool = SubAgentTool::new(
            shared_gen_model("结论：anon"),
            ToolRegistry::new(),
            4,
            Vec::new(),
        );
        let out = tool
            .invoke(
                r#"{"task":"x","subagent":"ghost"}"#,
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert!(
            out.contains("[子 agent 结论]"),
            "unknown name → anonymous worker: {out}"
        );
    }

    // --- D1: tools_allow name-prefix narrowing ---

    #[test]
    fn restrict_to_prefixes_keeps_matching_and_drops_rest() {
        let mut reg = ToolRegistry::new();
        reg.push(read_only_probe("skill__web_search"));
        reg.push(read_only_probe("read_file"));
        reg.push(read_only_probe("bash"));
        // Prefixes match by name-prefix: "skill__" catches skill__web_search,
        // "read_file" catches read_file, "bash" has no allowed prefix → dropped.
        let mut names: Vec<String> = reg
            .restrict_to_prefixes(&["skill__".into(), "read_file".into()])
            .infos()
            .into_iter()
            .map(|t| t.name)
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["read_file".to_string(), "skill__web_search".to_string()]
        );
    }

    #[test]
    fn restrict_to_prefixes_empty_allowlist_keeps_everything() {
        let mut reg = ToolRegistry::new();
        reg.push(read_only_probe("a"));
        reg.push(read_only_probe("b"));
        // Empty allowlist = inherit the full set (anonymous-worker contract).
        assert_eq!(reg.restrict_to_prefixes(&[]).len(), 2);
    }

    #[test]
    fn restrict_to_prefixes_ignores_blank_prefix_entry() {
        // A stray "" must NOT match every name (that would silently disable the
        // allowlist). It's dropped; a list of ONLY blanks behaves like empty.
        let mut reg = ToolRegistry::new();
        reg.push(read_only_probe("a"));
        reg.push(read_only_probe("b"));
        assert_eq!(
            reg.restrict_to_prefixes(&["".into()]).len(),
            2,
            "blank-only allowlist inherits all (not 'match everything per-tool')"
        );
        // A mix of blank + real applies only the real prefix.
        let restricted = reg.restrict_to_prefixes(&["".into(), "a".into()]);
        assert_eq!(restricted.len(), 1);
        assert_eq!(restricted.infos()[0].name, "a");
    }

    #[test]
    fn child_tool_registry_empty_allowlist_inherits_full_readonly_set() {
        let mut reg = ToolRegistry::new();
        reg.push(read_only_probe("skill__web_search"));
        reg.push(read_only_probe("read_file"));
        reg.push(read_only_probe("bash"));
        let tool = SubAgentTool::new(shared_gen_model("x"), reg, 4, Vec::new());
        // Anonymous worker (no tools_allow) → full read-only subset.
        assert_eq!(tool.child_tool_registry(&[]).len(), 3);
    }

    #[test]
    fn child_tool_registry_nonempty_allowlist_narrows_to_matching() {
        let mut reg = ToolRegistry::new();
        reg.push(read_only_probe("skill__web_search"));
        reg.push(read_only_probe("read_file"));
        reg.push(read_only_probe("bash"));
        let tool = SubAgentTool::new(shared_gen_model("x"), reg, 4, Vec::new());
        // Named spec bound to tools_allow: [skill__web_search] → only that tool.
        let mut names: Vec<String> = tool
            .child_tool_registry(&["skill__web_search".into()])
            .infos()
            .into_iter()
            .map(|t| t.name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["skill__web_search".to_string()]);
    }

    #[test]
    fn sse_text_delta_yields_assistant_message() {
        let mut bufs = HashMap::new();
        let mut sig = String::new();
        let line = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#;
        let m = handle_sse_line(line, &mut bufs, &mut sig).unwrap();
        assert_eq!(m.content, "hi");
        assert!(m.tool_calls.is_empty());
        assert!(
            bufs.is_empty(),
            "text delta must not touch the tool accumulator"
        );
    }

    #[test]
    fn sse_accumulates_tool_use_across_split_json_deltas() {
        let mut bufs = HashMap::new();
        let mut sig = String::new();
        // content_block_start opens a tool_use block at index 1.
        let start = r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_9","name":"read_file"}}"#;
        assert!(
            handle_sse_line(start, &mut bufs, &mut sig).is_none(),
            "start yields nothing"
        );
        // input_json_delta arrives in two fragments — Anthropic streams partial JSON.
        let d1 = r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"/a"}}"#;
        let d2 = r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":".txt\"}"}}"#;
        assert!(
            handle_sse_line(d1, &mut bufs, &mut sig).is_none(),
            "json delta yields nothing"
        );
        assert!(
            handle_sse_line(d2, &mut bufs, &mut sig).is_none(),
            "json delta yields nothing"
        );
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
            Err(Error::Unsupported(
                "ScriptedModel: drive via stream()".into(),
            ))
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

    /// A ChatModel that records the `opts.model` of each `stream()` call and
    /// emits a fixed reply — lets a test assert the ReactAgent loop routed each
    /// turn (T9).
    #[derive(Clone)]
    struct RecordingModel {
        reply: Message,
        seen: Arc<std::sync::Mutex<Vec<Option<String>>>>,
        /// The id this stub reports via ChatModel::model_id. Empty by default
        /// (existing router tests use a sentinel router that ignores base_model,
        /// so they're unaffected); set via new_with_model_id for tests that need
        /// a concrete id (e.g. the base_model-fallback regression below).
        model_id: String,
    }

    impl RecordingModel {
        fn new(reply: Message) -> Self {
            Self {
                reply,
                seen: Arc::new(std::sync::Mutex::new(Vec::new())),
                model_id: String::new(),
            }
        }

        fn new_with_model_id(reply: Message, id: &str) -> Self {
            Self {
                reply,
                seen: Arc::new(std::sync::Mutex::new(Vec::new())),
                model_id: id.to_string(),
            }
        }
    }

    #[async_trait]
    impl ChatModel for RecordingModel {
        fn model_id(&self) -> &str {
            &self.model_id
        }
        async fn generate(&self, _: &[Message], _: &ModelOptions) -> Result<Message, Error> {
            Err(Error::Unsupported(
                "RecordingModel: drive via stream()".into(),
            ))
        }
        fn stream(&self, _: &[Message], opts: &ModelOptions) -> Result<MessageStream, Error> {
            self.seen.lock().unwrap().push(opts.model.clone());
            let msg = self.reply.clone();
            Ok(Box::pin(futures::stream::once(async move { Ok(msg) })))
        }
        fn with_tools(&self, _: &[ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
            Ok(Box::new(self.clone()))
        }
    }

    #[tokio::test]
    async fn run_per_step_router_overrides_opts_model() {
        use kernel_core::Agent;
        // Router always returns a sentinel; the recording model must see it as
        // opts.model on the (single, converging) turn. Proves the loop honors the
        // router before each stream call.
        let model = RecordingModel::new(Message::assistant("done"));
        let seen = model.seen.clone();
        let router: ModelRouterFn = Arc::new(|_, _| "routed-sentinel".to_string());
        let agent = ReactAgent::new(model, ToolRegistry::new(), "sys").with_model_router(router);
        let s = agent.run(go_input()).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Completed);
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen.len(),
            1,
            "one stream call on a converging turn: {seen:?}"
        );
        assert_eq!(
            seen[0].as_deref(),
            Some("routed-sentinel"),
            "router must override opts.model: {seen:?}"
        );
    }

    #[tokio::test]
    async fn run_loop_base_model_falls_back_to_chatmodel_id_not_flagship() {
        use kernel_core::Agent;
        // Regression (session 7f51a5d2, 2026-06-21): the chat path builds
        // AgentInput{model:None} (the resolved id already lives inside
        // GlmChatModel), so the per-step router's base_model used to fall back
        // to the hardcoded STRONG_MODEL (glm-4.6). With a model that resolved
        // to glm-5.2, that meant every GLM-family turn sent glm-4.6 → 401 (the
        // user's Z.AI key has no glm-4.6) — the user picked GLM-5.2 but the wire
        // body said glm-4.6. Fix: base_model falls back to the ChatModel's own
        // model_id() instead. Drive the REAL route_step router (not a sentinel)
        // so route_step's "non-STRONG base returned unchanged" guard is
        // exercised end-to-end: a glm-5.2 base yields glm-5.2 on the wire,
        // NEVER glm-4.6.
        let model = RecordingModel::new_with_model_id(Message::assistant("done"), "glm-5.2");
        let seen = model.seen.clone();
        let agent = ReactAgent::new(model, ToolRegistry::new(), "sys")
            .with_model_router(Arc::new(crate::kernel_impl::model_router::route_step));
        // Neutral prompt: no powerful hint, no short-confirmation keyword, so the
        // ONLY way the wire model becomes glm-5.2 is the base_model fallback
        // (route_step's guard returns a non-STRONG base unchanged). Pre-fix this
        // asserted glm-4.6 (STRONG_MODEL first-turn default).
        let input = kernel_core::AgentInput {
            prompt: "summarize the project goals".into(),
            working_dir: None,
            model: None,
            resume_from: None,
        };
        let s = agent.run(input).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Completed);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "one stream call on a converging turn: {seen:?}");
        assert_eq!(
            seen[0].as_deref(),
            Some("glm-5.2"),
            "base_model must fall back to the ChatModel's resolved id (glm-5.2), \
             not STRONG_MODEL (glm-4.6): {seen:?}"
        );
    }

    #[tokio::test]
    async fn run_halts_when_budget_exhausted() {
        use kernel_core::Agent;
        // Budget check always true → the agent degrades on turn 0 WITHOUT ever
        // calling the model. Proves the hard limit fires before spending.
        let model = RecordingModel::new(Message::assistant("done"));
        let seen = model.seen.clone();
        let check: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(|| true);
        let agent = ReactAgent::new(model, ToolRegistry::new(), "sys").with_budget_check(check);
        let s = agent.run(go_input()).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Failed);
        let summary = outcome.output_summary.expect("budget message");
        assert!(
            summary.contains("budget"),
            "budget reason in summary: {summary}"
        );
        // No LLM call was made — the limit fired before the first turn.
        assert!(
            seen.lock().unwrap().is_empty(),
            "no stream call when budget exhausted: {:?}",
            seen.lock().unwrap()
        );
    }

    // --- D2: UserPromptSubmit hook context injection into the run ---

    #[derive(Clone)]
    struct HistorySpy {
        reply: String,
        last_user: Arc<std::sync::Mutex<Option<String>>>,
    }

    #[async_trait]
    impl ChatModel for HistorySpy {
        async fn generate(&self, hist: &[Message], _: &ModelOptions) -> Result<Message, Error> {
            // Record the last user-role message the loop fed us — that's where
            // the injected hook context must appear.
            if let Some(m) = hist.iter().rev().find(|m| m.role == Role::User) {
                *self.last_user.lock().unwrap() = Some(m.content.clone());
            }
            Ok(Message::assistant(self.reply.clone()))
        }
        fn stream(&self, _: &[Message], _: &ModelOptions) -> Result<MessageStream, Error> {
            Err(Error::Unsupported("HistorySpy: drive via generate".into()))
        }
        fn with_tools(&self, _: &[ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
            Ok(Box::new(self.clone()))
        }
    }

    #[tokio::test]
    async fn run_loop_injects_user_hook_context_into_prompt() {
        // A UserPromptSubmit hook that echoes a sentinel → run_loop must append
        // that stdout to the user message BEFORE the model sees it. End-to-end
        // proof that dispatch_event + the injection wiring both work via the
        // generate path (run_loop / sub-agent).
        use crate::kernel_impl::hooks::HookManager;
        use crate::models::UserHookEvent;
        use crate::user_hooks::UserCommandHook;

        let spy = HistorySpy {
            reply: "done".into(),
            last_user: Arc::new(std::sync::Mutex::new(None)),
        };
        let last_user = spy.last_user.clone();

        let mut hooks = HookManager::new();
        hooks.register(Box::new(UserCommandHook::new(
            "inject".into(),
            UserHookEvent::UserPromptSubmit,
            // Cross-platform echo prints the sentinel on stdout.
            "echo ALWAYS-INJECT-SENTINEL".into(),
            true,
            10,
            std::env::current_dir().ok(),
        )));

        let agent = ReactAgent::new(spy, ToolRegistry::new(), "sys").with_hooks(Arc::new(hooks));
        let out = agent
            .run_loop("do the thing", ModelOptions::default())
            .await;
        assert!(out.is_ok(), "run_loop should converge: {out:?}");

        let captured = last_user
            .lock()
            .unwrap()
            .clone()
            .expect("model saw a user message");
        assert!(
            captured.contains("do the thing"),
            "original task preserved: {captured}"
        );
        assert!(
            captured.contains("[user-hook context]"),
            "hook-context fence injected: {captured}"
        );
        assert!(
            captured.contains("ALWAYS-INJECT-SENTINEL"),
            "hook stdout injected as context: {captured}"
        );
    }

    #[tokio::test]
    async fn run_loop_without_hooks_passes_plain_prompt() {
        // No HookManager → the prompt reaches the model verbatim, no fence. This
        // guards against accidentally injecting the fence when nothing ran.
        let spy = HistorySpy {
            reply: "done".into(),
            last_user: Arc::new(std::sync::Mutex::new(None)),
        };
        let last_user = spy.last_user.clone();
        // No with_hooks → self.hooks is None → dispatch skipped.
        let agent = ReactAgent::new(spy, ToolRegistry::new(), "sys");
        agent
            .run_loop("plain prompt", ModelOptions::default())
            .await
            .ok();
        let captured = last_user
            .lock()
            .unwrap()
            .clone()
            .expect("user message seen");
        assert_eq!(
            captured, "plain prompt",
            "no fence when no hooks: {captured}"
        );
    }

    // --- v2: exit-2 blocking (UserPromptSubmit + PreToolUse) ---

    #[tokio::test]
    async fn run_loop_submit_hook_exit2_blocks_without_entering_turn() {
        // v2: a UserPromptSubmit hook exiting 2 must REFUSE the turn — run_loop
        // returns the block reason as its answer and NEVER calls the model
        // (HistorySpy.last_user stays None). Proves dispatch_event's Err path
        // short-circuits before the user message enters history.
        use crate::kernel_impl::hooks::HookManager;
        use crate::models::UserHookEvent;
        use crate::user_hooks::UserCommandHook;

        let spy = HistorySpy {
            reply: "should-not-reach".into(),
            last_user: Arc::new(std::sync::Mutex::new(None)),
        };
        let last_user = spy.last_user.clone();

        let mut hooks = HookManager::new();
        hooks.register(Box::new(UserCommandHook::new(
            "gate".into(),
            UserHookEvent::UserPromptSubmit,
            "exit 2".into(),
            true,
            10,
            std::env::current_dir().ok(),
        )));

        let agent = ReactAgent::new(spy, ToolRegistry::new(), "sys").with_hooks(Arc::new(hooks));
        let out = agent
            .run_loop("do the thing", ModelOptions::default())
            .await;
        let answer = out.expect("block returns Ok with the reason, not Err");
        assert!(
            answer.contains("用户钩子阻止本轮提交"),
            "block reason surfaced as the answer: {answer}"
        );
        // The model was never called — the turn was refused before history.push.
        assert!(
            last_user.lock().unwrap().is_none(),
            "no user message reached the model on a blocked submit"
        );
    }

    #[tokio::test]
    async fn pre_tool_use_hook_exit2_blocks_tool_invocation() {
        // v2: a PreToolUse hook exiting 2 refuses the tool — execute_tool_call
        // returns the block reason and the tool body never runs (claude-code
        // PreToolUse semantics via the dispatch seam in execute_tool_call).
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut reg = ToolRegistry::new();
        reg.push(ProbeTool {
            name: "write_file",
            read_only: false,
            calls: calls.clone(),
        });

        let mut hooks = crate::kernel_impl::hooks::HookManager::new();
        hooks.register(Box::new(crate::user_hooks::UserCommandHook::new(
            "no-writes".into(),
            crate::models::UserHookEvent::PreToolUse,
            "exit 2".into(),
            true,
            10,
            std::env::current_dir().ok(),
        )));
        let agent =
            ReactAgent::new(ScriptedModel::new(vec![]), reg, "sys").with_hooks(Arc::new(hooks));
        let ctx = ToolContext::default();

        let r = agent
            .execute_tool_call(&probe_call("write_file", r#"{"path":"a.rs"}"#), &ctx)
            .await;
        assert!(
            r.contains("[blocked by user-hook:no-writes:"),
            "PreToolUse exit-2 must surface the block reason: {r}"
        );
        assert!(
            calls.lock().unwrap().is_empty(),
            "blocked tool must NOT be invoked"
        );
    }

    // --- v1.3: C1 context auto-compaction ---

    /// Mock that records the message count handed to each `stream()` call and
    /// counts `generate()` calls (the summarizer path). Drives the compaction
    /// integration: a large prior history must be summarized (generate fires)
    /// before the model sees it, so `stream` gets a compact history.
    #[derive(Clone)]
    struct CompactingModel {
        summary: String,
        stream_lens: Arc<std::sync::Mutex<Vec<usize>>>,
        generate_calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl CompactingModel {
        fn new(summary: &str) -> Self {
            Self {
                summary: summary.to_string(),
                stream_lens: Arc::new(std::sync::Mutex::new(Vec::new())),
                generate_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl ChatModel for CompactingModel {
        async fn generate(
            &self,
            _msgs: &[Message],
            _opts: &ModelOptions,
        ) -> Result<Message, Error> {
            self.generate_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Message::assistant(self.summary.clone()))
        }
        fn stream(&self, msgs: &[Message], _opts: &ModelOptions) -> Result<MessageStream, Error> {
            self.stream_lens.lock().unwrap().push(msgs.len());
            let msg = Message::assistant("done");
            Ok(Box::pin(futures::stream::once(async move { Ok(msg) })))
        }
        fn with_tools(&self, _: &[ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
            Ok(Box::new(self.clone()))
        }
    }

    #[tokio::test]
    async fn run_compacts_history_when_over_threshold() {
        use kernel_core::Agent;
        // 20 fat prior turns → far over a 100-token threshold. The loop must
        // summarize them (generate) BEFORE the first stream call, so the model
        // sees a compact history, not the whole transcript.
        let model = CompactingModel::new("压缩摘要");
        let stream_lens = model.stream_lens.clone();
        let gen_calls = model.generate_calls.clone();
        let mut prior = Vec::new();
        for i in 0..20 {
            prior.push(Message::user(format!("历史 turn {i} ").repeat(40)));
        }
        let agent = ReactAgent::new(model, ToolRegistry::new(), "sys")
            .with_history(prior)
            .with_context_compaction(100, 4);
        let s = agent.run(go_input()).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Completed);

        let gens = gen_calls.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(gens, 1, "summarizer (generate) must fire exactly once");

        let lens = stream_lens.lock().unwrap();
        assert_eq!(lens.len(), 1, "one converging turn");
        assert!(
            lens[0] <= 6,
            "stream must see the compacted history (system+summary+4 tail+task), got {}: {:?}",
            lens[0],
            lens
        );
        assert!(lens[0] < 22, "compaction must shrink from 22 messages");
    }

    #[tokio::test]
    async fn run_skips_compaction_under_threshold() {
        use kernel_core::Agent;
        // No prior history, generous threshold → no summarizer call, stream sees
        // the full (tiny) history verbatim: system + task = 2.
        let model = CompactingModel::new("压缩摘要");
        let stream_lens = model.stream_lens.clone();
        let gen_calls = model.generate_calls.clone();
        let agent = ReactAgent::new(model, ToolRegistry::new(), "sys")
            .with_context_compaction(1_000_000, 4);
        let s = agent.run(go_input()).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Completed);

        let gens = gen_calls.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(gens, 0, "no summarizer when under threshold");

        let lens = stream_lens.lock().unwrap();
        assert_eq!(lens.len(), 1);
        assert_eq!(lens[0], 2, "uncompacted history is system + task");
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

    // --- v1.1: C7 tool-call recovery (LLM retry + graceful degradation) ---

    /// Mock whose stream() always fails with a fixed error — drives the C7
    /// fatal-degradation path without a live LLM.
    struct ErrorModel {
        make: Arc<dyn Fn() -> Error + Send + Sync>,
    }
    #[async_trait]
    impl ChatModel for ErrorModel {
        async fn generate(&self, _: &[Message], _: &ModelOptions) -> Result<Message, Error> {
            Err((self.make)())
        }
        fn stream(&self, _: &[Message], _: &ModelOptions) -> Result<MessageStream, Error> {
            Err((self.make)())
        }
        fn with_tools(&self, _: &[ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
            Ok(Box::new(ErrorModel {
                make: self.make.clone(),
            }))
        }
    }

    /// Mock that fails the first `fails` stream attempts with a Network error
    /// (Retryable), then succeeds — drives the C7 retry-then-recover path.
    #[derive(Clone)]
    struct RetryingModel {
        fails: usize,
        call: Arc<std::sync::atomic::AtomicUsize>,
        ok: Message,
    }
    #[async_trait]
    impl ChatModel for RetryingModel {
        async fn generate(&self, _: &[Message], _: &ModelOptions) -> Result<Message, Error> {
            Err(Error::Unsupported(
                "RetryingModel: drive via stream()".into(),
            ))
        }
        fn stream(&self, _: &[Message], _: &ModelOptions) -> Result<MessageStream, Error> {
            let idx = self.call.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if idx < self.fails {
                Err(Error::Network("transient blip".into()))
            } else {
                let msg = self.ok.clone();
                Ok(Box::pin(futures::stream::once(async move { Ok(msg) })))
            }
        }
        fn with_tools(&self, _: &[ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
            Ok(Box::new(self.clone()))
        }
    }

    /// Drain a run stream and return the terminal Done outcome.
    async fn collect_outcome<S>(mut s: S) -> Option<kernel_core::AgentOutcome>
    where
        S: futures::Stream<Item = Result<kernel_core::AgentEvent, Error>> + Unpin,
    {
        use futures::StreamExt;
        while let Some(ev) = s.next().await {
            if let Ok(kernel_core::AgentEvent::Done(o)) = ev {
                return Some(o);
            }
        }
        None
    }

    fn go_input() -> kernel_core::AgentInput {
        kernel_core::AgentInput {
            prompt: "go".into(),
            working_dir: None,
            model: None,
            resume_from: None,
        }
    }

    #[tokio::test]
    async fn run_degrades_on_fatal_auth_error() {
        use kernel_core::Agent;
        // 401 is Fatal::Auth — no retry, graceful Done with the auth message.
        let model = ErrorModel {
            make: Arc::new(|| Error::Model("GLM failed: 401 unauthorized".into())),
        };
        let agent = ReactAgent::new(model, ToolRegistry::new(), "sys");
        let s = agent.run(go_input()).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Failed);
        let summary = outcome.output_summary.expect("degraded summary");
        assert!(summary.contains("API key"), "auth message: {summary}");
    }

    #[tokio::test]
    async fn run_retries_transient_then_completes() {
        use kernel_core::Agent;
        // First stream() call fails with Network (Retryable); the second
        // succeeds with bare text. Proves the run loop backs off (~1s real
        // sleep) and recovers instead of dying on the first blip. Single retry
        // keeps the cost to ~1s (tokio test-util/pause isn't enabled here).
        let model = RetryingModel {
            fails: 1,
            call: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            ok: Message::assistant("recovered"),
        };
        let agent = ReactAgent::new(model, ToolRegistry::new(), "sys");
        let s = agent.run(go_input()).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Completed);
        assert_eq!(outcome.output_summary.as_deref(), Some("recovered"));
    }

    #[tokio::test]
    async fn run_reports_step_limit_when_never_converging() {
        use kernel_core::Agent;
        // Every turn emits a tool_call → the loop never sees an empty-tool_calls
        // turn, so it must hit max_steps and report Failed (not the old
        // dishonest Completed with a stale/empty summary).
        let loop_msg = Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![kernel_core::ToolCall {
                id: "c".into(),
                call_type: "function".into(),
                function: kernel_core::FunctionCall {
                    name: "echo".into(),
                    arguments: r#"{"text":"x"}"#.into(),
                },
            }],
            tool_call_id: None,
            reasoning: None,
            reasoning_signature: None,
        };
        let model = ScriptedModel::new(vec![loop_msg; 16]);
        let reg = ToolRegistry::new().with(EchoTool);
        let agent = ReactAgent::new(model, reg, "sys").with_max_steps(3);
        let s = agent.run(go_input()).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Failed);
        let summary = outcome.output_summary.expect("step-limit summary");
        assert!(summary.contains("step"), "step-limit message: {summary}");
    }

    // --- v1.2 T7: self-verify gate (audit feeds back → self-repair) ---

    #[tokio::test]
    async fn run_self_verify_feeds_back_failure_then_completes() {
        use kernel_core::Agent;
        use std::sync::atomic::{AtomicUsize, Ordering};
        // turn 0: bare "done"; turn 1 (after feed-back): bare "fixed".
        let model = ScriptedModel::new(vec![
            Message::assistant("done"),
            Message::assistant("fixed"),
        ]);
        // Audit stub: always reports failed. max_verify=1 → the first
        // convergence feeds back; the second convergence skips verification
        // (verify_count == max_verify) and completes.
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_fn = calls.clone();
        let audit_fn: AuditFn = Arc::new(move |_, _| {
            calls_for_fn.fetch_add(1, Ordering::SeqCst);
            serde_json::json!({
                "status": "failed",
                "findings": [{"rule": "test", "severity": "error", "message": "broken"}]
            })
        });
        let ctx = kernel_core::ToolContext {
            working_dir: Some("/tmp/nonexistent".into()),
            conversation_id: None,
        };
        let agent = ReactAgent::new(model, ToolRegistry::new(), "sys")
            .with_max_verify(1)
            .with_audit_fn(audit_fn)
            .with_context(ctx);
        let s = agent.run(go_input()).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Completed);
        // The second turn's answer is the final output (after self-repair).
        assert_eq!(outcome.output_summary.as_deref(), Some("fixed"));
        // Audit ran exactly once (first convergence); second skipped.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn run_self_verify_disabled_when_max_verify_zero() {
        use kernel_core::Agent;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let model = ScriptedModel::new(vec![Message::assistant("done")]);
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_fn = calls.clone();
        let audit_fn: AuditFn = Arc::new(move |_, _| {
            calls_for_fn.fetch_add(1, Ordering::SeqCst);
            serde_json::json!({"status": "failed"})
        });
        let ctx = kernel_core::ToolContext {
            working_dir: Some("/tmp/nonexistent".into()),
            conversation_id: None,
        };
        // max_verify defaults to 0 → no verification, audit never called.
        let agent = ReactAgent::new(model, ToolRegistry::new(), "sys")
            .with_audit_fn(audit_fn)
            .with_context(ctx);
        let s = agent.run(go_input()).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Completed);
        assert_eq!(outcome.output_summary.as_deref(), Some("done"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "audit must not run when max_verify=0"
        );
    }

    // --- completion-hook fidelity: real agent stream → FileChanged → persist ---
    //
    // The completion hook (commands/agents.rs) consumes a ReactAgent's event
    // stream, maps each AgentEvent to a ChatStreamEvent, accumulates them, and
    // on Completed hands the blocks to persist_completion_memory. The hook's
    // wrapping closure (Tauri AppHandle.emit + tokio::spawn + live model) can't
    // run in `cargo test`, but everything INSIDE it that matters can: drive a
    // real ReactAgent with a mock model + ProbeTool (a write_file stand-in that
    // records calls but writes nothing), consume its ACTUAL stream exactly like
    // the driver does, then persist. Proves a write the agent really emits flows
    // unchanged into a queryable react_reflection row — the input shape the hook
    // depends on, verified end-to-end minus only the GUI glue.

    #[tokio::test]
    async fn run_stream_filechanged_flows_into_persisted_reflection() {
        use futures::StreamExt;
        use kernel_core::Agent;
        let write_calls = Arc::new(Mutex::new(Vec::new()));
        let mut reg = ToolRegistry::new();
        reg.push(ProbeTool {
            name: "write_file",
            read_only: false,
            calls: write_calls.clone(),
        });
        // turn 0: a write_file tool call → agent executes ProbeTool + emits
        // FileChanged(src/a.rs); turn 1: bare text → convergence → Done(Completed).
        let model = ScriptedModel::new(vec![
            Message {
                role: Role::Assistant,
                content: String::new(),
                tool_calls: vec![probe_call(
                    "write_file",
                    r#"{"path":"src/a.rs","content":"x"}"#,
                )],
                tool_call_id: None,
                reasoning: None,
                reasoning_signature: None,
            },
            Message::assistant("done, wrote src/a.rs"),
        ]);
        let agent = ReactAgent::new(model, reg, "sys");
        let mut s = agent.run(go_input()).unwrap();

        // Mirror react_chat_driver's consumption (agents.rs:294-336): every
        // AgentEvent → map_agent_event → accumulate; capture status + summary.
        let mut final_blocks: Vec<crate::agents::pty::ChatStreamEvent> = Vec::new();
        let mut completed = false;
        let mut summary = String::new();
        while let Some(Ok(ev)) = s.next().await {
            if let kernel_core::AgentEvent::Done(o) = &ev {
                completed = matches!(o.status, kernel_core::AgentRunStatus::Completed);
                if let Some(sm) = &o.output_summary {
                    summary = sm.clone();
                }
            }
            final_blocks.extend(crate::agents::react_chat::map_agent_event(ev, 0));
        }
        assert!(completed, "agent must converge Completed");
        assert!(
            !write_calls.lock().unwrap().is_empty(),
            "write_file must have actually executed"
        );

        // The completion hook's core, fed the REAL accumulated blocks (not a
        // hand-built fixture): a prose summary + a write tool → 2 entries.
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::init_db(&tmp.path().join("e.db")).unwrap();
        let hash = crate::activity::hash_project_path("/proj");
        let n = crate::kernel_impl::session_reflection::persist_completion_memory(
            &conn,
            &hash,
            "sid",
            "write src/a.rs",
            if summary.is_empty() {
                None
            } else {
                Some(&summary)
            },
            &final_blocks,
            &crate::models::AgentType::ClaudeCode,
        );
        assert_eq!(
            n, 2,
            "prose summary + write tool → react_session + react_reflection"
        );
        let got = crate::knowledge::store::get_entries_for_project(&conn, &hash).unwrap();
        let refl = got
            .iter()
            .find(|e| e.category == "react_reflection")
            .expect("react_reflection written from a real agent stream");
        // The agent REALLY emitted FileChanged("src/a.rs"); it survived
        // map_agent_event into the structured reflection content verbatim.
        assert!(
            refl.content.contains("src/a.rs"),
            "real file path landed: {}",
            refl.content
        );
        assert!(
            refl.content.contains("write_file"),
            "tool counted: {}",
            refl.content
        );
    }

    /// ScriptedModel that ALSO records every history snapshot passed into
    /// `stream()` — so a test can assert what the REAL run loop fed back to the
    /// model on a later turn (e.g. consecutive Role::Tool Messages produced by
    /// parallel tool_use). Shares script/call/seen across the with_tools clone.
    #[derive(Clone)]
    struct CapturingModel {
        script: Arc<Vec<Message>>,
        call: Arc<std::sync::atomic::AtomicUsize>,
        seen: Arc<std::sync::Mutex<Vec<Vec<Message>>>>,
    }

    impl CapturingModel {
        fn new(script: Vec<Message>, seen: Arc<std::sync::Mutex<Vec<Vec<Message>>>>) -> Self {
            Self {
                script: Arc::new(script),
                call: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                seen,
            }
        }
    }

    #[async_trait]
    impl ChatModel for CapturingModel {
        async fn generate(&self, _: &[Message], _: &ModelOptions) -> Result<Message, Error> {
            Err(Error::Unsupported(
                "CapturingModel: drive via stream()".into(),
            ))
        }
        fn stream(&self, msgs: &[Message], _opts: &ModelOptions) -> Result<MessageStream, Error> {
            self.seen.lock().unwrap().push(msgs.to_vec());
            let idx = self.call.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let msg = self
                .script
                .get(idx)
                .cloned()
                .unwrap_or_else(|| Message::assistant(String::new()));
            Ok(Box::pin(futures::stream::once(async move { Ok(msg) })))
        }
        fn with_tools(&self, _: &[ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
            Ok(Box::new(self.clone()))
        }
    }

    #[tokio::test]
    async fn run_loop_parallel_tool_use_feeds_merged_history_into_build_body() {
        // Internal E2E (everything real except the live HTTP hop): drive the
        // REAL ReactAgent run loop with a model that emits TWO parallel
        // tool_use calls in one turn, capture the history it hands back on
        // turn 2, then feed that REAL history through the REAL
        // GlmChatModel::build_body. Spans the full bug chain behind session
        // 34f2c468's 400 — run loop → consecutive Role::Tool Messages →
        // build_body merge — not just the build_body pure function alone.
        use kernel_core::Agent;
        let read_calls = Arc::new(Mutex::new(Vec::new()));
        let glob_calls = Arc::new(Mutex::new(Vec::new()));
        let mut reg = ToolRegistry::new();
        reg.push(ProbeTool {
            name: "read_file",
            read_only: true,
            calls: read_calls.clone(),
        });
        reg.push(ProbeTool {
            name: "glob",
            read_only: true,
            calls: glob_calls.clone(),
        });

        // Turn 0: assistant requests read_file AND glob in ONE message (the
        // parallel-tool-use shape). Turn 1: bare text → convergence.
        let turn0 = Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![
                kernel_core::ToolCall {
                    id: "call_00".into(),
                    call_type: "function".into(),
                    function: kernel_core::FunctionCall {
                        name: "read_file".into(),
                        arguments: r#"{"file_path":"package.json"}"#.into(),
                    },
                },
                kernel_core::ToolCall {
                    id: "call_01".into(),
                    call_type: "function".into(),
                    function: kernel_core::FunctionCall {
                        name: "glob".into(),
                        arguments: r#"{"pattern":"*"}"#.into(),
                    },
                },
            ],
            tool_call_id: None,
            reasoning: None,
            reasoning_signature: None,
        };
        let seen = Arc::new(Mutex::new(Vec::new()));
        let model = CapturingModel::new(vec![turn0, Message::assistant("done")], seen.clone());

        let agent = ReactAgent::new(model, reg, "sys");
        let s = agent.run(go_input()).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Completed);
        // Both parallel tools actually executed (the loop dispatched BOTH).
        assert_eq!(read_calls.lock().unwrap().len(), 1, "read_file executed");
        assert_eq!(glob_calls.lock().unwrap().len(), 1, "glob executed");

        // Turn-2 history carries the assistant turn + a Role::Tool Message per
        // call → two CONSECUTIVE Tool Messages. Pre-fix build_body serialized
        // these into two back-to-back user messages → Anthropic 400.
        let histories = seen.lock().unwrap();
        assert!(histories.len() >= 2, "model invoked on turn 2");
        let turn2 = histories.last().unwrap();
        let tail: Vec<&Message> = turn2.iter().rev().take(2).collect();
        assert_eq!(tail[1].role, Role::Tool);
        assert_eq!(tail[1].tool_call_id.as_deref(), Some("call_00"));
        assert_eq!(tail[0].role, Role::Tool, "consecutive Tool messages");
        assert_eq!(tail[0].tool_call_id.as_deref(), Some("call_01"));

        // Feed that REAL turn-2 history through the REAL GlmChatModel
        // build_body: the two consecutive Tool Messages MUST merge into ONE
        // user message, restoring strict user/assistant alternation.
        let glm = GlmChatModel::bigmodel("k", "glm-4.6");
        let body = glm.build_body("glm-4.6", turn2, &ModelOptions::default(), false);
        let wire = body["messages"].as_array().unwrap();
        let roles: Vec<&str> = wire.iter().map(|m| m["role"].as_str().unwrap()).collect();
        for w in wire.windows(2) {
            assert_ne!(
                w[0]["role"], w[1]["role"],
                "back-to-back roles: {:?}",
                roles
            );
        }
        let merged = wire.last().unwrap();
        assert_eq!(merged["role"], "user");
        let results = merged["content"].as_array().unwrap();
        assert_eq!(
            results.len(),
            2,
            "both tool_results merged into one user message"
        );
        assert_eq!(results[0]["tool_use_id"], "call_00");
        assert_eq!(results[1]["tool_use_id"], "call_01");
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
        let stop =
            handle_sse_line(r#"data: {"type":"message_stop"}"#, &mut bufs, &mut sig).unwrap();
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
        assert_eq!(
            parse_usage(start),
            Some(pricing::TokenUsage { input: 42, output: 0, cache_read: 0, cache_write: 0 })
        );
        // Standard Anthropic: message_delta carries only output_tokens.
        let delta = r#"data: {"type":"message_delta","usage":{"output_tokens":128}}"#;
        assert_eq!(
            parse_usage(delta),
            Some(pricing::TokenUsage { input: 0, output: 128, cache_read: 0, cache_write: 0 })
        );
        // GLM: message_delta ALSO carries the real input_tokens (message_start's
        // is a 0 placeholder). parse_usage reads both → the caller's
        // saturating_add recovers the real input. Without this the streaming
        // path undercounted input tokens to 0.
        let glm_delta =
            r#"data: {"type":"message_delta","usage":{"input_tokens":16,"output_tokens":10}}"#;
        assert_eq!(
            parse_usage(glm_delta),
            Some(pricing::TokenUsage { input: 16, output: 10, cache_read: 0, cache_write: 0 })
        );
        // Non-usage event types → None.
        assert_eq!(parse_usage(r#"data: {"type":"content_block_delta"}"#), None);
        // Non-data lines → None.
        assert_eq!(parse_usage("event: ping"), None);
        assert_eq!(parse_usage(""), None);
    }

    #[test]
    fn parse_usage_reads_prompt_cache_tiers_from_message_start() {
        // B5: real Anthropic reports cache_read_input_tokens +
        // cache_creation_input_tokens on message_start. parse_usage must surface
        // them so the transparent cost breakdown can price the cache tiers.
        let start = r#"data: {"type":"message_start","message":{"usage":{"input_tokens":100,"cache_read_input_tokens":5000,"cache_creation_input_tokens":2000}}}"#;
        let usage = parse_usage(start).expect("message_start yields usage");
        assert_eq!(usage.input, 100);
        assert_eq!(usage.cache_read, 5000);
        assert_eq!(usage.cache_write, 2000);
        // message_delta never carries cache tiers → both stay 0.
        let delta = r#"data: {"type":"message_delta","usage":{"output_tokens":1}}"#;
        let d = parse_usage(delta).expect("message_delta yields usage");
        assert_eq!(d.cache_read, 0);
        assert_eq!(d.cache_write, 0);
    }

    #[test]
    fn usage_from_response_reads_usage_object() {
        let v = json!({"usage":{"input_tokens":10,"output_tokens":20}});
        assert_eq!(
            usage_from_response(&v),
            pricing::TokenUsage { input: 10, output: 20, cache_read: 0, cache_write: 0 }
        );
        // Missing usage → all-zero TokenUsage, not an error.
        let v2 = json!({"content":[]});
        assert_eq!(usage_from_response(&v2), pricing::TokenUsage::default());
    }

    #[test]
    fn usage_from_response_reads_cache_tiers() {
        // B5: the non-streaming path must also surface the cache tiers.
        let v = json!({"usage":{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":7,"cache_creation_input_tokens":3}});
        let usage = usage_from_response(&v);
        assert_eq!(usage.input, 10);
        assert_eq!(usage.output, 20);
        assert_eq!(usage.cache_read, 7);
        assert_eq!(usage.cache_write, 3);
    }

    #[test]
    fn glm_model_attaches_circuit_and_cost_sink_builders() {
        use crate::cost::circuit_breaker::CircuitBreakerConfig;
        use crate::cost::sink::NullCostSink;
        use std::time::Duration;
        let m = GlmChatModel::bigmodel("k", "glm-4.6")
            .with_circuit(std::sync::Arc::new(CircuitBreaker::new(
                CircuitBreakerConfig {
                    failure_threshold: 1,
                    cooldown: Duration::from_secs(60),
                    half_open_max: 1,
                },
            )))
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
            thinking: Some(ThinkingConfig {
                budget_tokens: 2000,
            }),
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

    #[test]
    fn build_body_merges_parallel_tool_results_into_one_user_message() {
        // Reproduces session 34f2c468's instant 400: an assistant turn that
        // issues TWO parallel tool_use calls. The run loop appends one
        // Role::Tool Message per result, so history carries two consecutive
        // Tool Messages. build_body MUST merge them into a single user message
        // (array of tool_result blocks) — emitting two back-to-back user
        // messages trips Anthropic's 400: "tool_use ids were found without
        // tool_result blocks immediately after".
        use kernel_core::{FunctionCall, Message, ModelOptions, Role, ToolCall};
        let assistant_turn = Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![
                ToolCall {
                    id: "call_00".into(),
                    call_type: "function".into(),
                    function: FunctionCall {
                        name: "read_file".into(),
                        arguments: r#"{"file_path":"package.json"}"#.into(),
                    },
                },
                ToolCall {
                    id: "call_01".into(),
                    call_type: "function".into(),
                    function: FunctionCall {
                        name: "glob".into(),
                        arguments: r#"{"pattern":"*"}"#.into(),
                    },
                },
            ],
            tool_call_id: None,
            reasoning: None,
            reasoning_signature: None,
        };
        let tool_a = Message {
            role: Role::Tool,
            content: "PKG".into(),
            tool_calls: Vec::new(),
            tool_call_id: Some("call_00".into()),
            reasoning: None,
            reasoning_signature: None,
        };
        let tool_b = Message {
            role: Role::Tool,
            content: "f1\nf2".into(),
            tool_calls: Vec::new(),
            tool_call_id: Some("call_01".into()),
            reasoning: None,
            reasoning_signature: None,
        };
        let history = vec![Message::user("list files"), assistant_turn, tool_a, tool_b];
        let model = GlmChatModel::bigmodel("k", "glm-4.6");
        let body = model.build_body("glm-4.6", &history, &ModelOptions::default(), false);
        let msgs = body["messages"].as_array().unwrap();
        // Merge: 4 internal non-system messages → 3 wire messages (no back-to-back user).
        assert_eq!(
            msgs.len(),
            3,
            "parallel tool_results must merge into one user message"
        );
        // Strict alternation — the protocol property this fix restores.
        let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, vec!["user", "assistant", "user"]);
        // The merged user message carries BOTH tool_result blocks, in order.
        let merged = &msgs[2]["content"];
        assert_eq!(merged.as_array().unwrap().len(), 2);
        assert_eq!(merged[0]["type"], "tool_result");
        assert_eq!(merged[0]["tool_use_id"], "call_00");
        assert_eq!(merged[0]["content"], "PKG");
        assert_eq!(merged[1]["tool_use_id"], "call_01");
        assert_eq!(merged[1]["content"], "f1\nf2");
    }

    #[test]
    fn build_body_keeps_single_tool_result_in_one_user_message() {
        // Regression guard: a single-tool turn (the overwhelmingly common case)
        // must stay exactly one user message with one tool_result block — the
        // merge path must not split or duplicate it.
        use kernel_core::{FunctionCall, Message, ModelOptions, Role, ToolCall};
        let assistant_turn = Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "call_00".into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "read_file".into(),
                    arguments: r#"{"file_path":"a"}"#.into(),
                },
            }],
            tool_call_id: None,
            reasoning: None,
            reasoning_signature: None,
        };
        let tool = Message {
            role: Role::Tool,
            content: "A".into(),
            tool_calls: Vec::new(),
            tool_call_id: Some("call_00".into()),
            reasoning: None,
            reasoning_signature: None,
        };
        let model = GlmChatModel::bigmodel("k", "glm-4.6");
        let body = model.build_body(
            "glm-4.6",
            &[Message::user("go"), assistant_turn, tool],
            &ModelOptions::default(),
            false,
        );
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        let last = &msgs[2];
        assert_eq!(last["role"], "user");
        assert_eq!(last["content"].as_array().unwrap().len(), 1);
        assert_eq!(last["content"][0]["tool_use_id"], "call_00");
    }

    // ===== GLM real-response fixtures =====
    //
    // Replay responses recorded from a live GLM (Anthropic-compatible) endpoint
    // through the pure parse functions (`decode_anthropic_message`,
    // `handle_sse_line`, `parse_usage`, `usage_from_response`). No HTTP, no key,
    // fully deterministic. These are the ONLY tests exercising the parse layer
    // against GLM's actual wire format: GLM-specific usage fields, '\n' emitted
    // as a standalone text_delta, tool_use content_block accumulation.
    //
    // Fixtures: tests/fixtures/glm/. To re-record (needs a live key) rerun the
    // curl commands; the fixtures intentionally contain NO credential.

    /// Replay an SSE byte stream through the same per-line loop `stream()` runs
    /// (split on '\n', trim, `parse_usage` + `handle_sse_line`). Returns the
    /// yielded Messages and accumulated usage. No HTTP — the unit-test harness
    /// for the streaming parse path.
    fn replay_sse(sse: &str) -> (Vec<Message>, pricing::TokenUsage) {
        let mut tool_bufs: HashMap<u64, (String, String, String)> = HashMap::new();
        let mut sig_buf = String::new();
        let mut msgs = Vec::new();
        let mut usage = pricing::TokenUsage::default();
        for raw in sse.split('\n') {
            let line = raw.trim();
            if let Some(delta) = parse_usage(line) {
                usage = usage.saturating_add(delta);
            }
            if let Some(msg) = handle_sse_line(line, &mut tool_bufs, &mut sig_buf) {
                msgs.push(msg);
            }
        }
        (msgs, usage)
    }

    #[test]
    fn decode_anthropic_message_parses_real_glm_nonstream() {
        // Real GLM non-stream response carries GLM-specific usage extensions
        // (cache_read_input_tokens / server_tool_use / service_tier). The
        // decoder must extract text and ignore the extras.
        let raw = include_str!("../../tests/fixtures/glm/nonstream_text.json");
        let v: Value = serde_json::from_str(raw).expect("fixture is valid JSON");
        let msg = decode_anthropic_message(&v).expect("decode succeeds");
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content, "PONG");
        assert!(msg.tool_calls.is_empty(), "no tool_use in a plain reply");
        assert!(msg.reasoning.is_none(), "no thinking block");
    }

    #[test]
    fn usage_from_response_reads_real_glm_usage() {
        // GLM usage object has standard input/output_tokens plus extras;
        // usage_from_response reads input/output (and cache tiers if present,
        // which GLM doesn't emit → 0).
        let raw = include_str!("../../tests/fixtures/glm/nonstream_text.json");
        let v: Value = serde_json::from_str(raw).unwrap();
        let usage = usage_from_response(&v);
        assert_eq!(usage.input, 15);
        assert_eq!(usage.output, 3);
        assert_eq!(usage.cache_read, 0, "GLM emits no cache_read_input_tokens");
        assert_eq!(usage.cache_write, 0);
    }

    #[test]
    fn handle_sse_line_streams_real_glm_text_deltas() {
        // Real GLM streams `count 1..5` and emits '\n' as its OWN text_delta —
        // the per-token fragmentation GLM uses. Deltas must concatenate to
        // "1\n2\n3\n4\n5". (GLM fragmentation is the historically flaky path.)
        let sse = include_str!("../../tests/fixtures/glm/stream_text.sse");
        let (msgs, usage) = replay_sse(sse);
        let text: String = msgs
            .iter()
            .filter(|m| !m.content.is_empty())
            .map(|m| m.content.clone())
            .collect();
        assert_eq!(text, "1\n2\n3\n4\n5");
        assert_eq!(usage.output, 10, "output_tokens from message_delta");
        assert!(msgs.iter().all(|m| m.tool_calls.is_empty()));
    }

    #[test]
    fn glm_stream_input_tokens_recovered_from_message_delta() {
        // GLM puts the REAL input_tokens (16) on message_delta; message_start
        // carries 0. parse_usage reads input from BOTH events and saturating_adds
        // them → 0 + 16 = 16. (Standard Anthropic omits input_tokens on
        // message_delta → stays at start_input + 0, no double count.) This was a
        // 0-undercount bug before parse_usage's message_delta branch learned to
        // read input_tokens.
        let sse = include_str!("../../tests/fixtures/glm/stream_text.sse");
        let (_msgs, usage) = replay_sse(sse);
        assert_eq!(usage.input, 16, "input_tokens accumulated from message_delta");
        assert_eq!(usage.output, 10);
    }

    #[test]
    fn handle_sse_line_assembles_real_glm_tool_use() {
        // Real GLM tool_use stream: index 0 text block, index 1 tool_use block.
        // GLM sent the whole partial_json in one input_json_delta; message_stop
        // reassembles it into a terminal tool_calls Message (id/name/args).
        let sse = include_str!("../../tests/fixtures/glm/stream_tool_use.sse");
        let (msgs, _usage) = replay_sse(sse);
        let terminal = msgs
            .last()
            .expect("message_stop yields terminal tool_calls");
        assert!(!terminal.tool_calls.is_empty(), "tool_use reassembled");
        let tc = &terminal.tool_calls[0];
        assert_eq!(tc.function.name, "get_weather");
        assert_eq!(tc.function.arguments, "{\"city\":\"Beijing\"}");
        assert!(
            !tc.id.is_empty(),
            "tool_use id carried from content_block_start"
        );
        // The text block before the tool_use still streamed inline.
        let text: String = msgs
            .iter()
            .take_while(|m| m.tool_calls.is_empty())
            .filter(|m| !m.content.is_empty())
            .map(|m| m.content.clone())
            .collect();
        assert!(text.contains("Beijing"), "preamble text streamed: {text}");
    }

    #[test]
    fn handle_sse_line_accumulates_fragmented_tool_use_input() {
        // The real recording sent partial_json in one chunk; this hand-split
        // variant fragments {"city":"Beijing"} across 3 input_json_delta events
        // ("{\"ci" / "ty\":\"Be" / "ijing\"}"). GLM fragments long tool args in
        // practice, so the slot.2.push_str accumulation is a must-test path.
        let sse = include_str!("../../tests/fixtures/glm/stream_tool_use_fragmented.sse");
        let (msgs, _usage) = replay_sse(sse);
        let terminal = msgs.last().expect("terminal tool_calls");
        assert_eq!(terminal.tool_calls.len(), 1);
        let tc = &terminal.tool_calls[0];
        assert_eq!(tc.id, "call_frag");
        assert_eq!(tc.function.name, "get_weather");
        assert_eq!(tc.function.arguments, "{\"city\":\"Beijing\"}");
    }

    // ===== live GLM smoke (#[ignore]: needs GLM_API_KEY, spends tokens) =====

    /// Cost sink that captures the last usage GlmChatModel reported, so the live
    /// smoke test can assert input_tokens > 0 after the parse_usage fix.
    struct CapturingSink(std::sync::Mutex<crate::cost::pricing::TokenUsage>);

    impl CapturingSink {
        fn new() -> Self {
            Self(std::sync::Mutex::new(crate::cost::pricing::TokenUsage::default()))
        }
    }

    impl crate::cost::sink::CostSink for CapturingSink {
        fn record(&self, _: &str, usage: crate::cost::pricing::TokenUsage, _: f64) {
            *self.0.lock().unwrap() = usage;
        }
    }

    #[ignore = "needs a live GLM key in GLM_API_KEY env; spends real tokens"]
    #[tokio::test]
    async fn glm_live_stream_meters_real_input_tokens_end_to_end() {
        // End-to-end against the real GLM endpoint. A streaming call must
        // (a) complete and yield assistant text, and (b) meter input_tokens > 0
        // — proving the parse_usage fix works on GLM's real wire format (GLM
        // reports input on message_delta, where the pre-fix code never looked).
        // Skipped without GLM_API_KEY so CI stays green; run locally:
        //   GLM_API_KEY=... cargo test --lib -- --ignored glm_live
        let key = match std::env::var("GLM_API_KEY") {
            Ok(k) if !k.is_empty() => k,
            _ => {
                eprintln!("GLM_API_KEY unset — skipping live smoke");
                return;
            }
        };
        use futures::StreamExt;
        let sink = std::sync::Arc::new(CapturingSink::new());
        let model = GlmChatModel::bigmodel(key, "glm-4.6")
            .with_cost_sink(
                std::sync::Arc::clone(&sink) as std::sync::Arc<dyn crate::cost::sink::CostSink>
            );
        let stream = model
            .stream(
                &[Message::user("Reply with exactly one word: HELLO")],
                &ModelOptions::default(),
            )
            .expect("stream starts");
        let collected: Vec<Message> = stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .filter_map(|r| r.ok())
            .collect();
        let text: String = collected.iter().map(|m| m.content.clone()).collect();
        let usage = *sink.0.lock().unwrap();
        assert!(!text.is_empty(), "live GLM returned text: {text:?}");
        assert!(
            usage.input > 0,
            "input_tokens metered from message_delta after fix, got {}",
            usage.input
        );
    }

    #[ignore = "needs a live GLM key in GLM_API_KEY env; spends real tokens"]
    #[tokio::test]
    async fn glm_live_react_agent_runs_full_loop_and_meters_cost() {
        // The deepest backend end-to-end check without the GUI: a real
        // ReactAgent drives its reason->act->observe loop against live GLM.
        // This wires together every layer the front-end would reach over IPC —
        // the system prompt, the streaming GLM call, SSE parsing, the agent run
        // loop emitting Token + Done, and the cost sink receiving real usage —
        // so a regression in any of them surfaces here, not just in the
        // stream()-only smoke above. No tools => single turn (GLM replies with
        // text, no tool_calls, loop ends after one model round); the tool-calling
        // loop itself is covered by the mock-driven self_agent_e2e_test, while
        // here the point is the LIVE wire format flowing through the whole agent.
        // Skipped without GLM_API_KEY so CI stays green; run locally:
        //   GLM_API_KEY=... cargo test --lib -- --ignored glm_live
        let key = match std::env::var("GLM_API_KEY") {
            Ok(k) if !k.is_empty() => k,
            _ => {
                eprintln!("GLM_API_KEY unset — skipping live smoke");
                return;
            }
        };
        use futures::StreamExt;
        use kernel_core::{Agent, AgentEvent, AgentInput};
        let sink = std::sync::Arc::new(CapturingSink::new());
        let model = GlmChatModel::bigmodel(key, "glm-4.6")
            .with_cost_sink(
                std::sync::Arc::clone(&sink) as std::sync::Arc<dyn crate::cost::sink::CostSink>
            );
        let agent = ReactAgent::new(model, ToolRegistry::new(), "You are a concise assistant.");
        let mut stream = agent
            .run(AgentInput {
                prompt: "Reply with exactly one word: PONG".into(),
                working_dir: None,
                model: None,
                resume_from: None,
            })
            .expect("agent run starts");
        let mut done = false;
        let mut text = String::new();
        while let Some(ev) = stream.next().await {
            match ev.unwrap() {
                AgentEvent::Token(t) => text.push_str(&t),
                AgentEvent::Done(_) => done = true,
                _ => {}
            }
        }
        let usage = *sink.0.lock().unwrap();
        assert!(done, "agent never reached Done; text so far: {text:?}");
        assert!(
            !text.is_empty(),
            "agent produced no assistant text: {text:?}"
        );
        assert!(
            usage.input > 0,
            "cost sink saw input_tokens>0 from the full live loop, got {}",
            usage.input
        );
        assert!(
            usage.output > 0,
            "cost sink saw output_tokens>0 from the full live loop, got {}",
            usage.output
        );
    }

    #[ignore = "reads the GUI's real ~/.dev-workbench/providers.toml (the only key store); spends real tokens"]
    #[tokio::test]
    async fn build_react_agent_wires_real_gui_provider_to_live_glm_and_maps_wire_events() {
        // The assembly + wire-mapping layers the unit smoke bypasses — driven
        // with a LIVE model so a regression in any of them surfaces here, not
        // only in mock-driven tests. This is the exact path the front-end
        // triggers over IPC, minus the GUI transport (AppHandle/emit, a thin
        // wrapper the project has no Tauri-mock precedent for):
        //   build_react_agent reads the GUI's real providers.toml (the ONLY key
        //   store — no env config), resolve_provider maps the default model,
        //   the agent runs against live GLM, and every AgentEvent flows through
        //   the SAME map_agent_event react_chat_driver serializes to the
        //   agent:event wire the front-end types/index.ts deserializes.
        // Skipped when the GUI provider has no key (CI / fresh install); run on
        // a machine where Settings → Providers holds a keyed GLM entry:
        //   cargo test --lib -- --ignored build_react_agent_wires
        use kernel_core::{Agent, AgentEvent, AgentInput};
        let home = crate::commands::projects::dirs_home();
        let data_dir = home.join(".dev-workbench");
        let has_key = crate::config::providers::load_providers_config(&data_dir)
            .ok()
            .and_then(|c| crate::config::providers::resolve_provider(&c, "glm-4.6"))
            .map(|r| !r.api_key.is_empty())
            .unwrap_or(false);
        if !has_key {
            eprintln!("no keyed GUI provider in {data_dir:?} — skipping live assembly smoke");
            return;
        }
        let agent = crate::kernel_impl::executor::build_react_agent(
            Some("glm-4.6"),
            None,
            ".",
            None,
            Vec::new(),
            None,
            crate::kernel_impl::hooks::PermissionMode::default(),
            None,
            None, // session_id: test agents — traces record with a null session_id
            None, // skill_filter
            None, // mcp_filter
            None, // knowledge_ids
            None, // app — test agents get no WorkflowTool
        )
        .expect("build_react_agent assembles from GUI provider config");
        let mut stream = agent
            .run(AgentInput {
                prompt: "Reply with exactly one word: PONG".into(),
                working_dir: None,
                model: None,
                resume_from: None,
            })
            .expect("agent run starts");
        use futures::StreamExt;
        let mut done = false;
        let mut wire: Vec<crate::agents::pty::ChatStreamEvent> = Vec::new();
        while let Some(ev) = stream.next().await {
            let ev = ev.unwrap();
            if matches!(ev, AgentEvent::Done(_)) {
                done = true;
            }
            wire.extend(crate::agents::react_chat::map_agent_event(ev, 0));
        }
        assert!(
            done,
            "agent never reached Done through the real assembly path"
        );
        assert!(
            !wire.is_empty(),
            "map_agent_event produced no wire events from the live run"
        );
        // The wire events ARE the agent:event payload the front-end
        // types/index.ts deserializes; they must serialize to {kind: ...} JSON
        // (the TS union discriminator), proving map_agent_event output is valid
        // wire — not just valid Rust enums.
        let json = serde_json::to_string(&wire).expect("wire events serialize to agent:event JSON");
        assert!(
            json.contains("\"kind\""),
            "wire payload carries the kind discriminator the TS union narrows on: {json}"
        );
    }

    /// 真HTTP验证修复:GLM在一个turn并行发起2+个tool_use时,第二轮请求经
    /// build_body合并连续Tool Message后不再被provider以400拒绝——会话
    /// 34f2c468的精确失败场景(修复前连续user消息→400;修复后合并成一条
    /// user→通过)。prompt强烈引导并行,但模型是否真并行是自主行为:并行
    /// 则完全复刻34f2c468并验证修复;串行则至少证明真HTTP的tool往返不破。
    ///   cargo test --lib -- --ignored live_glm_parallel_tool_use --nocapture
    #[ignore = "live GLM; needs keyed GUI provider; spends tokens"]
    #[tokio::test]
    async fn live_glm_parallel_tool_use_does_not_400_on_followup_turn() {
        use kernel_core::{Agent, AgentEvent, AgentInput};
        let home = crate::commands::projects::dirs_home();
        let data_dir = home.join(".dev-workbench");
        let has_key = crate::config::providers::load_providers_config(&data_dir)
            .ok()
            .and_then(|c| crate::config::providers::resolve_provider(&c, "glm-4.6"))
            .map(|r| !r.api_key.is_empty())
            .unwrap_or(false);
        if !has_key {
            eprintln!("no keyed GUI provider — skipping live parallel-tool-use smoke");
            return;
        }
        let working_dir = env!("CARGO_MANIFEST_DIR").to_string();
        let agent = crate::kernel_impl::executor::build_react_agent(
            Some("glm-4.6"),
            None,
            &working_dir,
            None,
            Vec::new(),
            None,
            crate::kernel_impl::hooks::PermissionMode::default(),
            None,
            None,
            None, // skill_filter
            None, // mcp_filter
            None, // knowledge_ids
            None, // app — test agents get no WorkflowTool
        )
        .expect("build_react_agent");
        // 强引导并行:"一次性发出两个tool调用,不要分开做"。
        let mut stream = agent
            .run(AgentInput {
                prompt: "Do BOTH in a single response — issue both tool calls together in one turn, do NOT do them one at a time: (1) read_file on Cargo.toml, (2) glob with pattern '*.toml'. Then reply in ONE short sentence with the package name and the count of .toml files.".into(),
                working_dir: Some(working_dir),
                model: None,
                resume_from: None,
            })
            .expect("agent run starts");
        use futures::StreamExt;
        let mut done_status: Option<kernel_core::AgentRunStatus> = None;
        let mut tool_uses_seen = 0usize;
        let mut summary = String::new();
        let mut stream_err = String::new();
        while let Some(ev) = stream.next().await {
            match ev {
                Ok(AgentEvent::Done(o)) => {
                    done_status = Some(o.status);
                    if let Some(s) = o.output_summary {
                        summary = s;
                    }
                }
                Ok(other) => {
                    // 嗅探tool_use wire事件(粗略计数,观察模型用了几个工具)。
                    for w in crate::agents::react_chat::map_agent_event(other, 0) {
                        let s = serde_json::to_string(&w).unwrap_or_default();
                        if s.contains("tool_use") || s.contains("ToolUse") {
                            tool_uses_seen += 1;
                        }
                    }
                }
                Err(e) => stream_err = e.to_string(),
            }
        }
        eprintln!(
            "live parallel-tool-use smoke: status={:?} tool_uses_seen={} summary={:?} err={:?}",
            done_status, tool_uses_seen, summary, stream_err
        );
        assert!(
            stream_err.is_empty(),
            "stream error (possible 400): {stream_err}"
        );
        let status = done_status.expect("agent never reached Done");
        // 环境问题(GLM key失效)≠ 代码问题(400)。has_key 只查 key 非空,
        // 运行时才发现 key 失效——此时优雅跳过,不假装通过;只有真400/
        // 非Completed(非auth原因)才判fail,保留对34f2c468修复的严格断言。
        if matches!(status, kernel_core::AgentRunStatus::Failed)
            && summary.to_lowercase().contains("authentication")
        {
            eprintln!(
                "SKIP: GUI GLM key failed authentication — live e2e needs a valid key. summary: {summary}"
            );
            return;
        }
        assert!(
            matches!(status, kernel_core::AgentRunStatus::Completed),
            "agent did not complete (status={:?}) — parallel tool_use may have 400'd the followup turn. summary: {summary}",
            status
        );
        assert!(
            tool_uses_seen >= 1,
            "no tool_use observed in wire — agent didn't use tools"
        );
    }

    /// 确定性真HTTP验证修复(不依赖模型自主选择并行):手工复刻34f2c468的
    /// history——assistant一个turn发2个并行tool_use,run loop push 2条连续
    /// Tool Message——经build_body合并成一条user后,真POST到GLM endpoint,
    /// 断言provider接受(不再400 "tool_use ids were found without tool_result
    /// blocks immediately after")。key走env(GLM_API_KEY),回退GUI toml,不落盘。
    ///   GLM_API_KEY=... cargo test --lib -- --ignored live_glm_accepts_merged --nocapture
    #[ignore = "live GLM POST; needs GLM_API_KEY or keyed GUI provider; spends tokens"]
    #[tokio::test]
    async fn live_glm_accepts_merged_parallel_tool_use_body() {
        use kernel_core::{FunctionCall, Role, ToolCall};
        // env key优先(不落盘,符合"密钥仅用环境变量"),回退GUI toml。
        let key = std::env::var("GLM_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| {
                let home = crate::commands::projects::dirs_home();
                crate::config::providers::load_providers_config(&home.join(".dev-workbench"))
                    .ok()
                    .and_then(|c| crate::config::providers::resolve_provider(&c, "glm-4.6"))
                    .map(|r| r.api_key)
                    .unwrap_or_default()
            });
        if key.is_empty() {
            eprintln!("SKIP: no GLM_API_KEY env and no keyed GUI provider");
            return;
        }
        // 复刻34f2c468的history:assistant一个turn两个并行tool_use + 两条
        // 连续Tool Message。修复前build_body→两条user→400;修复后→一条user。
        let history = vec![
            Message::user("List the package name and the files."),
            Message {
                role: Role::Assistant,
                content: String::new(),
                tool_calls: vec![
                    ToolCall {
                        id: "call_00".into(),
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: "read_file".into(),
                            arguments: r#"{"file_path":"Cargo.toml"}"#.into(),
                        },
                    },
                    ToolCall {
                        id: "call_01".into(),
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: "glob".into(),
                            arguments: r#"{"pattern":"*.toml"}"#.into(),
                        },
                    },
                ],
                tool_call_id: None,
                reasoning: None,
                reasoning_signature: None,
            },
            Message {
                role: Role::Tool,
                content: "name = \"x\"".into(),
                tool_calls: Vec::new(),
                tool_call_id: Some("call_00".into()),
                reasoning: None,
                reasoning_signature: None,
            },
            Message {
                role: Role::Tool,
                content: "Cargo.toml\ntauri.conf.toml".into(),
                tool_calls: Vec::new(),
                tool_call_id: Some("call_01".into()),
                reasoning: None,
                reasoning_signature: None,
            },
        ];
        let glm = GlmChatModel::bigmodel(&key, "glm-4.6");
        let body = glm.build_body("glm-4.6", &history, &ModelOptions::default(), false);
        // 本地wire合规(合并+严格交替)——复刻的history经修复后必须满足。
        let wire = body["messages"].as_array().unwrap();
        for w in wire.windows(2) {
            assert_ne!(
                w[0]["role"], w[1]["role"],
                "local wire has back-to-back roles"
            );
        }
        // 真POST到GLM。修复前这个body会400;修复后应被接受。
        let client = reqwest::Client::new();
        let resp = client
            .post("https://open.bigmodel.cn/api/anthropic/v1/messages")
            .bearer_auth(&key)
            .json(&body)
            .send()
            .await
            .expect("HTTP send");
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let head = &text[..text.len().min(300)];
        eprintln!(
            "live merged-body POST: status={} resp_head={}",
            status, head
        );
        // 环境问题(key失效)→ 优雅skip,不假装通过。
        if status.as_u16() == 401 || text.contains("authentication") {
            eprintln!("SKIP: key failed authentication: {}", head);
            return;
        }
        // 核心回归断言:绝不再因tool_use/tool_result结构被400(34f2c468 bug)。
        let structure_400 = status.as_u16() == 400
            && (text.contains("tool_result") || text.contains("tool_use ids"));
        assert!(
            !structure_400,
            "REGRESSION: provider 400'd on tool_result structure (the 34f2c468 bug): {} {}",
            status, head
        );
        // 否则期望成功(2xx)。
        assert!(
            status.is_success(),
            "provider rejected merged body (non-auth): {} {}",
            status,
            head
        );
    }

    /// Records a real GLM run's wire events to e2e/fixtures/ so the front-end
    /// Playwright suite renders BlocksView against genuine model output instead
    /// of hand-written mocks. Run once locally with a keyed GUI provider, then
    /// commit the fixture (it carries no credentials):
    ///   cargo test --lib -- --ignored record_real_glm_wire --nocapture
    #[ignore = "writes e2e/fixtures/agent-blocks-real.json; needs keyed GUI provider; spends tokens"]
    #[tokio::test]
    async fn record_real_glm_wire_to_e2e_fixture() {
        use kernel_core::{Agent, AgentInput};
        let home = crate::commands::projects::dirs_home();
        let data_dir = home.join(".dev-workbench");
        let has_key = crate::config::providers::load_providers_config(&data_dir)
            .ok()
            .and_then(|c| crate::config::providers::resolve_provider(&c, "glm-4.6"))
            .map(|r| !r.api_key.is_empty())
            .unwrap_or(false);
        if !has_key {
            eprintln!("no keyed GUI provider — skipping recording");
            return;
        }
        // build_react_agent's default registry wires read_file/glob/grep/bash, so
        // a tool-asking prompt yields real tool_use + tool_result blocks in the
        // wire — the multi-block shape BlocksView must render. Calling
        // build_react_agent directly (not react_chat_driver) skips the shadow-git
        // checkpoint, leaving the working tree untouched.
        let working_dir = env!("CARGO_MANIFEST_DIR").to_string();
        let agent = crate::kernel_impl::executor::build_react_agent(
            Some("glm-4.6"),
            None,
            &working_dir,
            None,
            Vec::new(),
            None,
            crate::kernel_impl::hooks::PermissionMode::default(),
            None,
            None, // session_id: test agents — traces record with a null session_id
            None, // skill_filter
            None, // mcp_filter
            None, // knowledge_ids
            None, // app — test agents get no WorkflowTool
        )
        .expect("build_react_agent");
        let mut stream = agent
            .run(AgentInput {
                prompt: "Use the read_file tool to read Cargo.toml, then reply in one short sentence with the package name.".into(),
                working_dir: Some(working_dir.clone()),
                model: None,
                resume_from: None,
            })
            .expect("agent run starts");
        use futures::StreamExt;
        let mut wire: Vec<crate::agents::pty::ChatStreamEvent> = Vec::new();
        while let Some(ev) = stream.next().await {
            let ev = ev.unwrap();
            wire.extend(crate::agents::react_chat::map_agent_event(ev, 0));
        }
        assert!(!wire.is_empty(), "live run produced no wire events");
        let json = serde_json::to_string_pretty(&wire).expect("serialize wire");
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("e2e")
            .join("fixtures")
            .join("agent-blocks-real.json");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        std::fs::write(&out, &json).unwrap();
        eprintln!("recorded {} wire events to {}", wire.len(), out.display());
    }

    /// DIAG-ONLY (delete after root-causing): DeepSeek deepseek-v4-flash
    /// sessions fail 100% of the time (status=Failed blocks=1 in the app log),
    /// yet an external curl/urllib probe of the same request shape returns 200.
    /// This bypasses the run_loop (which swallows the real error into a generic
    /// "could not be recovered" message) and calls GlmChatModel.stream() with
    /// the real reqwest client, so the raw Error::Model("GLM stream failed: N")
    /// / Error::Network surfaces. The suspect: reqwest's default HTTP/2 ALPN
    /// (vs the probe's HTTP/1.1) breaking DeepSeek's streaming response.
    ///   cargo test --lib -- --ignored diag_deepseek --nocapture
    #[ignore = "diagnostic; needs keyed DeepSeek GUI provider; spends tokens"]
    #[tokio::test]
    async fn diag_deepseek_glm_stream_raw() {
        use futures::StreamExt;
        use kernel_core::{ChatModel, Message, ModelOptions, ThinkingConfig};
        let home = crate::commands::projects::dirs_home();
        let data_dir = home.join(".dev-workbench");
        let r = crate::config::providers::load_providers_config(&data_dir)
            .ok()
            .and_then(|c| crate::config::providers::resolve_provider(&c, "deepseek-v4-flash"))
            .expect("keyed DeepSeek provider");
        eprintln!("endpoint={} model_in_config={}", r.endpoint, r.model);
        let model = GlmChatModel::new(&r.endpoint, &r.api_key, &r.model);
        let msgs = vec![
            Message::system("You are a helpful assistant."),
            Message::user("为什么信息直接失败"),
        ];
        // Mirror executor.rs with_thinking(2048) + build_body's max_tokens floor.
        let opts = ModelOptions {
            model: Some(r.model.clone()),
            thinking: Some(ThinkingConfig {
                budget_tokens: 2048,
            }),
            max_tokens: Some(6144),
            ..Default::default()
        };
        let mut s = match model.stream(&msgs, &opts) {
            Err(e) => {
                eprintln!("!!! stream() returned Err before first poll: {e}");
                return;
            }
            Ok(s) => s,
        };
        let mut i = 0usize;
        while let Some(item) = s.next().await {
            match item {
                Ok(m) => eprintln!(
                    "[{i}] Ok role={:?} content({}) reasoning({}) tools({}) sig={}",
                    m.role,
                    m.content.len(),
                    m.reasoning.as_deref().unwrap_or("").len(),
                    m.tool_calls.len(),
                    m.reasoning_signature.as_deref().unwrap_or("")
                ),
                Err(e) => {
                    eprintln!("!!! [{i}] Err FROM STREAM: {e}");
                    break;
                }
            }
            i += 1;
        }
        eprintln!("=== deepseek stream consumed after {i} items ===");
    }

    /// DIAG-ONLY: same model via the full build_react_agent → agent.run path.
    /// If diag_deepseek_glm_stream_raw succeeds but this fails, the regression
    /// is in the run_loop layer (thinking replay / opts wiring), not the HTTP
    /// layer.
    #[ignore = "diagnostic; needs keyed DeepSeek GUI provider; spends tokens"]
    #[tokio::test]
    async fn diag_deepseek_agent_run() {
        use futures::StreamExt;
        use kernel_core::{Agent, AgentInput};
        let working_dir = env!("CARGO_MANIFEST_DIR").to_string();
        let agent = crate::kernel_impl::executor::build_react_agent(
            Some("deepseek-v4-flash"),
            None,
            &working_dir,
            None,
            Vec::new(),
            None,
            crate::kernel_impl::hooks::PermissionMode::default(),
            None,
            None, // session_id: test agents — traces record with a null session_id
            None, // skill_filter
            None, // mcp_filter
            None, // knowledge_ids
            None, // app — test agents get no WorkflowTool
        )
        .expect("build_react_agent");
        let mut stream = agent
            .run(AgentInput {
                prompt: "为什么信息直接失败".into(),
                working_dir: Some(working_dir.clone()),
                model: Some("deepseek-v4-flash".into()),
                resume_from: None,
            })
            .expect("agent run starts");
        let mut i = 0usize;
        while let Some(ev) = stream.next().await {
            let ev = ev.unwrap();
            match &ev {
                kernel_core::AgentEvent::Done(o) => {
                    eprintln!(
                        "[{i}] Done status={:?} summary={:?}",
                        o.status, o.output_summary
                    );
                }
                kernel_core::AgentEvent::Token(t) => {
                    eprintln!("[{i}] Token({} chars)", t.len());
                }
                kernel_core::AgentEvent::Reasoning(t) => {
                    eprintln!("[{i}] Reasoning({} chars)", t.len());
                }
                other => eprintln!("[{i}] {other:?}"),
            }
            i += 1;
        }
        eprintln!("=== agent.run stream consumed after {i} events ===");
    }
}
