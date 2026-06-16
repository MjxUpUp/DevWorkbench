//! End-to-end test: self-built ReactAgent with Skills + builtin tools + Hooks.
//!
//! No network: a scripted MockChatModel drives the reason->act->observe loop.
//! Verifies the full self-agent stack: ToolRegistry, SkillTool loading from
//! disk, hook interception, and the loop producing tool-call events.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use kernel_core::{
    Agent, AgentEvent, AgentInput, ChatModel, Error, FunctionCall, Message, MessageStream,
    ModelOptions, Role, Tool, ToolCall, ToolCallStatus, ToolContext, ToolInfo,
};
use serde_json::json;

use app_lib::kernel_impl::react_agent::{ReactAgent, ToolRegistry};
use app_lib::kernel_impl::skill_tool::SkillTool;

struct MockChatModel {
    script: Arc<Mutex<Vec<Message>>>,
}

impl MockChatModel {
    fn new(script: Vec<Message>) -> Self {
        Self { script: Arc::new(Mutex::new(script)) }
    }
}

#[async_trait]
impl ChatModel for MockChatModel {
    async fn generate(&self, _messages: &[Message], _opts: &ModelOptions) -> Result<Message, Error> {
        // run() now drives stream(); generate() is kept for trait completeness.
        let mut script = self.script.lock().unwrap();
        if script.is_empty() {
            return Ok(Message::assistant("[script exhausted]"));
        }
        Ok(script.remove(0))
    }
    fn stream(&self, _m: &[Message], _o: &ModelOptions) -> Result<MessageStream, Error> {
        // One stream() call serves one turn: pop the next scripted Message and
        // yield it. Mirrors GlmChatModel::stream's terminal shape (a Message
        // carrying that turn's text + tool_calls) so the run loop exercises the
        // same accumulate-then-dispatch path as real streaming.
        let script = Arc::clone(&self.script);
        let s = async_stream::try_stream! {
            let msg = {
                let mut g = script.lock().unwrap();
                if g.is_empty() {
                    Message {
                        role: Role::Assistant,
                        content: String::new(),
                        tool_calls: Vec::new(),
                        tool_call_id: None,
                        reasoning: None,
                    }
                } else {
                    g.remove(0)
                }
            };
            yield msg;
        };
        Ok(Box::pin(s))
    }
}

/// A streaming mock: each stream() call yields a whole turn's worth of Messages
/// (multiple text deltas + a terminal tool_calls Message). This is the exact
/// shape GlmChatModel::stream produces from Anthropic SSE, so it exercises the
/// F2.2 run loop's real-streaming path — one Token per delta, tool loop on the
/// terminal tool_calls — without any network.
struct StreamingChatModel {
    turns: Arc<Mutex<VecDeque<Vec<Message>>>>,
}

impl StreamingChatModel {
    fn new(turns: Vec<Vec<Message>>) -> Self {
        Self { turns: Arc::new(Mutex::new(turns.into())) }
    }
}

#[async_trait]
impl ChatModel for StreamingChatModel {
    async fn generate(&self, _m: &[Message], _o: &ModelOptions) -> Result<Message, Error> {
        Err(Error::Unsupported("streaming mock has no generate".into()))
    }
    fn stream(&self, _m: &[Message], _o: &ModelOptions) -> Result<MessageStream, Error> {
        let turns = Arc::clone(&self.turns);
        let s = async_stream::try_stream! {
            let next = { turns.lock().unwrap().pop_front() };
            if let Some(msgs) = next {
                for m in msgs {
                    yield m;
                }
            }
        };
        Ok(Box::pin(s))
    }
}

/// A capture mock: records the EXACT `&[Message]` history its first `stream()`
/// call receives, then yields one terminal assistant message so `run()`
/// finishes in a single step. Used to verify `with_history` splices prior turns
/// into the model's input in the right place/order.
struct CapturingChatModel {
    seen: Arc<Mutex<Vec<Message>>>,
}

#[async_trait]
impl ChatModel for CapturingChatModel {
    async fn generate(&self, _m: &[Message], _o: &ModelOptions) -> Result<Message, Error> {
        Err(Error::Unsupported("capture mock has no generate".into()))
    }
    fn stream(&self, m: &[Message], _o: &ModelOptions) -> Result<MessageStream, Error> {
        // Record the full history the run loop handed us — this is the assertion
        // target for the resume test.
        *self.seen.lock().unwrap() = m.to_vec();
        let s = async_stream::try_stream! {
            yield Message::assistant("turn2 reply");
        };
        Ok(Box::pin(s))
    }
}

fn tool_call(id: &str, name: &str, args: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        call_type: "function".into(),
        function: FunctionCall { name: name.into(), arguments: args.into() },
    }
}

struct CountFilesTool {
    #[allow(dead_code)]
    calls: AtomicUsize,
}

#[async_trait]
impl Tool for CountFilesTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "count_files".into(),
            description: "count files in a directory".into(),
            parameters_schema: json!({"type":"object","properties":{"dir":{"type":"string"}}}),
        }
    }
    async fn invoke(&self, _args: &str, _ctx: &ToolContext) -> Result<String, Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok("3 files found".into())
    }
    fn is_read_only(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn react_agent_runs_skill_and_builtin_tool_then_answers() {
    let builtin = CountFilesTool { calls: AtomicUsize::new(0) };

    let tmp = tempfile::tempdir().unwrap();
    let skill_dir = tmp.path().join("my-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let skill_md = skill_dir.join("SKILL.md");
    std::fs::write(
        &skill_md,
        "---\nname: my-skill\ndescription: \"Use when: counting things.\"\n---\n\n# How to count\n\nBe precise.\n",
    ).unwrap();
    let skill_tool = SkillTool::parse_file(&skill_md).unwrap();

    let registry = ToolRegistry::new().with(builtin).with(skill_tool);
    assert_eq!(registry.len(), 2);
    assert!(registry.find("count_files").is_some());
    assert!(registry.find("skill__my-skill").is_some());

    let script = vec![
        Message {
            role: Role::Assistant,
            content: "Let me count first.".into(),
            tool_calls: vec![tool_call("c1", "count_files", r#"{"dir":"."}"#)],
            tool_call_id: None,
            reasoning: None,
        },
        Message {
            role: Role::Assistant,
            content: "Now let me read the skill.".into(),
            tool_calls: vec![tool_call("c2", "skill__my-skill", "{}")],
            tool_call_id: None,
            reasoning: None,
        },
        Message::assistant("Done. Found 3 files using the counting procedure."),
    ];

    let agent = ReactAgent::new(MockChatModel::new(script), registry, "You are a helpful agent.");

    let mut events = vec![];
    let stream = agent.run(AgentInput {
        prompt: "count and report".into(),
        working_dir: None,
        model: None,
        resume_from: None,
    }).unwrap();
    use futures::StreamExt;
    let mut s = stream;
    while let Some(ev) = s.next().await {
        events.push(ev.unwrap());
    }

    let tool_events: Vec<String> = events.iter().filter_map(|e| match e {
        AgentEvent::ToolCall(t) => Some(t.tool.clone()),
        _ => None,
    }).collect();
    assert!(tool_events.iter().any(|t| t == "count_files"), "got {tool_events:?}");
    assert!(tool_events.iter().any(|t| t == "skill__my-skill"), "got {tool_events:?}");
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Done(_))), "missing Done");
}

#[tokio::test]
async fn skill_tool_invoke_returns_body() {
    let tmp = tempfile::tempdir().unwrap();
    let skill_md = tmp.path().join("SKILL.md");
    std::fs::write(&skill_md, "---\nname: x\ndescription: y\n---\n\nProcedure body here.\n").unwrap();
    let t = SkillTool::parse_file(&skill_md).unwrap();
    let out = t.invoke("", &ToolContext::default()).await.unwrap();
    assert!(out.contains("Procedure body here."));
}

#[tokio::test]
async fn hook_manager_blocks_dangerous_command() {
    use app_lib::kernel_impl::hooks::{Action, CommandGuardHook, HookManager};
    let mut mgr = HookManager::new();
    mgr.register(Box::new(CommandGuardHook::default()));
    let action = Action::RunCommand { command: "rm -rf /home".into() };
    let result = mgr.before(&action).await;
    assert!(result.is_err(), "rm -rf must be blocked");
}

#[tokio::test]
async fn hook_manager_reports_assertion_weakening() {
    use app_lib::kernel_impl::hooks::{Action, ActionOutcome, AssertionGuardHook, HookManager};
    let mut mgr = HookManager::new();
    mgr.register(Box::new(AssertionGuardHook));
    let diff = "--- a/t.rs\n+++ b/t.rs\n-x\n-t.Fatal(\"boom\")\n+x\n+t.Log(\"boom\")\n";
    let outcome = ActionOutcome {
        action: Action::WriteFile { path: "t.rs".into(), content_preview: "".into() },
        ok: true,
        diff: Some(diff.into()),
        error: None,
    };
    let findings = mgr.after(&outcome).await;
    assert!(findings.iter().any(|f| f.rule == "fatal_to_log"));
}

#[tokio::test]
async fn mcp_tool_flattens_content_blocks_to_text() {
    use app_lib::kernel_impl::mcp_tool::flatten_mcp_result;
    let v = json!({ "content": [
        {"type": "text", "text": "result part 1"},
        {"type": "text", "text": "result part 2"}
    ]});
    assert_eq!(flatten_mcp_result(&v), "result part 1\nresult part 2");
}

#[tokio::test]
async fn react_agent_streams_token_deltas_then_tool_calls_and_threads_ctx() {
    // F2.2 real streaming + F1.1 ctx threading. stream() yields multiple text
    // deltas then a terminal tool_calls Message per turn; the run loop must emit
    // a Token per delta (real streaming, not one merged message), enter the tool
    // loop on the terminal tool_calls, and pass the agent's ToolContext through
    // to the invoked tool.
    struct CtxProbe {
        seen: Arc<Mutex<Option<ToolContext>>>,
    }
    #[async_trait]
    impl Tool for CtxProbe {
        fn info(&self) -> ToolInfo {
            ToolInfo {
                name: "probe".into(),
                description: "records the tool context".into(),
                parameters_schema: json!({"type": "object"}),
            }
        }
        async fn invoke(&self, _args: &str, ctx: &ToolContext) -> Result<String, Error> {
            *self.seen.lock().unwrap() = Some(ctx.clone());
            Ok("ok".into())
        }
    }

    let seen = Arc::new(Mutex::new(None));
    let registry = ToolRegistry::new().with(CtxProbe { seen: Arc::clone(&seen) });

    let turns = vec![
        // Turn 1: text deltas + terminal tool_calls (the F2.1 stream shape).
        vec![
            Message::assistant("Hel"),
            Message::assistant("lo"),
            Message {
                role: Role::Assistant,
                content: String::new(),
                tool_calls: vec![tool_call("c1", "probe", "{}")],
                tool_call_id: None,
                reasoning: None,
            },
        ],
        // Turn 2: text only → turn boundary.
        vec![Message::assistant("All done.")],
    ];
    let agent = ReactAgent::new(StreamingChatModel::new(turns), registry, "sys").with_context(ToolContext {
        working_dir: Some("/proj".into()),
        conversation_id: Some("conv-9".into()),
    });

    let mut events = vec![];
    let mut stream = agent
        .run(AgentInput {
            prompt: "p".into(),
            working_dir: None,
            model: None,
            resume_from: None,
        })
        .unwrap();
    use futures::StreamExt;
    while let Some(ev) = stream.next().await {
        events.push(ev.unwrap());
    }

    // Real streaming: separate Token events for each delta (not one merged blob).
    let tokens: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Token(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert!(tokens.iter().any(|t| t == "Hel"), "first delta missing: {tokens:?}");
    assert!(tokens.iter().any(|t| t == "lo"), "second delta missing: {tokens:?}");
    assert!(tokens.iter().any(|t| t == "All done."), "final turn missing: {tokens:?}");

    // Tool loop entered on the terminal tool_calls: probe Started then Succeeded.
    let probe_statuses: Vec<ToolCallStatus> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCall(t) if t.tool == "probe" => Some(t.status.clone()),
            _ => None,
        })
        .collect();
    assert!(probe_statuses.iter().any(|s| matches!(s, ToolCallStatus::Started)), "no Started: {probe_statuses:?}");
    assert!(probe_statuses.iter().any(|s| matches!(s, ToolCallStatus::Succeeded)), "no Succeeded: {probe_statuses:?}");

    // F1.1 ctx threading: probe received the agent's working_dir + conversation_id.
    let ctx = seen.lock().unwrap().clone();
    assert_eq!(ctx.as_ref().and_then(|c| c.working_dir.as_deref()), Some("/proj"), "working_dir not threaded: {ctx:?}");
    assert_eq!(ctx.as_ref().and_then(|c| c.conversation_id.as_deref()), Some("conv-9"), "conversation_id not threaded: {ctx:?}");

    // Turn boundary after the final text-only turn, then Done.
    assert!(events.iter().any(|e| matches!(e, AgentEvent::TurnBoundary)), "no TurnBoundary");
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Done(_))), "no Done");
}

#[tokio::test]
async fn react_agent_injects_prior_history_between_system_and_current_task() {
    // G3a multi-turn resume: with_history splices prior-turn Messages between
    // the system prompt and the current task, in order, so the model sees real
    // conversation context — not just the latest prompt in isolation.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let model = CapturingChatModel { seen: Arc::clone(&seen) };
    let registry = ToolRegistry::new();

    // Prior turn 1: a user question + the assistant's reply (with a tool call)
    // + the tool result — the exact shape turns_to_history produces.
    let prior = vec![
        Message::user("turn1 prompt"),
        Message {
            role: Role::Assistant,
            content: "turn1 reply".into(),
            tool_calls: vec![tool_call("turn0_call0", "probe", "{}")],
            tool_call_id: None,
            reasoning: None,
        },
        Message {
            role: Role::Tool,
            content: "turn1 tool result".into(),
            tool_calls: Vec::new(),
            tool_call_id: Some("turn0_call0".into()),
            reasoning: None,
        },
    ];
    let agent = ReactAgent::new(model, registry, "SYS").with_history(prior);

    let mut stream = agent
        .run(AgentInput {
            prompt: "turn2 prompt".into(),
            working_dir: None,
            model: None,
            resume_from: None,
        })
        .unwrap();
    use futures::StreamExt;
    while stream.next().await.is_some() {}

    let captured = seen.lock().unwrap().clone();
    // Expected order: system, prior[0..3], current task. No drops, no reordering.
    assert_eq!(captured.len(), 5, "captured: {:?}", captured.iter().map(|m| (m.role, m.content.as_str())).collect::<Vec<_>>());
    assert_eq!(captured[0].role, Role::System);
    assert_eq!(captured[0].content, "SYS");
    assert_eq!(captured[1].role, Role::User);
    assert_eq!(captured[1].content, "turn1 prompt");
    assert_eq!(captured[2].role, Role::Assistant);
    assert_eq!(captured[2].content, "turn1 reply");
    assert_eq!(captured[3].role, Role::Tool);
    // Current task is LAST — after all prior history.
    assert_eq!(captured[4].role, Role::User);
    assert_eq!(captured[4].content, "turn2 prompt");
}

#[tokio::test]
async fn react_agent_without_history_keeps_system_then_task_only() {
    // Regression guard: empty history must reproduce the original single-turn
    // shape — system, then the task, nothing spliced between.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let model = CapturingChatModel { seen: Arc::clone(&seen) };
    let registry = ToolRegistry::new();
    let agent = ReactAgent::new(model, registry, "SYS").with_history(Vec::new());

    let mut stream = agent
        .run(AgentInput {
            prompt: "only prompt".into(),
            working_dir: None,
            model: None,
            resume_from: None,
        })
        .unwrap();
    use futures::StreamExt;
    while stream.next().await.is_some() {}

    let captured = seen.lock().unwrap().clone();
    assert_eq!(captured.len(), 2, "captured: {:?}", captured);
    assert_eq!(captured[0].role, Role::System);
    assert_eq!(captured[1].role, Role::User);
    assert_eq!(captured[1].content, "only prompt");
}
