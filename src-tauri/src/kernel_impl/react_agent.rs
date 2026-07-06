//! Transparent ReactAgent + ToolRegistry.
//!
//! The "transparent" agent: the kernel controls the LLM call AND the tool loop
//! directly (eino `adk/react.go` Rust port). Used for kernel-internal tasks and
//! as a self-built agent that can call MCP tools and Skills.
//!
//! Two pieces live here:
//! - [`ToolRegistry`]: a cloneable collection of `dyn Tool` (MCP + Skill + builtin).
//! - [`ReactAgent`]: reason->act->observe loop, bounded by max_steps, implements
//!   `kernel_core::Agent`. Binds tools to the model, dispatches hooks around
//!   tool calls, and streams AgentEvents.
//!
//! The `ChatModel` implementations live in sibling modules: the Anthropic
//! Messages API in [`crate::kernel_impl::anthropic_chat_model`] and the OpenAI
//! Chat Completions API in [`crate::kernel_impl::openai_chat_model`], both
//! sharing cross-cutting state via [`crate::kernel_impl::chat_model_shared`].

use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;
use tauri::Emitter;

use async_trait::async_trait;
use futures::stream::BoxStream;
use kernel_core::{
    AgentCaps, AgentEvent, AgentInput, AgentKind, AgentOutcome, AgentRunStatus, ChatModel,
    CostTally, Error, Message, ModelOptions, Role, Tool, ToolContext, ToolInfo,
};
use serde_json::Value;

use crate::kernel_impl::hooks::HookManager;
use crate::kernel_impl::llm_recovery::{
    FatalReason, LlmErrorKind, MAX_ATTEMPTS, classify_llm_error, fatal_user_message,
    is_stream_interrupt, retry_delay, should_retry,
};
use crate::kernel_impl::context_compact::{self, ArchivedChunk};
use crate::kernel_impl::human_gate::{HumanGateCtx, HumanGateOutcome};
use crate::kernel_impl::model_router::TierCtx;

/// Injectable audit callback signature (project audit: cargo check + assertion
/// weakening scan). Shared by the config field, the builder, and test stubs.
type AuditFn = Arc<dyn Fn(&std::path::Path, &str) -> Value + Send + Sync>;
/// Per-step model router callback: (history, base_model) -> chosen model_id.
type ModelRouterFn = Arc<dyn Fn(&[Message], &str) -> String + Send + Sync>;

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
    /// The provider's tier pair inherited from the parent ReactAgent, so a
    /// dispatched child can itself run the per-step router + dispatch tiered
    /// grandchild agents. None = single-model provider (no routing).
    tier_ctx: Option<TierCtx>,
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
            None,
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
        tier_ctx: Option<TierCtx>,
    ) -> Self {
        Self {
            model,
            read_only_tools,
            max_steps,
            named,
            concurrency,
            tier_ctx,
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
        // model supports it (production AnthropicChatModel), so this child's LLM cost
        // is tallied into an accumulator we read after the run and append to the
        // tool result — the per-dispatch cost visibility the multi-agent board
        // surfaces. Test/ad-hoc models return None and run cost-blind (unchanged).
        let (child_model, accumulator) = match self.model.fork_with_counting_cost() {
            Some((m, acc)) => (m, Some(acc)),
            None => (Arc::clone(&self.model), None),
        };
        let child = ReactAgent::new_shared(child_model, child_tools, worker_prompt.as_str())
            .with_context(ctx.clone())
            .with_max_steps(self.max_steps);
        // "Model half" of sub-agent dispatch — 机器科层制: 贵模型只在裁决节点,
        // 不干杂活。fork_with_counting_cost 只换成本计数器、不改 model id,所以
        // 此前子 agent 每一轮都克隆父的 strong 模型。这里按任务类型提前分类,给子
        // agent 挂对应逐轮 router,让劳动轮跑 cheap model:
        //   CheapOnly(明确杂活:搜索/读取/抽取) → 全程 cheap
        //   Routed(含推理关键词或歧义)         → 挂 route_step(首轮 strong + 回声轮 cheap)
        // 子 agent 的 model 不是 provider 的 strong model(或 provider 无 tier pair) →
        // 不挂 router,子 agent 用自身模型均匀跑,与 executor.rs wire-time 的 tierable
        // 守门及 route_step 自身 base guard 对称,规避把本 provider 的 id 灌进异端点 → 400。
        // 裁决仍在 main:降档错了产出弱结论会被 main 抓回重派。
        let child = if let Some(tier) = self.tier_ctx.clone() {
            // Inherit the pair so a dispatched child can itself dispatch tiered
            // grandchild agents using the same provider pair.
            let child = child.with_tier_ctx(tier.strong.clone(), tier.cheap.clone());
            let dt = crate::kernel_impl::model_router::dispatch_tier_for(
                self.model.model_id(),
                &task,
                &tier,
            );
            match dt {
                Some(crate::kernel_impl::model_router::DispatchTier::CheapOnly) => child
                    .with_model_router(Arc::new(move |h, b| {
                        crate::kernel_impl::model_router::force_cheap_router(h, b, &tier)
                    })),
                Some(crate::kernel_impl::model_router::DispatchTier::Routed) => child
                    .with_model_router(Arc::new(move |h, b| {
                        crate::kernel_impl::model_router::route_step(h, b, &tier)
                    })),
                None => child,
            }
        } else {
            child
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
                    accumulator
                        .as_deref()
                        .map(kernel_core::CostAccumulator::tally),
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
    human_gate: Option<&HumanGateCtx>,
) -> CallOutcome {
    let mut events: Vec<kernel_core::ToolCallEvent> = Vec::new();
    // Classify once: the before-hook uses it for the veto, and — on a
    // successful write — we re-match it below to emit a per-write FileChanged.
    let action =
        crate::kernel_impl::hooks::classify_action(&call.function.name, &call.function.arguments);
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
    // Human Gate (Clutch #3): the hooks vetoed nothing, so before the tool
    // lands we let an interactive approval gate SUSPEND a destructive action.
    // Approve → fall through to the normal invoke; Reject/Retry → synthesize a
    // tool result (the agent adapts / gets feedback) WITHOUT invoking. Skipped
    // entirely when no gate is wired (the default), so non-gate callers pay zero
    // cost. Resolve relative write paths against the project working dir.
    let gate_override = if blocked.is_none() {
        if let Some(gate) = human_gate {
            let workdir = ctx
                .working_dir
                .as_deref()
                .map(std::path::Path::new)
                .unwrap_or_else(|| std::path::Path::new("."));
            match gate
                .check(&action, &call.function.name, &call.function.arguments, workdir)
                .await
            {
                HumanGateOutcome::Allow => None,
                HumanGateOutcome::Reject => Some("[blocked: 用户拒绝该破坏性操作]".to_string()),
                HumanGateOutcome::Retry(feedback) => {
                    Some(format!("[retry: {feedback}]"))
                }
            }
        } else {
            None
        }
    } else {
        None
    };
    let (result, file_changed) = match blocked {
        Some(b) => (b, None),
        None => match gate_override {
            // Human Gate rejected / asked retry → synthesize the result without
            // invoking the tool. Recorded as Failed so the run UI flags it.
            Some(g) => {
                events.push(kernel_core::ToolCallEvent {
                    tool: call.function.name.clone(),
                    arguments: call.function.arguments.clone(),
                    status: kernel_core::ToolCallStatus::Failed,
                    result: Some(g.clone()),
                });
                (g, None)
            }
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
    human_gate: Option<&HumanGateCtx>,
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
            let o = execute_one_call(tools, c, ctx, hooks, human_gate).await;
            (*i, o)
        });
        for (i, o) in futures::future::join_all(futs).await {
            outcomes[i] = Some(o);
        }
        // Run the remaining (non-dispatch) calls serially — writes included, so
        // AssertionGuard sees a clean pre/post git-diff window per write.
        for (i, call) in calls.iter().enumerate() {
            if outcomes[i].is_none() {
                outcomes[i] = Some(execute_one_call(tools, call, ctx, hooks, human_gate).await);
            }
        }
    } else {
        for (i, call) in calls.iter().enumerate() {
            outcomes[i] = Some(execute_one_call(tools, call, ctx, hooks, human_gate).await);
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
    /// The provider's strong + cheap model ids (multi-protocol refactor):
    /// drives sub-agent dispatch tiering (`dispatch_tier_for`) and the child
    /// router closures, so a dispatched sub-agent's labor turns run on the cheap
    /// model for ANY provider that declares both tiers — not just Z.AI. None =
    /// single-model provider (no per-step routing, no dispatch tiering).
    tier_ctx: Option<TierCtx>,
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
    /// G3 step-repetition breaker threshold. When the agent issues the same tool
    /// call (same name + same arguments) this many times in a row, the loop
    /// halts with a clear failure instead of burning the whole step budget — the
    /// MAST top failure mode is a weak model re-issuing an identical call hoping
    /// for a different result. Defaults to 5 (a normal explore-then-act sequence
    /// rarely repeats an identical call 5×). 1 disables (a single call trips).
    step_repetition_threshold: usize,
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
    /// Session id this agent run belongs to (chat path only). When set, the
    /// compaction sink can archive the dropped原文 + emit a Compact meta-event
    /// scoped to this session. Workflow/ACP agents leave it None (no archive,
    /// no UI event — pure compaction). v1.3 C2.
    session_id: Option<String>,
    /// Tauri AppHandle for emitting the Compact meta-event on `agent:event`.
    /// Held directly (NOT via ToolContext) — same pattern as `WorkflowTool`.
    /// None for tests / workflow agents / ACP → sink becomes a no-op. The
    /// Option is shared with session_id above; both must be Some to archive.
    app: Option<tauri::AppHandle>,
    /// Shared buffer the compaction sink appends Compact events into, so the
    /// driver loop can persist them into `session.blocks` (Compact bypasses the
    /// AgentEvent stream, so it can't be collected there). Held via Arc<Mutex>
    /// so the FnMut sink (which the try_stream owns) and the driver (which owns
    /// the agent) both see the same Vec. None = sink doesn't persist into this
    /// buffer (workflow/test path).
    compaction_blocks: Option<Arc<Mutex<Vec<crate::agents::pty::ChatStreamEvent>>>>,
    /// Approval registry for the Human Gate (Clutch #3). When set, every
    /// destructive tool call in this run suspends for interactive approval before
    /// landing; the user resolves it via `resolve_human_gate_cmd`. None = Human
    /// Gate off (default; workflows/tests/ACP leave it unset). Shares the same
    /// `ApprovalMap` instance as `commands::agents::AgentApprovalState` so the
    /// resolve command delivers to this run's suspended call. Reuses `app` /
    /// `session_id` set via [`with_compaction_archive`]; if either is None the
    /// gate stays a no-op (fail-open Allow).
    approval: Option<crate::kernel_impl::human_gate::ApprovalMap>,
}

/// G3 step-repetition breaker — detects when the agent loops on the same tool
/// call (identical name + arguments) and trips so `run_loop` can halt with a
/// clear failure instead of burning the whole step budget. This is the MAST
/// top failure mode: a weak model re-issues an identical call hoping for a
/// different result. Exact-arg matching is deliberate — differing args (even
/// trivially) is exploration, not a loop, and fuzzy matching would risk false
/// positives that abort legitimate retry. Pure + deterministic; no IO.
struct StepRepetitionBreaker {
    threshold: usize,
    last_sig: Option<String>,
    consecutive: usize,
}

impl StepRepetitionBreaker {
    fn new(threshold: usize) -> Self {
        Self {
            // threshold of 0/1 makes the very first repeat trip; guard so a
            // misconfigured 0 still means "trip on the 2nd identical call".
            threshold: threshold.max(2),
            last_sig: None,
            consecutive: 0,
        }
    }

    /// Observe one tool call's signature. Returns `Some(reason)` when the breaker
    /// trips on this observation (i.e. `consecutive` reaches `threshold`). A
    /// different signature resets the streak to 1 (this call).
    fn observe(&mut self, name: &str, arguments: &str) -> Option<String> {
        // NUL can't appear in either field, so it's a safe separator.
        let sig = format!("{name}\u{0}{arguments}");
        if self.last_sig.as_deref() == Some(sig.as_str()) {
            self.consecutive += 1;
        } else {
            self.consecutive = 1;
            self.last_sig = Some(sig);
        }
        if self.consecutive >= self.threshold {
            Some(format!(
                "step 重复熔断：连续 {n} 次相同「{name}」调用（参数未变）——疑似循环，停止以免空耗步数",
                n = self.consecutive
            ))
        } else {
            None
        }
    }
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
            tier_ctx: None,
            budget_check: None,
            max_context_tokens: None,
            compact_keep_recent: 6,
            step_repetition_threshold: 5,
            system_prompt: system_prompt.into(),
            ctx: ToolContext::default(),
            history: Vec::new(),
            thinking: None,
            session_id: None,
            app: None,
            compaction_blocks: None,
            approval: None,
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

    /// Set the step-repetition breaker threshold (G3). The run halts once the
    /// same tool call (name + arguments) repeats this many times consecutively.
    /// Default 5; lower for cheaper loop detection in tests.
    pub fn with_step_repetition_threshold(mut self, n: usize) -> Self {
        self.step_repetition_threshold = n;
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

    /// Set the provider's strong + cheap model pair, enabling data-driven
    /// per-step routing and sub-agent dispatch tiering for this provider
    /// (multi-protocol refactor: replaces the old hardcoded GLM constants). Both
    /// ids must belong to the SAME provider (same endpoint/key) — routing swaps
    /// the model id only, never the endpoint.
    pub fn with_tier_ctx(mut self, strong: impl Into<String>, cheap: impl Into<String>) -> Self {
        self.tier_ctx = Some(TierCtx {
            strong: strong.into(),
            cheap: cheap.into(),
        });
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

    /// Wire the chat-path session id + AppHandle + shared Compact-event buffer
    /// so the compaction sink can (a) archive the dropped原文 to
    /// `agents/compact/{sid}.jsonl`, (b) emit a Compact meta-event on
    /// `agent:event`, and (c) append the event into the driver's final_blocks
    /// for persistence. All three are gated together — workflow/test/ACP agents
    /// leave them None and the sink stays a no-op (compaction runs but stays
    /// silent, the original kernel behavior). v1.3 C2.
    pub fn with_compaction_archive(
        mut self,
        session_id: String,
        app: tauri::AppHandle,
        compaction_blocks: Arc<Mutex<Vec<crate::agents::pty::ChatStreamEvent>>>,
    ) -> Self {
        self.session_id = Some(session_id);
        self.app = Some(app);
        self.compaction_blocks = Some(compaction_blocks);
        self
    }

    /// Wire the Human Gate (Clutch #3). When set, destructive tool calls in
    /// this run suspend for interactive approval before landing. `approvals` is
    /// the same shared map held by `commands::agents::AgentApprovalState`, so
    /// `resolve_human_gate_cmd` delivers the user's decision to the suspended
    /// call. Reuses `app` + `session_id` set via [`with_compaction_archive`]; if
    /// those are unset the gate is a silent no-op (fail-open Allow), so a
    /// workflow/test agent that never sets them stays ungated. v2 Human Gate.
    pub fn with_human_gate(
        mut self,
        approvals: crate::kernel_impl::human_gate::ApprovalMap,
    ) -> Self {
        self.approval = Some(approvals);
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
            // D2 lifecycle: SessionStart fires once at run entry — stdout
            // (exit 0) is injected as session-level context into the first
            // turn's prompt. exit 2 is logged but cannot refuse a session.
            match hooks
                .dispatch_event(&crate::kernel_impl::hooks::HookEvent::SessionStart {
                    task: task.to_string(),
                })
                .await
            {
                Ok(ctxs) if !ctxs.is_empty() => {
                    full_task.push_str("\n\n[session-start context]\n");
                    full_task.push_str(&ctxs.join("\n---\n"));
                }
                Ok(_) => {}
                Err(reason) => {
                    log::warn!(
                        "[user-hook] SessionStart exit-2 ignored (session not refuseable): {}",
                        reason.message
                    );
                }
            }
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
        // Human Gate context (Clutch #3): built once per run_loop so the seq
        // counter yields distinct resume tokens across turns. No-op when app /
        // session_id / approval aren't all set (sub-agent dispatch from a
        // workflow, or tests) — fail-open Allow, matching the run() path.
        let hg = match (&self.app, &self.session_id, &self.approval) {
            (Some(app), Some(sid), Some(ap)) => {
                Some(HumanGateCtx::new(app.clone(), sid.clone(), ap.clone()))
            }
            _ => None,
        };
        let result: Result<String, Error> = async {
            // G3 step-repetition breaker: a weak model re-issuing the same tool
            // call (same name + args) consecutively is the MAST top failure mode.
            // Trips at step_repetition_threshold identical calls → halt with a
            // clear reason instead of burning the whole step budget on a loop.
            let mut rep_breaker = StepRepetitionBreaker::new(self.step_repetition_threshold);
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
                // G3: observe each call's signature; if the same tool+args
                // repeats past the threshold, halt (loop detected) — before
                // spending another tool execution on an identical call.
                for call in &resp.tool_calls {
                    if let Some(reason) = rep_breaker
                        .observe(&call.function.name, &call.function.arguments)
                    {
                        return Err(Error::Agent(reason));
                    }
                }
                for call in &resp.tool_calls {
                    let result = self
                        .execute_tool_call(call, &self.ctx, hg.as_ref())
                        .await;
                    history.push(Message {
                        role: Role::Tool,
                        content: context_compact::cap_tool_result(
                            &result,
                            context_compact::MAX_TOOL_RESULT_TOKENS,
                        ),
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

    async fn execute_tool_call(
        &self,
        call: &kernel_core::ToolCall,
        ctx: &ToolContext,
        human_gate: Option<&HumanGateCtx>,
    ) -> String {
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
        // Human Gate (Clutch #3): hooks + dry-run both let it through, so before
        // the side effect lands, suspend a destructive action for interactive
        // approval. Approve → fall through to the real invoke; Reject/Retry →
        // synthesize the tool result without invoking (mirrors execute_one_call).
        if let Some(gate) = human_gate {
            let workdir = ctx
                .working_dir
                .as_deref()
                .map(std::path::Path::new)
                .unwrap_or_else(|| std::path::Path::new("."));
            match gate
                .check(&action, &call.function.name, &call.function.arguments, workdir)
                .await
            {
                HumanGateOutcome::Allow => {}
                HumanGateOutcome::Reject => return "[blocked: 用户拒绝该破坏性操作]".to_string(),
                HumanGateOutcome::Retry(feedback) => return format!("[retry: {feedback}]"),
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
                    .map(|f| {
                        format!(
                            "  - [{}] {} (evidence: {})",
                            f.rule, f.explanation, f.evidence
                        )
                    })
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
        let app_opt = self.app.clone();
        let sid_opt = self.session_id.clone();
        let compaction_buf_opt = self.compaction_blocks.clone();
        let approval_opt = self.approval.clone();
        // Clone the threshold by value (usize) so the async block below doesn't
        // borrow `&self` — the stream must own its data to satisfy 'static.
        let step_repetition_threshold = self.step_repetition_threshold;

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
            // Human Gate context (Clutch #3): built once so the seq counter
            // yields distinct resume tokens across turns. No-op when app /
            // session_id / approval aren't all set (workflow/test/ACP agents) —
            // fail-open Allow, matching run_loop. Destructive actions suspend
            // here for interactive approval before the tool lands.
            let hg = match (&app_opt, &sid_opt, &approval_opt) {
                (Some(app), Some(sid), Some(ap)) => {
                    Some(HumanGateCtx::new(app.clone(), sid.clone(), ap.clone()))
                }
                _ => None,
            };
            // D2 lifecycle: dispatch UserPromptSubmit BEFORE the user message
            // enters history; user-hook stdout (exit 0) is appended to the prompt
            // as additional context (claude-code additionalContext injection).
            let mut full_task = task.clone();
            if let Some(h) = hooks.as_ref() {
                // SessionStart: inject session-level context into the first
                // turn. exit 2 logged but cannot refuse a session.
                match h
                    .dispatch_event(&crate::kernel_impl::hooks::HookEvent::SessionStart {
                        task: task.clone(),
                    })
                    .await
                {
                    Ok(ctxs) if !ctxs.is_empty() => {
                        full_task.push_str("\n\n[session-start context]\n");
                        full_task.push_str(&ctxs.join("\n---\n"));
                    }
                    Ok(_) => {}
                    Err(reason) => {
                        log::warn!(
                            "[user-hook] SessionStart exit-2 ignored (session not refuseable): {}",
                            reason.message
                        );
                    }
                }
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
            // inside the ChatModel), so a blanket flagship fallback would route
            // every turn against one fixed id — picking a different model then sent
            // the wrong id (session 7f51a5d2, 2026-06-21: 401, the user's key had
            // no access to the hardcoded flagship). A ChatModel that doesn't expose
            // an id (test stubs) returns "" → keep a concrete fallback string
            // there so construction never panics.
            let base_model = model_opt.clone().unwrap_or_else(|| {
                let mid = model.model_id();
                if mid.is_empty() {
                    "glm-4.6".to_string()
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
            // G3 step-repetition breaker: trips when the same tool call (name +
            // args) repeats past step_repetition_threshold → halt with a clear
            // failure instead of looping. Same detector run_loop uses; this is
            // the streaming chat path (the MAST top failure mode happens here).
            let mut rep_breaker = StepRepetitionBreaker::new(step_repetition_threshold);
            let mut step_repetition_trip: Option<String> = None;
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
                    // Compaction is a meta-event: it never enters the AgentEvent
                    // stream. maybe_compact pushes each dropped原文 chunk into a
                    // shared Arc<Mutex> buffer (an owned Arc — no &mut borrow of a
                    // local, so the future stays Send across the summarizer
                    // .await without tripping the borrow checker). AFTER the call
                    // returns we drain the buffer and emit one Compact agent:event
                    // per chunk + append into the driver's final_blocks — this
                    // emit/drain runs in sync code, free of any await-lifetime
                    // entanglement. Only the chat path has all three of
                    // session_id/app/driver_buf; workflow/ACP/test agents leave
                    // them None → no buffer passed, no emit (pure compaction,
                    // the original behavior).
                    let archive_buf: Arc<Mutex<Vec<ArchivedChunk>>> =
                        Arc::new(Mutex::new(Vec::new()));
                    let archive_buf_for_call = match (&sid_opt, &app_opt) {
                        (Some(_), Some(_)) => Some(Arc::clone(&archive_buf)),
                        _ => None,
                    };
                    // D2 lifecycle: PreCompact fires before compaction runs.
                    // exit 2 blocks this round — history is left untouched and
                    // the next turn retries. exit 0 stdout is logged (v1; v2 may
                    // pass it as a keep-hint to the summarizer).
                    let mut skip_compact = false;
                    if let Some(h) = hooks.as_ref() {
                        match h
                            .dispatch_event(&crate::kernel_impl::hooks::HookEvent::PreCompact {
                                current_tokens: context_compact::estimate_tokens(&history),
                                max_tokens: max_tok,
                            })
                            .await
                        {
                            Ok(_) => {}
                            Err(reason) => {
                                log::info!(
                                    "[user-hook] PreCompact blocked compaction this round: {}",
                                    reason.message
                                );
                                skip_compact = true;
                            }
                        }
                    }
                    if !skip_compact {
                        let _ = context_compact::maybe_compact(
                            &mut history,
                            model.as_ref(),
                            &opts,
                            max_tok,
                            compact_keep_recent,
                            &mut compact_consecutive_failures,
                            archive_buf_for_call,
                        )
                        .await;
                    }
                    // Drain + emit/persist OUTSIDE the maybe_compact borrow.
                    if let (Some(sid), Some(app), Some(driver_buf)) =
                        (sid_opt.as_ref(), app_opt.as_ref(), compaction_buf_opt.as_ref())
                    {
                        let chunks: Vec<ArchivedChunk> = archive_buf
                            .lock()
                            .map(|mut g| g.drain(..).collect())
                            .unwrap_or_default();
                        for chunk in chunks {
                            let dropped_count = chunk.dropped_messages.len();
                            // Resolve (summary, is_error) per kind. BreakerTripped
                            // is the failure case — surfaces the silent-suspension
                            // mode as a danger card; the other two are normal
                            // compactions (info cards).
                            let (summary_text, is_error) = match chunk.kind {
                                context_compact::ArchivedKind::BreakerTripped => (
                                    chunk.summary.clone().unwrap_or_else(|| {
                                        "上下文压缩已暂停".to_string()
                                    }),
                                    true,
                                ),
                                context_compact::ArchivedKind::HardTruncate => (
                                    chunk.summary.clone().unwrap_or_else(|| {
                                        "压缩已暂停且仍超上限，紧急丢弃最早历史（数据有损）".to_string()
                                    }),
                                    true,
                                ),
                                context_compact::ArchivedKind::MicroClear => (
                                    chunk.summary.clone().unwrap_or_else(|| {
                                        format!("已压缩 {} 条陈旧工具输出", dropped_count)
                                    }),
                                    false,
                                ),
                                context_compact::ArchivedKind::Summarize => (
                                    chunk.summary
                                        .clone()
                                        .unwrap_or_else(|| "已压缩历史".to_string()),
                                    false,
                                ),
                            };
                            // Archive dropped原文 to disk (best-effort; the
                            // returned path becomes the card's archived_at —
                            // the UI's expand view keys off its presence). The
                            // gate is "was anything dropped", NOT is_error:
                            // BreakerTripped carries no dropped content (skip —
                            // an empty JSONL line only pollutes the audit log),
                            // but HardTruncate is an error card that DID drop
                            // real turns — those must be recoverable from disk.
                            let archived_at = if chunk.dropped_messages.is_empty() {
                                None
                            } else {
                                crate::agents::pty::append_compact_archive(sid, &chunk)
                            };
                            let wire = crate::agents::pty::ChatStreamEvent::Compact {
                                summary: summary_text,
                                archived_at,
                                dropped_count,
                                is_error,
                            };
                            let _ = app.emit(
                                "agent:event",
                                serde_json::json!({ "sessionId": sid, "event": &wire }),
                            );
                            // Persist into the driver's final_blocks so the
                            // compact card survives the live→persisted handoff
                            // (Compact bypasses the AgentEvent stream, so the
                            // driver loop can't collect it itself).
                            if let Ok(mut g) = driver_buf.lock() {
                                g.push(wire);
                            }
                        }
                    }
                }
                // Real streaming: consume the model's SSE stream, yielding each
                // text delta as a Token (chat renders token-by-token) while the
                // stream() helper accumulates tool_calls from content_block_start
                // + input_json_delta events. Text + tool_calls are reassembled
                // into one assistant Message for coherent next-turn history.
                use futures::StreamExt;
                // C7 tool-call recovery + L1/L3 stream reliability: retry
                // transient LLM failures — send errors (network/5xx/429) AND
                // mid-stream truncation/idle-stall that arrive BEFORE any output
                // — with exponential backoff; fatal errors (circuit open / quota
                // / auth / 4xx) and stream truncation AFTER partial output was
                // already shown degrade at once. The breaker inside
                // AnthropicChatModel records each attempt, so a run of retries
                // naturally trips the circuit. NOTE: both retry paths (stream
                // establishment + mid-stream) share `attempt` — the total budget
                // across them is MAX_ATTEMPTS, not MAX_ATTEMPTS each.
                let mut attempt = 1u32;
                let mut turn_text = String::new();
                let mut turn_reasoning = String::new();
                let mut turn_tool_calls: Vec<kernel_core::ToolCall> = Vec::new();
                let mut turn_sig: Option<String> = None;
                'retry: loop {
                    // Establish the stream — retry transient send failures.
                    let mut turn_stream = match bound.stream(&history, &opts) {
                        Ok(s) => s,
                        Err(e) => {
                            if should_retry(&e, attempt) {
                                log::warn!(
                                    "[ReactAgent] transient LLM error, retry {}/{}: {}",
                                    attempt,
                                    MAX_ATTEMPTS,
                                    e
                                );
                                tokio::time::sleep(retry_delay(attempt)).await;
                                attempt += 1;
                                continue 'retry;
                            }
                            degraded = Some(match classify_llm_error(&e) {
                                LlmErrorKind::Fatal(r) => r,
                                LlmErrorKind::Retryable => FatalReason::Generic,
                            });
                            break 'retry;
                        }
                    };
                    // Reset this turn's accumulators on every (re)entry so a
                    // retried turn starts clean — partial output from a prior
                    // failed attempt was never emitted to history.
                    turn_text.clear();
                    turn_reasoning.clear();
                    turn_tool_calls.clear();
                    turn_sig = None;
                    while let Some(msg_res) = turn_stream.next().await {
                        let msg = match msg_res {
                            Ok(m) => m,
                            Err(e) => {
                                // L4: a stream interrupted (L1 truncation / L3
                                // idle stall) BEFORE any content was emitted is
                                // safe to retry — the UI has nothing to discard.
                                // After partial output, retrying would duplicate
                                // it, so degrade honestly as StreamTruncated.
                                let emitted =
                                    !turn_text.is_empty() || !turn_reasoning.is_empty();
                                // is_interrupt = the error is an upstream stream
                                // cut (L1 truncation / L3 idle stall / network
                                // mid-stream). Computed independently of `emitted`:
                                // a stream can be cut before ANY byte reaches the
                                // wire (90s idle with nothing sent, or connection
                                // reset on every attempt). Only when `is_interrupt
                                // && emitted` — partial output really was shown —
                                // do we use StreamTruncated's "interrupted after
                                // partial output" message; otherwise fall through
                                // to classify so a never-emitted turn doesn't
                                // falsely claim partial output was shown.
                                let is_interrupt = is_stream_interrupt(&e);
                                if !emitted && should_retry(&e, attempt) {
                                    // Any transient error (truncation/idle/network)
                                    // before output is safe to retry — the UI has
                                    // nothing to discard.
                                    log::warn!(
                                        "[ReactAgent] stream interrupted before output, retry {}/{}: {}",
                                        attempt,
                                        MAX_ATTEMPTS,
                                        e
                                    );
                                    tokio::time::sleep(retry_delay(attempt)).await;
                                    attempt += 1;
                                    continue 'retry;
                                }
                                degraded = Some(if is_interrupt && emitted {
                                    FatalReason::StreamTruncated
                                } else {
                                    match classify_llm_error(&e) {
                                        LlmErrorKind::Fatal(r) => r,
                                        LlmErrorKind::Retryable => FatalReason::Generic,
                                    }
                                });
                                break 'retry;
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
                    break 'retry; // turn consumed cleanly
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
                // G3: observe this turn's calls; if the same tool+args repeats
                // past the threshold, record the trip + halt (loop detected).
                if step_repetition_trip.is_none() {
                    for call in &turn_tool_calls {
                        if let Some(reason) = rep_breaker
                            .observe(&call.function.name, &call.function.arguments)
                        {
                            step_repetition_trip = Some(reason);
                            break;
                        }
                    }
                    if step_repetition_trip.is_some() {
                        break;
                    }
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
                    let outcomes =
                        execute_call_set(&tools, &turn_tool_calls, &ctx, &hooks, hg.as_ref()).await;
                    for o in &outcomes {
                        for ev in &o.events {
                            yield AgentEvent::ToolCall(ev.clone());
                        }
                        if let Some(p) = &o.file_changed {
                            yield AgentEvent::FileChanged(p.clone());
                        }
                        history.push(Message {
                            role: Role::Tool,
                            content: context_compact::cap_tool_result(
                                &o.result,
                                context_compact::MAX_TOOL_RESULT_TOKENS,
                            ),
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
                        let o =
                            execute_one_call(&tools, call, &ctx, &hooks, hg.as_ref()).await;
                        for ev in &o.events {
                            yield AgentEvent::ToolCall(ev.clone());
                        }
                        if let Some(p) = &o.file_changed {
                            yield AgentEvent::FileChanged(p.clone());
                        }
                        history.push(Message {
                            role: Role::Tool,
                            content: context_compact::cap_tool_result(
                                &o.result,
                                context_compact::MAX_TOOL_RESULT_TOKENS,
                            ),
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
                let summary = if let Some(t) = &step_repetition_trip {
                    t.clone()
                } else if let Some(reason) = &degraded {
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
            if let Some(reason) = step_repetition_trip {
                yield AgentEvent::Done(AgentOutcome {
                    status: AgentRunStatus::Failed,
                    files_changed: Vec::new(),
                    exit_code: Some(1),
                    output_summary: Some(reason),
                    honesty: None,
                });
            } else if let Some(reason) = degraded {
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
    use crate::cost::circuit_breaker::CircuitBreaker;
    use crate::cost::pricing;
    use crate::kernel_impl::anthropic_chat_model::{
        AnthropicChatModel, decode_anthropic_message, handle_sse_line, parse_usage,
        shared_anthropic_circuit, usage_from_response,
    };
    use kernel_core::MessageStream;
    use kernel_core::ToolInfo;
    use serde_json::json;
    use std::collections::HashMap;

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
        assert!(
            line.contains("1234→567 tok"),
            "token split rendered: {line}"
        );
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
            Err(Error::Unsupported(
                "ConcurrentModel is generate-only".into(),
            ))
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
            None,
        );
        let mut reg = ToolRegistry::new();
        reg.push(tool);
        let calls = vec![
            probe_call("dispatch_subagent", r#"{"task":"a"}"#),
            probe_call("dispatch_subagent", r#"{"task":"b"}"#),
            probe_call("dispatch_subagent", r#"{"task":"c"}"#),
        ];
        let outcomes =
            execute_call_set(&reg, &calls, &ToolContext::default(), &None, None).await;
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
            assert_eq!(
                parse_subagent_status(&o.result),
                Some(SubagentStatus::Completed)
            );
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
            None,
        );
        let mut reg = ToolRegistry::new();
        reg.push(tool);
        let calls = vec![
            probe_call("dispatch_subagent", r#"{"task":"a"}"#),
            probe_call("dispatch_subagent", r#"{"task":"b"}"#),
            probe_call("dispatch_subagent", r#"{"task":"c"}"#),
        ];
        let outcomes =
            execute_call_set(&reg, &calls, &ToolContext::default(), &None, None).await;
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
        let outcomes =
            execute_call_set(&reg, &calls, &ToolContext::default(), &None, None).await;
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].call_id, calls[0].id);
        assert_eq!(outcomes[1].call_id, calls[1].id);
        // Each succeeded with one Succeeded event carrying the echoed args.
        assert_eq!(outcomes[0].events.len(), 1);
        assert_eq!(
            outcomes[0].events[0].status,
            kernel_core::ToolCallStatus::Succeeded
        );
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
        let mut model = AnthropicChatModel::bigmodel("k", "glm-4.6");
        model.shared.bound_tools = vec![ToolInfo {
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
        let model = AnthropicChatModel::bigmodel("k", "glm-4.6");
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
        let model = AnthropicChatModel::bigmodel("k", "glm-4.6");
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
            Err(Error::Unsupported(
                "TwoPhaseModel: drive via generate".into(),
            ))
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
        assert!(
            out.is_ok(),
            "run_loop should converge after repair: {out:?}"
        );
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
            .execute_tool_call(&probe_call("write_file", r#"{"path":"a.rs"}"#), &ctx, None)
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
            .execute_tool_call(&probe_call("read_file", r#"{"path":"a.rs"}"#), &ctx, None)
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
            .execute_tool_call(&probe_call("nope", "{}"), &ctx, None)
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
            .execute_tool_call(&probe_call("write_file", r#"{"path":"a.rs"}"#), &ctx, None)
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
        // AnthropicChatModel), so the per-step router's base_model used to fall back
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
        let tier = crate::kernel_impl::model_router::TierCtx {
            strong: "glm-4.6".to_string(),
            cheap: "glm-4-flash".to_string(),
        };
        let agent = ReactAgent::new(model, ToolRegistry::new(), "sys").with_model_router(Arc::new(
            move |h, b| crate::kernel_impl::model_router::route_step(h, b, &tier),
        ));
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
        assert_eq!(
            seen.len(),
            1,
            "one stream call on a converging turn: {seen:?}"
        );
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
            .execute_tool_call(&probe_call("write_file", r#"{"path":"a.rs"}"#), &ctx, None)
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

    /// Mock that returns a stream emitting `emit` messages and then yielding
    /// `trunc` mid-stream — drives the L1/L3 mid-stream truncation path that
    /// a stream()-establishment mock (RetryingModel) can't reach. On the second
    /// call (retry) it succeeds with `recover` if set, else truncates again.
    #[derive(Clone)]
    struct MidStreamTruncationModel {
        emit: Vec<Message>,
        make_trunc: Arc<dyn Fn() -> Error + Send + Sync>,
        recover: Option<Message>,
        call: Arc<std::sync::atomic::AtomicUsize>,
    }
    #[async_trait]
    impl ChatModel for MidStreamTruncationModel {
        async fn generate(&self, _: &[Message], _: &ModelOptions) -> Result<Message, Error> {
            Err(Error::Unsupported(
                "MidStreamTruncationModel: drive via stream()".into(),
            ))
        }
        fn stream(&self, _: &[Message], _: &ModelOptions) -> Result<MessageStream, Error> {
            let idx = self.call.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if idx == 0 {
                // First attempt: emit partial output, then truncate mid-stream.
                let emit = self.emit.clone();
                let trunc = (self.make_trunc)();
                Ok(Box::pin(async_stream::stream! {
                    for m in emit {
                        yield Ok(m);
                    }
                    yield Err(trunc);
                }))
            } else if let Some(ok) = self.recover.clone() {
                Ok(Box::pin(futures::stream::once(async move { Ok(ok) })))
            } else {
                let trunc = (self.make_trunc)();
                Ok(Box::pin(futures::stream::once(async move { Err(trunc) })))
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

    /// Drain a run stream, accumulating every non-terminal AgentEvent (Token /
    /// Reasoning / ...) alongside the terminal Done outcome — so a test can
    /// assert what was actually streamed to the UI (e.g. partial output before
    /// a degrade), not just the terminal outcome. Done is extracted into the
    /// second slot and kept OUT of the events vec so callers don't have to
    /// filter it when asserting on streamed tokens.
    async fn collect_events<S>(
        mut s: S,
    ) -> (Vec<kernel_core::AgentEvent>, Option<kernel_core::AgentOutcome>)
    where
        S: futures::Stream<Item = Result<kernel_core::AgentEvent, Error>> + Unpin,
    {
        use futures::StreamExt;
        let mut events = Vec::new();
        let mut outcome = None;
        while let Some(ev) = s.next().await {
            match ev {
                Ok(kernel_core::AgentEvent::Done(o)) => outcome = Some(o),
                Ok(e) => events.push(e),
                Err(_) => {}
            }
        }
        (events, outcome)
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
            make: Arc::new(|| Error::Model("LLM call failed: 401 unauthorized".into())),
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
    async fn run_retries_mid_stream_truncation_before_output() {
        use kernel_core::Agent;
        // L4: stream established OK but truncated (StreamIncomplete) BEFORE any
        // content was emitted → safe to retry the whole turn. Second attempt
        // succeeds. Proves mid-stream truncation with no emitted output is
        // retried (not silently completed, not degraded) — the exact regression
        // behind the 8f41b658 "only thinking, no reply" session.
        let model = MidStreamTruncationModel {
            emit: vec![], // nothing emitted before truncation
            make_trunc: Arc::new(|| Error::StreamIncomplete { got_reason: false }),
            recover: Some(Message::assistant("recovered after truncation")),
            call: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let agent = ReactAgent::new(model, ToolRegistry::new(), "sys");
        let s = agent.run(go_input()).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Completed);
        assert_eq!(
            outcome.output_summary.as_deref(),
            Some("recovered after truncation")
        );
    }

    #[tokio::test]
    async fn run_degrades_when_truncated_after_partial_output() {
        use kernel_core::Agent;
        // L5: stream emitted partial output, THEN truncated. Retrying would
        // duplicate the partial output already shown, so the run degrades to
        // Failed with the StreamTruncated message instead of pretending it
        // completed. `recover` is configured to prove it is NOT used after emit.
        // We collect the full event stream (not just the outcome) to assert the
        // partial turn was actually streamed to the UI as a Token event BEFORE
        // the degrade — that is the L5 contract ("show partial, then degrade").
        let model = MidStreamTruncationModel {
            emit: vec![Message::assistant("partial response")],
            make_trunc: Arc::new(|| Error::StreamIncomplete { got_reason: false }),
            recover: Some(Message::assistant("would-duplicate")),
            call: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let agent = ReactAgent::new(model, ToolRegistry::new(), "sys");
        let s = agent.run(go_input()).unwrap();
        let (events, outcome) = collect_events(s).await;
        let outcome = outcome.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Failed);
        let summary = outcome.output_summary.expect("degraded summary");
        assert!(
            summary.contains("interrupted"),
            "StreamTruncated message: {summary}"
        );
        let streamed: Vec<String> = events
            .into_iter()
            .filter_map(|e| match e {
                kernel_core::AgentEvent::Token(t) => Some(t),
                _ => None,
            })
            .collect();
        assert!(
            streamed.iter().any(|t| t.contains("partial response")),
            "partial output must be streamed as a Token before degrade: {streamed:?}"
        );
    }

    #[tokio::test]
    async fn run_retries_mid_stream_idle_before_output() {
        use kernel_core::Agent;
        // L3 watchdog: stream established OK but went idle (StreamIdle) BEFORE
        // any content was emitted → same retry path as truncation. Second
        // attempt succeeds. Locks that StreamIdle shares the L4 mid-stream
        // retry-before-emit path (is_stream_interrupt membership), so an idle
        // stall never silently completes either.
        let model = MidStreamTruncationModel {
            emit: vec![],
            make_trunc: Arc::new(|| Error::StreamIdle { secs: 90 }),
            recover: Some(Message::assistant("recovered after idle")),
            call: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let agent = ReactAgent::new(model, ToolRegistry::new(), "sys");
        let s = agent.run(go_input()).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Completed);
        assert_eq!(
            outcome.output_summary.as_deref(),
            Some("recovered after idle")
        );
    }

    #[tokio::test]
    async fn run_degrades_after_attempt_exhaustion_with_no_output() {
        use kernel_core::Agent;
        // Locks the fix for the false "partial output already shown" message:
        // the stream truncates on EVERY attempt with no content emitted, so
        // retries exhaust (attempt 1→2→3) and the run degrades. Because nothing
        // was ever shown, the degrade must take the generic path — NOT
        // StreamTruncated's "after partial output" wording. Pre-fix this test
        // would fail (degrade wrongly picked StreamTruncated because
        // is_interrupt was true regardless of emitted).
        let model = MidStreamTruncationModel {
            emit: vec![],
            make_trunc: Arc::new(|| Error::StreamIncomplete { got_reason: false }),
            recover: None, // never recovers — every attempt truncates
            call: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let agent = ReactAgent::new(model, ToolRegistry::new(), "sys");
        let s = agent.run(go_input()).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Failed);
        let summary = outcome.output_summary.expect("degraded summary");
        assert!(
            !summary.contains("partial output"),
            "must NOT claim partial output was shown (none was): {summary}"
        );
        assert!(
            summary.contains("retries") || summary.contains("failed"),
            "generic-fatal wording after retry exhaustion: {summary}"
        );
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

    // --- G3: step-repetition breaker (same tool+args loop → halt) ---

    #[test]
    fn step_repetition_breaker_trips_at_threshold_and_resets_on_change() {
        // Pure logic: trips at threshold consecutive identical sigs; a different
        // sig (different args) resets the streak to 1.
        let mut b = StepRepetitionBreaker::new(3);
        assert_eq!(b.observe("grep", r#"{"q":"x"}"#), None); // streak 1
        assert_eq!(b.observe("grep", r#"{"q":"x"}"#), None); // streak 2
        // different args → reset to 1, no trip
        assert_eq!(b.observe("grep", r#"{"q":"y"}"#), None);
        // back to identical → 2, then 3 trips
        assert_eq!(b.observe("grep", r#"{"q":"y"}"#), None); // streak 2
        let trip = b.observe("grep", r#"{"q":"y"}"#).expect("trips at threshold 3");
        assert!(trip.contains("step 重复熔断"), "trip reason: {trip}");
        assert!(trip.contains("grep"), "names the looping tool: {trip}");
        // threshold 0/1 clamps to 2 (misconfig guard — first repeat trips).
        let mut b0 = StepRepetitionBreaker::new(0);
        assert_eq!(b0.observe("t", "{}"), None); // 1
        assert!(b0.observe("t", "{}").is_some()); // 2 → trip
    }

    #[tokio::test]
    async fn run_trips_step_repetition_breaker_on_identical_loop() {
        use kernel_core::Agent;
        // G3: the model re-issues the SAME tool call (name + args) every turn —
        // a classic weak-model loop. With threshold=2 the 2nd identical call
        // halts with the breaker reason, NOT the step-limit message.
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
        let agent = ReactAgent::new(model, reg, "sys")
            .with_max_steps(10)
            .with_step_repetition_threshold(2);
        let s = agent.run(go_input()).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Failed);
        let summary = outcome.output_summary.expect("breaker summary");
        assert!(
            summary.contains("step 重复熔断"),
            "breaker message (not step-limit): {summary}"
        );
    }

    #[tokio::test]
    async fn run_does_not_trip_when_args_vary() {
        use kernel_core::Agent;
        // G3: same tool, DIFFERENT args each turn = exploration, not a loop →
        // breaker must NOT trip; the run converges when the model emits a bare
        // answer on turn 3.
        let mk = |id: &str, args: &str| Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![kernel_core::ToolCall {
                id: id.into(),
                call_type: "function".into(),
                function: kernel_core::FunctionCall {
                    name: "echo".into(),
                    arguments: args.into(),
                },
            }],
            tool_call_id: None,
            reasoning: None,
            reasoning_signature: None,
        };
        let varied = vec![
            mk("c1", r#"{"text":"a"}"#),
            mk("c2", r#"{"text":"b"}"#),
            Message::assistant("done"),
        ];
        let model = ScriptedModel::new(varied);
        let reg = ToolRegistry::new().with(EchoTool);
        let agent = ReactAgent::new(model, reg, "sys")
            .with_max_steps(10)
            .with_step_repetition_threshold(2);
        let s = agent.run(go_input()).unwrap();
        let outcome = collect_outcome(s).await.expect("must emit Done");
        assert_eq!(outcome.status, kernel_core::AgentRunStatus::Completed);
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
        // AnthropicChatModel::build_body. Spans the full bug chain behind session
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

        // Feed that REAL turn-2 history through the REAL AnthropicChatModel
        // build_body: the two consecutive Tool Messages MUST merge into ONE
        // user message, restoring strict user/assistant alternation.
        let glm = AnthropicChatModel::bigmodel("k", "glm-4.6");
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
            Some(pricing::TokenUsage {
                input: 42,
                output: 0,
                cache_read: 0,
                cache_write: 0
            })
        );
        // Standard Anthropic: message_delta carries only output_tokens.
        let delta = r#"data: {"type":"message_delta","usage":{"output_tokens":128}}"#;
        assert_eq!(
            parse_usage(delta),
            Some(pricing::TokenUsage {
                input: 0,
                output: 128,
                cache_read: 0,
                cache_write: 0
            })
        );
        // GLM: message_delta ALSO carries the real input_tokens (message_start's
        // is a 0 placeholder). parse_usage reads both → the caller's
        // saturating_add recovers the real input. Without this the streaming
        // path undercounted input tokens to 0.
        let glm_delta =
            r#"data: {"type":"message_delta","usage":{"input_tokens":16,"output_tokens":10}}"#;
        assert_eq!(
            parse_usage(glm_delta),
            Some(pricing::TokenUsage {
                input: 16,
                output: 10,
                cache_read: 0,
                cache_write: 0
            })
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
            pricing::TokenUsage {
                input: 10,
                output: 20,
                cache_read: 0,
                cache_write: 0
            }
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
        let m = AnthropicChatModel::bigmodel("k", "glm-4.6")
            .with_circuit(std::sync::Arc::new(CircuitBreaker::new(
                CircuitBreakerConfig {
                    failure_threshold: 1,
                    cooldown: Duration::from_secs(60),
                    half_open_max: 1,
                },
            )))
            .with_cost_sink(std::sync::Arc::new(NullCostSink));
        assert!(m.shared.circuit.is_some());
        assert!(m.shared.cost_sink.is_some());
    }

    #[test]
    fn shared_anthropic_circuit_returns_same_instance() {
        // The breaker must be a process-wide singleton so a trip in one agent
        // is observed by all — two calls must hand back the *same* Arc.
        let a = shared_anthropic_circuit();
        let b = shared_anthropic_circuit();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn build_body_enables_thinking_and_replays_preserved_thinking_block() {
        use kernel_core::{Message, ModelOptions, Role, ThinkingConfig};
        let model = AnthropicChatModel::bigmodel("k", "glm-4.6");
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
        let model = AnthropicChatModel::bigmodel("k", "glm-4.6");
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
        let model = AnthropicChatModel::bigmodel("k", "glm-4.6");
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
    // Fixtures: tests/fixtures/anthropic/. To re-record (needs a live key) rerun the
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
        let raw = include_str!("../../tests/fixtures/anthropic/nonstream_text.json");
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
        let raw = include_str!("../../tests/fixtures/anthropic/nonstream_text.json");
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
        let sse = include_str!("../../tests/fixtures/anthropic/stream_text.sse");
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
        let sse = include_str!("../../tests/fixtures/anthropic/stream_text.sse");
        let (_msgs, usage) = replay_sse(sse);
        assert_eq!(
            usage.input, 16,
            "input_tokens accumulated from message_delta"
        );
        assert_eq!(usage.output, 10);
    }

    #[test]
    fn handle_sse_line_assembles_real_glm_tool_use() {
        // Real GLM tool_use stream: index 0 text block, index 1 tool_use block.
        // GLM sent the whole partial_json in one input_json_delta; message_stop
        // reassembles it into a terminal tool_calls Message (id/name/args).
        let sse = include_str!("../../tests/fixtures/anthropic/stream_tool_use.sse");
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
        let sse = include_str!("../../tests/fixtures/anthropic/stream_tool_use_fragmented.sse");
        let (msgs, _usage) = replay_sse(sse);
        let terminal = msgs.last().expect("terminal tool_calls");
        assert_eq!(terminal.tool_calls.len(), 1);
        let tc = &terminal.tool_calls[0];
        assert_eq!(tc.id, "call_frag");
        assert_eq!(tc.function.name, "get_weather");
        assert_eq!(tc.function.arguments, "{\"city\":\"Beijing\"}");
    }

    // ===== live GLM smoke (#[ignore]: needs GLM_API_KEY, spends tokens) =====

    /// Cost sink that captures the last usage AnthropicChatModel reported, so the live
    /// smoke test can assert input_tokens > 0 after the parse_usage fix.
    struct CapturingSink(std::sync::Mutex<crate::cost::pricing::TokenUsage>);

    impl CapturingSink {
        fn new() -> Self {
            Self(std::sync::Mutex::new(
                crate::cost::pricing::TokenUsage::default(),
            ))
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
        let model = AnthropicChatModel::bigmodel(key, "glm-4.6").with_cost_sink(
            std::sync::Arc::clone(&sink) as std::sync::Arc<dyn crate::cost::sink::CostSink>,
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
        let model = AnthropicChatModel::bigmodel(key, "glm-4.6").with_cost_sink(
            std::sync::Arc::clone(&sink) as std::sync::Arc<dyn crate::cost::sink::CostSink>,
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
            None, // compaction_blocks — test agents don't persist Compact events
            None, // approval — test agents don't run the Human Gate
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
            None, // compaction_blocks — test agents don't persist Compact events
            None, // approval — test agents don't run the Human Gate
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
        let glm = AnthropicChatModel::bigmodel(&key, "glm-4.6");
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
            None, // compaction_blocks — test agents don't persist Compact events
            None, // approval — test agents don't run the Human Gate
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
    /// "could not be recovered" message) and calls AnthropicChatModel.stream() with
    /// the real reqwest client, so the raw Error::Model("LLM stream failed: N")
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
        let model = AnthropicChatModel::new(&r.endpoint, &r.api_key, &r.model);
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
            None, // compaction_blocks — test agents don't persist Compact events
            None, // approval — test agents don't run the Human Gate
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
