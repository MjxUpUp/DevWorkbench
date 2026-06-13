//! Transparent ReactAgent + GLM ChatModel.
//!
//! The "transparent" agent: the kernel controls the LLM call and the tool loop
//! directly (eino `adk/react.go` Rust port, simplified). Used for kernel-internal
//! tasks (routing, summarization, honesty analysis) as opposed to opaque CLI agents.
//!
//! - [`GlmChatModel`]: `ChatModel` impl calling Zhipu GLM via the Anthropic-
//!   compatible endpoint (the user's configured backend).
//! - [`ReactAgent`]: reason→act loop bounded by max_steps, implements
//!   `kernel_core::Agent`.

use async_trait::async_trait;
use futures::stream::BoxStream;
use kernel_core::{
    AgentCaps, AgentError, AgentEvent, AgentInput, AgentKind, AgentOutcome, AgentRunStatus,
    ChatModel, Error, Message, MessageStream, ModelOptions, Role, Tool,
};
use serde_json::{json, Value};

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
            client: reqwest::Client::new(),
        }
    }

    /// From the user's standard backend (open.bigmodel.cn).
    pub fn bigmodel(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self::new("https://open.bigmodel.cn/api/anthropic", api_key, model)
    }

    fn build_body(
        &self,
        model: &str,
        messages: &[Message],
        opts: &ModelOptions,
        stream: bool,
    ) -> Value {
        let msgs: Vec<Value> = messages
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| {
                json!({
                    "role": match m.role { Role::User => "user", _ => "assistant" },
                    "content": m.content,
                })
            })
            .collect();
        let system: String = messages
            .iter()
            .filter(|m| m.role == Role::System)
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
            .join("\n");
        let mut body = json!({
            "model": model,
            "messages": msgs,
            "max_tokens": opts.max_tokens.unwrap_or(4096),
            "stream": stream,
        });
        if !system.is_empty() {
            body["system"] = Value::String(system);
        }
        if let Some(t) = opts.temperature {
            body["temperature"] = json!(t);
        }
        body
    }
}

#[async_trait]
impl ChatModel for GlmChatModel {
    async fn generate(
        &self,
        messages: &[Message],
        opts: &ModelOptions,
    ) -> Result<Message, Error> {
        let model = opts.model.clone().unwrap_or_else(|| self.model.clone());
        let body = self.build_body(&model, messages, opts, false);
        let resp = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::Model(format!("GLM {status}: {text}")));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| Error::Model(format!("decode: {e}")))?;
        decode_anthropic_message(&v)
    }

    fn stream(&self, messages: &[Message], opts: &ModelOptions) -> Result<MessageStream, Error> {
        // PoC: yield the full message in one chunk. True SSE token streaming is
        // a TODO for latency-critical paths.
        let model = self.clone();
        let messages = messages.to_vec();
        let opts = opts.clone();
        let s = async_stream::try_stream! {
            let msg = model.generate(&messages, &opts).await?;
            yield msg;
        };
        Ok(Box::pin(s))
    }
}

/// Decode an Anthropic-format response into our Message.
fn decode_anthropic_message(v: &Value) -> Result<Message, Error> {
    let content = v
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| {
            arr.iter()
                .filter_map(|block| {
                    if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                        block.get("text").and_then(|t| t.as_str()).map(String::from)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .first()
                .cloned()
        })
        .unwrap_or_default();
    Ok(Message::assistant(content))
}

// ---------------------------------------------------------------------------
// ReactAgent — the transparent reason→act loop
// ---------------------------------------------------------------------------

/// A transparent agent: ChatModel looped up to max_steps.
///
/// (Tool integration is the next increment: the kernel-core `Tool` trait is in
/// place; ReactAgent will accept `Vec<Box<dyn Tool>>` and run a full reason→act
/// loop once a tool-bearing PoC task is wired. For now it performs single-turn
/// generation, which is what routing/summarization/honesty-analysis tasks need.)
pub struct ReactAgent {
    model: GlmChatModel,
    max_steps: usize,
    system_prompt: String,
}

impl ReactAgent {
    pub fn new(
        model: GlmChatModel,
        _tools: Vec<Box<dyn Tool>>,
        system_prompt: impl Into<String>,
    ) -> Self {
        // _tools accepted for API forward-compat (callers pass a toolset); the
        // field will be stored once the tool loop is implemented.
        Self {
            model,
            max_steps: 12,
            system_prompt: system_prompt.into(),
        }
    }

    pub fn with_max_steps(mut self, n: usize) -> Self {
        self.max_steps = n;
        self
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
            read_only: false,
        }
    }

    fn run(
        &self,
        input: AgentInput,
    ) -> Result<BoxStream<'static, Result<AgentEvent, AgentError>>, AgentError> {
        let model = self.model.clone();
        let system_prompt = self.system_prompt.clone();
        let max_steps = self.max_steps;
        let task = input.prompt;
        let model_opt = input.model;

        let s = async_stream::try_stream! {
            let mut history = vec![Message::system(&system_prompt), Message::user(&task)];
            let opts = ModelOptions { model: model_opt, ..Default::default() };
            let mut final_output = String::new();

            for step in 0..max_steps {
                let resp = model.generate(&history, &opts).await.map_err(AgentError::from)?;
                history.push(resp.clone());
                if !resp.content.is_empty() {
                    yield AgentEvent::Token(resp.content.clone());
                }
                if resp.tool_calls.is_empty() {
                    final_output = resp.content;
                    yield AgentEvent::TurnBoundary;
                    break;
                }
                let _ = step;
            }
            yield AgentEvent::Done(AgentOutcome {
                status: AgentRunStatus::Completed,
                files_changed: Vec::new(),
                exit_code: Some(0),
                output_summary: Some(final_output),
            });
        };
        Ok(Box::pin(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_anthropic_text_block() {
        let v = json!({ "content": [ {"type": "text", "text": "hello"} ] });
        let m = decode_anthropic_message(&v).unwrap();
        assert_eq!(m.content, "hello");
        assert_eq!(m.role, Role::Assistant);
    }

    #[test]
    fn build_body_separates_system_from_user() {
        let model = GlmChatModel::bigmodel("k", "glm-4.6");
        let body = model.build_body(
            "glm-4.6",
            &[Message::system("be brief"), Message::user("hi")],
            &ModelOptions::default(),
            false,
        );
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["model"], "glm-4.6");
        assert_eq!(body["stream"], false);
    }

    #[test]
    fn glm_model_is_clone() {
        let m = GlmChatModel::bigmodel("k", "glm-4.6");
        let _m2 = m.clone();
    }
}
