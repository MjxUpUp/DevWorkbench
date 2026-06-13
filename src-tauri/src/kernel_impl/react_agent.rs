//! Transparent ReactAgent + GLM ChatModel + ToolRegistry.
//!
//! The "transparent" agent: the kernel controls the LLM call AND the tool loop
//! directly (eino `adk/react.go` Rust port). Used for kernel-internal tasks and
//! as a self-built agent that can call MCP tools and Skills.
//!
//! Three pieces:
//! - [`GlmChatModel`]: `ChatModel` impl calling Zhipu GLM via Anthropic API.
//! - [`ToolRegistry`]: a cloneable collection of `dyn Tool` (MCP + Skill + builtin).
//! - [`ReactAgent`]: reason→act→observe loop, bounded by max_steps, implements
//!   `kernel_core::Agent`. Now executes tool calls for real.

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use kernel_core::{
    AgentCaps, AgentError, AgentEvent, AgentInput, AgentKind, AgentOutcome, AgentRunStatus,
    ChatModel, Error, Message, MessageStream, ModelOptions, Role, Tool, ToolContext, ToolInfo,
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

    fn build_body(&self, model: &str, messages: &[Message], opts: &ModelOptions, stream: bool) -> Value {
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
    async fn generate(&self, messages: &[Message], opts: &ModelOptions) -> Result<Message, Error> {
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
// ToolRegistry — cloneable collection of tools (MCP + Skill + builtin)
// ---------------------------------------------------------------------------

/// A cloneable registry of tools. Wraps each tool in `Arc` so the registry
/// itself can be cheaply cloned into an async stream.
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

    /// Add a tool from an already-erased `Box<dyn Tool>` (e.g. an McpTool).
    pub fn push_boxed(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(boxed_tool_to_arc(tool));
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

/// Convert a boxed tool into an Arc tool. (Box → Arc requires a thin owning
/// wrapper since `Box<dyn Trait>` can't be turned into `Arc<dyn Trait>` directly.)
fn boxed_tool_to_arc(tool: Box<dyn Tool>) -> Arc<dyn Tool> {
    struct ToolBox(Box<dyn Tool>);
    #[async_trait]
    impl Tool for ToolBox {
        fn info(&self) -> ToolInfo {
            self.0.info()
        }
        async fn invoke(&self, args: &str, ctx: &ToolContext) -> Result<String, Error> {
            self.0.invoke(args, ctx).await
        }
        fn is_dangerous(&self) -> bool {
            self.0.is_dangerous()
        }
        fn is_read_only(&self) -> bool {
            self.0.is_read_only()
        }
    }
    Arc::new(ToolBox(tool))
}

// ---------------------------------------------------------------------------
// ReactAgent — the transparent reason→act→observe loop
// ---------------------------------------------------------------------------

/// A transparent agent: a ChatModel + a registry of tools, looped up to
/// max_steps. Holds the model as `Arc<dyn ChatModel>` so the agent is testable
/// with a mock model and the loop can be driven into an async stream.
pub struct ReactAgent {
    model: Arc<dyn ChatModel>,
    tools: ToolRegistry,
    max_steps: usize,
    system_prompt: String,
}

impl ReactAgent {
    /// Build from any ChatModel (GLM, a mock, a local model, …).
    pub fn new(
        model: impl ChatModel + 'static,
        tools: ToolRegistry,
        system_prompt: impl Into<String>,
    ) -> Self {
        Self {
            model: Arc::new(model),
            tools,
            max_steps: 12,
            system_prompt: system_prompt.into(),
        }
    }

    /// Build from an already-erased boxed model.
    pub fn new_boxed(
        model: Box<dyn ChatModel>,
        tools: ToolRegistry,
        system_prompt: impl Into<String>,
    ) -> Self {
        Self {
            model: boxed_chatmodel_to_arc(model),
            tools,
            max_steps: 12,
            system_prompt: system_prompt.into(),
        }
    }

    pub fn with_max_steps(mut self, n: usize) -> Self {
        self.max_steps = n;
        self
    }

    /// Run the loop to completion, returning the final answer text.
    pub async fn run_loop(&self, task: &str, opts: ModelOptions) -> Result<String, Error> {
        let mut history = vec![Message::system(&self.system_prompt), Message::user(task)];
        for _step in 0..self.max_steps {
            let resp = self.model.generate(&history, &opts).await?;
            history.push(resp.clone());
            if resp.tool_calls.is_empty() {
                return Ok(resp.content);
            }
            for call in &resp.tool_calls {
                let result = match self.tools.find(&call.function.name) {
                    Some(t) => {
                        t.invoke(&call.function.arguments, &ToolContext::default())
                            .await
                            .unwrap_or_else(|e| format!("[tool error: {e}]"))
                    }
                    None => format!("[unknown tool: {}]", call.function.name),
                };
                history.push(Message {
                    role: Role::Tool,
                    content: result,
                    tool_calls: Vec::new(),
                    tool_call_id: Some(call.id.clone()),
                    reasoning: None,
                });
            }
        }
        Err(Error::Agent(format!(
            "ReactAgent exceeded {} steps without a final answer",
            self.max_steps
        )))
    }
}

/// Box<dyn ChatModel> → Arc<dyn ChatModel> (same owning-wrapper trick as tools).
fn boxed_chatmodel_to_arc(model: Box<dyn ChatModel>) -> Arc<dyn ChatModel> {
    struct ModelBox(Box<dyn ChatModel>);
    #[async_trait]
    impl ChatModel for ModelBox {
        async fn generate(&self, m: &[Message], o: &ModelOptions) -> Result<Message, Error> {
            self.0.generate(m, o).await
        }
        fn stream(&self, m: &[Message], o: &ModelOptions) -> Result<MessageStream, Error> {
            self.0.stream(m, o)
        }
        fn with_tools(&self, tools: &[ToolInfo]) -> Result<Box<dyn ChatModel>, Error> {
            self.0.with_tools(tools)
        }
    }
    Arc::new(ModelBox(model))
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

    fn run(&self, input: AgentInput) -> Result<BoxStream<'static, Result<AgentEvent, AgentError>>, AgentError> {
        let model = Arc::clone(&self.model);
        let tools = self.tools.clone();
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
                // Execute tool calls (the reason→act→observe loop).
                for call in &resp.tool_calls {
                    yield AgentEvent::ToolCall(kernel_core::ToolCallEvent {
                        tool: call.function.name.clone(),
                        arguments: call.function.arguments.clone(),
                        status: kernel_core::ToolCallStatus::Started,
                    });
                    let result = match tools.find(&call.function.name) {
                        Some(t) => match t.invoke(&call.function.arguments, &ToolContext::default()).await {
                            Ok(out) => {
                                yield AgentEvent::ToolCall(kernel_core::ToolCallEvent {
                                    tool: call.function.name.clone(),
                                    arguments: call.function.arguments.clone(),
                                    status: kernel_core::ToolCallStatus::Succeeded,
                                });
                                out
                            }
                            Err(e) => {
                                yield AgentEvent::ToolCall(kernel_core::ToolCallEvent {
                                    tool: call.function.name.clone(),
                                    arguments: call.function.arguments.clone(),
                                    status: kernel_core::ToolCallStatus::Failed,
                                });
                                format!("[tool error: {e}]")
                            }
                        },
                        None => format!("[unknown tool: {}]", call.function.name),
                    };
                    history.push(Message {
                        role: Role::Tool,
                        content: result,
                        tool_calls: Vec::new(),
                        tool_call_id: Some(call.id.clone()),
                        reasoning: None,
                    });
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
    use kernel_core::ToolInfo;

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

    /// A deterministic in-memory tool for testing the loop without network.
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
}
