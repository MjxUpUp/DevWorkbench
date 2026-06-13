//! End-to-end test: self-built ReactAgent with Skills + builtin tools + Hooks.
//!
//! No network: a scripted MockChatModel drives the reason->act->observe loop.
//! Verifies the full self-agent stack: ToolRegistry, SkillTool loading from
//! disk, hook interception, and the loop producing tool-call events.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use kernel_core::{
    Agent, AgentEvent, AgentInput, ChatModel, Error, FunctionCall, Message, MessageStream,
    ModelOptions, Role, Tool, ToolCall, ToolContext, ToolInfo,
};
use serde_json::json;

use app_lib::kernel_impl::react_agent::{ReactAgent, ToolRegistry};
use app_lib::kernel_impl::skill_tool::SkillTool;

struct MockChatModel {
    script: Mutex<Vec<Message>>,
}

impl MockChatModel {
    fn new(script: Vec<Message>) -> Self {
        Self { script: Mutex::new(script) }
    }
}

#[async_trait]
impl ChatModel for MockChatModel {
    async fn generate(&self, _messages: &[Message], _opts: &ModelOptions) -> Result<Message, Error> {
        let mut script = self.script.lock().unwrap();
        if script.is_empty() {
            return Ok(Message::assistant("[script exhausted]"));
        }
        Ok(script.remove(0))
    }
    fn stream(&self, _m: &[Message], _o: &ModelOptions) -> Result<MessageStream, Error> {
        Err(Error::Unsupported("mock cannot stream".into()))
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
