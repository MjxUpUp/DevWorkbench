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

/// A1 (OTel span tree): the span context a ChatModel attributes every LLM call
/// to. One per agent instance — `span_id` groups all calls this model makes
/// into one trace-tree node; `parent_span_id` is the orchestrating agent's
/// span (None for the root). Set at agent construction (`SpanContext::root`)
/// and at fork (`SpanContext::child_of`) so the agent-DAG nesting (main →
/// subagent) surfaces in TraceView. All-None = no span context (ad-hoc/test
/// agents) — recorded as NULL, an honest absence rather than a faked root.
#[derive(Debug, Clone, Default)]
pub struct SpanContext {
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub span_name: Option<String>,
}

impl SpanContext {
    /// Root span for a top-level agent (no parent). `name` labels the node in
    /// the trace tree (e.g. "agent").
    pub fn root(name: &str) -> Self {
        Self {
            span_id: Some(uuid::Uuid::new_v4().to_string()),
            parent_span_id: None,
            span_name: Some(name.to_string()),
        }
    }

    /// Child span under `parent_span_id` — used at fork so a sub-agent's calls
    /// nest under the orchestrating agent in the tree. `parent_span_id` = None
    /// (the parent carried no span) yields a rootless child: still attributed
    /// to its own span, just not nested (honest, not faked).
    pub fn child_of(parent_span_id: Option<&str>, name: &str) -> Self {
        Self {
            span_id: Some(uuid::Uuid::new_v4().to_string()),
            parent_span_id: parent_span_id.map(str::to_string),
            span_name: Some(name.to_string()),
        }
    }
}

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
    /// A1 (OTel span tree): every LLM call this model makes is attributed to
    /// this span. Default = no span context (ad-hoc/test); production agents
    /// set a root span at construction and a child span at fork.
    pub span: SpanContext,
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
            span: SpanContext::default(),
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

    /// Attach a span context (A1 OTel span tree). Every LLM call this model
    /// makes is then attributed to `span.span_id` with `span.parent_span_id`,
    /// so TraceView renders the agent-DAG nesting. Used at agent construction
    /// (root span) and at fork (child span under the orchestrator).
    pub fn with_span(mut self, span: SpanContext) -> Self {
        self.span = span;
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
                    span_id: self.span.span_id.clone(),
                    parent_span_id: self.span.parent_span_id.clone(),
                    span_name: self.span.span_name.clone(),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A TraceSink that captures every (session_id, LlmTrace) pair so a test can
    /// assert exactly what record_trace stamped — the production DbTraceSink
    /// fire-and-forgets into SQLite, which would make the span attribution
    /// unverifiable without a DB round-trip.
    struct CapturingSink(Mutex<Vec<(Option<String>, LlmTrace)>>);
    impl TraceSink for CapturingSink {
        fn record_llm_call(&self, session_id: Option<&str>, trace: LlmTrace) {
            self.0
                .lock()
                .unwrap()
                .push((session_id.map(String::from), trace));
        }
    }

    #[test]
    fn span_context_root_has_no_parent() {
        let s = SpanContext::root("agent");
        assert!(s.span_id.is_some(), "root gets a fresh span_id");
        assert!(s.parent_span_id.is_none(), "root has no parent");
        assert_eq!(s.span_name.as_deref(), Some("agent"));
    }

    #[test]
    fn span_context_child_of_nests_under_parent() {
        let s = SpanContext::child_of(Some("span-root"), "subagent");
        assert!(s.span_id.is_some());
        assert_ne!(
            s.span_id.as_deref(),
            Some("span-root"),
            "child must get its OWN span_id, not reuse the parent's"
        );
        assert_eq!(s.parent_span_id.as_deref(), Some("span-root"));
        assert_eq!(s.span_name.as_deref(), Some("subagent"));
    }

    #[test]
    fn span_context_child_of_none_is_rootless_not_faked() {
        // A fork from an ad-hoc/test parent (no span) yields a child with its
        // own span but no parent — an honest rootless node, not a faked root.
        let s = SpanContext::child_of(None, "subagent");
        assert!(s.span_id.is_some());
        assert!(s.parent_span_id.is_none());
    }

    #[test]
    fn span_context_default_is_all_none() {
        let s = SpanContext::default();
        assert!(s.span_id.is_none());
        assert!(s.parent_span_id.is_none());
        assert!(s.span_name.is_none());
    }

    /// The core A1 attribution contract: record_trace stamps the model's span
    /// context onto the LlmTrace it hands the sink, so every call this agent
    /// makes lands in the trace tree under its span. Without this the span
    /// columns would always be NULL — the columns exist but carry nothing.
    #[test]
    fn record_trace_stamps_span_context_onto_trace() {
        let inner = Arc::new(CapturingSink(Mutex::new(vec![])));
        let sink: Arc<dyn TraceSink> = inner.clone();
        let shared = ChatModelShared::new("https://x", "k", "m")
            .with_session_id(Some("sess-1".into()))
            .with_trace_sink(sink)
            .with_span(SpanContext {
                span_id: Some("span-agent".into()),
                parent_span_id: None,
                span_name: Some("agent".into()),
            });
        shared.record_trace("glm-4.6", Some(200), None, "{}", None, 5, None, None, None, None);
        let captured = inner.0.lock().unwrap();
        assert_eq!(captured.len(), 1, "exactly one trace recorded");
        let (sid, trace) = &captured[0];
        assert_eq!(sid.as_deref(), Some("sess-1"));
        assert_eq!(trace.span_id.as_deref(), Some("span-agent"));
        assert!(trace.parent_span_id.is_none());
        assert_eq!(trace.span_name.as_deref(), Some("agent"));
    }

    /// An ad-hoc model (no span attached) records span_id = NULL — honest
    /// absence, not a faked root span. Guards against record_trace inventing a
    /// span where none was set.
    #[test]
    fn record_trace_leaves_span_null_when_no_span_attached() {
        let inner = Arc::new(CapturingSink(Mutex::new(vec![])));
        let sink: Arc<dyn TraceSink> = inner.clone();
        let shared = ChatModelShared::new("https://x", "k", "m").with_trace_sink(sink);
        shared.record_trace("m", None, Some("network"), "{}", None, 1, None, None, None, None);
        let captured = inner.0.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert!(captured[0].1.span_id.is_none());
        assert!(captured[0].1.parent_span_id.is_none());
        assert!(captured[0].1.span_name.is_none());
    }
}
