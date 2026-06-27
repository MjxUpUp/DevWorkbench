//! Cross-cutting state shared by every concrete `ChatModel` implementation.
//!
//! The HTTP client, credential, circuit breaker, cost/trace sinks, timing
//! checker, session attribution, and bound tools are **protocol-agnostic** —
//! they have nothing to do with whether the wire is Anthropic Messages or
//! OpenAI Chat Completions. Extracting them here lets each protocol's
//! `ChatModel` impl stay focused on its own `build_body` / `generate` /
//! `stream` / `decode` shape, while delegating the cross-cutting bookkeeping
//! (circuit gate, trace recording) to one place. A protocol impl embeds a
//! `ChatModelShared` and calls `admit_or_err()` / `record_trace(...)` on it.

use std::sync::Arc;

use kernel_core::{Error, ToolInfo};

use crate::cost::circuit_breaker::CircuitBreaker;
use crate::cost::sink::CostSink;
use crate::trace::{LlmTrace, TimingChecker, TraceSink};

/// Cross-cutting state shared by every concrete `ChatModel`: HTTP client,
/// credential, circuit breaker, cost/trace sinks, timing + session attribution,
/// and the bound tools (tools bind identically at the trait level; only the
/// wire serialization differs per protocol). Owns NOTHING about Anthropic vs
/// OpenAI wire shape.
#[derive(Clone)]
pub struct ChatModelShared {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub client: reqwest::Client,
    /// Shared upstream circuit breaker. When Some, every request is gated +
    /// its outcome recorded, so a failing endpoint trips open instead of every
    /// agent turn hammering it. None = unprotected (tests / offline).
    pub circuit: Option<Arc<CircuitBreaker>>,
    /// Optional cost sink — records token usage + cost per completed request.
    /// None = untracked (tests / ad-hoc agents without a session).
    pub cost_sink: Option<Arc<dyn CostSink>>,
    /// Optional trace sink — records the request/response of every LLM HTTP
    /// call to `llm_traces`. None = untraced (tests / ad-hoc agents).
    pub trace_sink: Option<Arc<dyn TraceSink>>,
    /// Optional timing checker — flags slow LLM turns (total latency or
    /// time-to-first-byte over threshold) so a hung/stalled model call is
    /// surfaced as a warn log. None = unchecked (tests / ad-hoc agents).
    pub timing_checker: Option<Arc<TimingChecker>>,
    /// The session id this model is serving (for trace attribution). Set by
    /// build_react_agent from the driver's session id; None for ad-hoc/test
    /// agents.
    pub session_id: Option<String>,
    /// Tools bound via `with_tools` — serialized to the wire differently per
    /// protocol, but held here so the trait's `with_tools` clone-and-swap is
    /// shared.
    pub bound_tools: Vec<ToolInfo>,
}

impl ChatModelShared {
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
            circuit: None,
            cost_sink: None,
            trace_sink: None,
            timing_checker: None,
            session_id: None,
            bound_tools: Vec::new(),
        }
    }

    /// Attach a shared circuit breaker. The same breaker should be shared
    /// across every ChatModel instance that targets the same upstream, so a
    /// trip in one agent is observed by all (see `shared_anthropic_circuit` /
    /// `shared_openai_circuit`).
    pub fn with_circuit(mut self, circuit: Arc<CircuitBreaker>) -> Self {
        self.circuit = Some(circuit);
        self
    }

    pub fn with_cost_sink(mut self, sink: Arc<dyn CostSink>) -> Self {
        self.cost_sink = Some(sink);
        self
    }

    pub fn with_trace_sink(mut self, sink: Arc<dyn TraceSink>) -> Self {
        self.trace_sink = Some(sink);
        self
    }

    pub fn with_timing_checker(mut self, checker: Arc<TimingChecker>) -> Self {
        self.timing_checker = Some(checker);
        self
    }

    pub fn with_session_id(mut self, session_id: Option<String>) -> Self {
        self.session_id = session_id;
        self
    }

    /// Circuit-breaker admission gate. Returns `Err` (circuit open) when the
    /// upstream is tripped; otherwise `Ok`. The caller still records the
    /// outcome (`record_success` / `record_failure` / `record_probe_inconclusive`)
    /// on the same breaker after the call resolves — gate + outcome are a pair.
    pub fn admit_or_err(&self) -> Result<(), Error> {
        if let Some(cb) = &self.circuit {
            if !cb.try_admit(&self.base_url) {
                return Err(Error::Model(format!(
                    "upstream circuit open: {}",
                    self.base_url
                )));
            }
        }
        Ok(())
    }

    /// Record one LLM call to the trace sink (if attached). Centralizes
    /// `LlmTrace` construction so generate/stream stay readable; no-op when no
    /// sink is attached (tests / ad-hoc agents). Also runs the attached
    /// TimingChecker so a slow turn is flagged at warn (independent of whether
    /// a trace sink is attached — timing health surfaces either way).
    ///
    /// `ttfb_ms` = request-send → first response signal (None when the call
    /// never reached a first byte). `stream_ms` = first-byte → completion (None
    /// when there was no streaming/output phase).
    #[allow(clippy::too_many_arguments)]
    pub fn record_trace(
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
        // Flag slow turns before persisting. Checked regardless of the sink
        // (timing health is observability any attached agent should get).
        if let Some(checker) = &self.timing_checker {
            if let Some(w) = checker.check(latency_ms, ttfb_ms) {
                log::warn!("[timing] {model} {}: {}", w.kind, w.message);
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
}
