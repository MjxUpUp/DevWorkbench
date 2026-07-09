//! ReactAgent construction for the self-built transparent kernel path.
//!
//! `build_react_agent` resolves credentials from the user's `providers.toml`,
//! wires the tool registry (built-ins + skills + MCP + subagent dispatcher), the
//! hook pipeline, tier routing, and compaction — then returns a ready-to-run
//! `ReactAgent`. The chat driver (`commands::agents`) and the ACP server are the
//! two live callers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kernel_core::{Tool, ToolContext};

use crate::db::DbState;
use crate::kernel_impl::anthropic_chat_model::AnthropicChatModel;
use crate::kernel_impl::mcp_tool::McpTool;
use crate::kernel_impl::openai_chat_model::OpenAIChatModel;
use crate::kernel_impl::react_agent::{ReactAgent, ToolRegistry};
use crate::kernel_impl::skill_tool::SkillTool;
use crate::mcp::registry::McpRegistry;

/// Construct the right [`kernel_core::ChatModel`] impl for a resolved provider's
/// wire `protocol`: Anthropic → [`AnthropicChatModel`]; OpenAI →
/// [`OpenAIChatModel`]. Each gets its protocol-specific process-wide circuit
/// breaker (so one protocol's outage doesn't trip the other) plus the shared
/// cost/trace/timing/session sinks. Extracted as a free function (no ReactAgent
/// started) so the protocol dispatch is unit-testable in isolation.
fn build_chat_model(
    protocol: crate::config::providers::ProtocolKind,
    endpoint: &str,
    api_key: &str,
    model: &str,
    cost_sink: Arc<dyn crate::cost::sink::CostSink>,
    trace_sink: Arc<dyn crate::trace::TraceSink>,
    session_id: Option<String>,
    timing_checker: Arc<crate::trace::TimingChecker>,
    // A1: span_name labels the root span this model attributes its LLM calls to.
    // "agent" = an orchestrator/main agent; Sub-agents fork a child span under
    // this root.
    span_name: &str,
) -> Arc<dyn kernel_core::ChatModel> {
    match protocol {
        crate::config::providers::ProtocolKind::Anthropic => Arc::new(
            AnthropicChatModel::new(endpoint, api_key, model)
                .with_circuit(crate::kernel_impl::anthropic_chat_model::shared_anthropic_circuit())
                .with_cost_sink(cost_sink)
                .with_trace_sink(trace_sink)
                .with_session_id(session_id)
                .with_timing_checker(timing_checker)
                .with_span(crate::kernel_impl::chat_model_shared::SpanContext::root(
                    span_name,
                )),
        ),
        crate::config::providers::ProtocolKind::OpenAI => Arc::new(
            OpenAIChatModel::new(endpoint, api_key, model)
                .with_circuit(crate::kernel_impl::openai_chat_model::shared_openai_circuit())
                .with_cost_sink(cost_sink)
                .with_trace_sink(trace_sink)
                .with_session_id(session_id)
                .with_timing_checker(timing_checker)
                .with_span(crate::kernel_impl::chat_model_shared::SpanContext::root(
                    span_name,
                )),
        ),
    }
}

/// Build a transparent ReactAgent, resolving credentials from the user's
/// `providers.toml` (gap-②). The default model is data-driven — the first
/// enabled Strong-tier model in `providers.toml` (the kernel-internal
/// self-hosted path), falling back to any enabled model and ultimately to
/// `glm-4.6` as a last-resort literal — overridable via `spec.model`. Flagship
/// models stay on the opaque path (claude/codex/gemini)
/// where the user selects them.
///
/// If no enabled+keyed provider serves the requested model (e.g. the user
/// hasn't filled an API key yet), we fall back to an empty-key default Z.AI
/// model: the agent still CONSTRUCTS (so the run doesn't crash), but GLM
/// calls fail at request time with 401 — the honest signal that Settings →
/// Providers needs a key.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_react_agent(
    model: Option<&str>,
    mcp: Option<&McpRegistry>,
    working_dir: &str,
    conversation_id: Option<&str>,
    history: Vec<kernel_core::Message>,
    db: Option<DbState>,
    mode: crate::kernel_impl::hooks::PermissionMode,
    task_ref: Option<&str>,
    session_id: Option<&str>,
    // Per-node skill filter: only register skills in this list (by name).
    // None = register all installed skills.
    skill_filter: Option<&[String]>,
    // Per-node MCP tool filter: only register tools matching these patterns
    // ("server/tool" or "server/*"). None = register all enabled MCP tools.
    mcp_filter: Option<&[String]>,
    // AppHandle for the chat path — used to wire the compaction-archive sink so
    // dropped原文 is persisted + the Compact event reaches the driver. None for
    // ACP/test agents (compaction runs but stays silent — no archive, no UI).
    app: Option<tauri::AppHandle>,
    // Shared buffer for collecting Compact meta-events so the driver can persist
    // them into session.blocks. None for ACP/test agents. v1.3 C2.
    compaction_blocks: Option<std::sync::Arc<std::sync::Mutex<Vec<crate::agents::pty::ChatStreamEvent>>>>,
    // Human Gate approval registry (Clutch #3). When set AND mode == HumanGate,
    // destructive tool calls suspend for interactive approval. None = gate off.
    approval: Option<crate::kernel_impl::human_gate::ApprovalMap>,
) -> Result<ReactAgent, String> {
    // P1: snapshot the first user message BEFORE the method chain below, which
    // moves `history` into the agent. The classifier needs the prompt text to
    // pick a TaskKind-appropriate budget. See resource_budget.rs for the table.
    let first_user_prompt: String = history
        .iter()
        .find(|m| matches!(m.role, kernel_core::Role::User))
        .map(|m| m.content.clone())
        .unwrap_or_default();
    let budget_kind = crate::kernel_impl::resource_budget::classify_task_kind(&first_user_prompt);
    let budget = crate::kernel_impl::resource_budget::ResourceBudget::for_kind(budget_kind);
    log::info!(
        "[executor] adaptive budget: kind={:?} max_steps={} max_input_tok={} wallclock={}s",
        budget_kind,
        budget.max_steps,
        budget.max_input_tokens,
        budget.max_wallclock_secs,
    );

    let data_dir = crate::commands::projects::dirs_home().join(".dev-workbench");
    let config = crate::config::providers::load_providers_config(&data_dir).ok();
    // model=None → request the '__default__' alias; resolve_provider expands it
    // to the user's configured default (modelMapping['__default__']) or the
    // data-driven default_model_id. Keeps the executor free of any hardcoded
    // vendor model id (the old unwrap_or("glm-4.6")).
    let model_id = model
        .map(|m| m.to_string())
        .unwrap_or_else(|| "__default__".to_string());
    // Keep the full ResolvedProvider around (not just the 4 wire fields) so the
    // tier pair + protocol stay available for routing (node C) and protocol
    // dispatch (node E) without re-resolving.
    let resolved = config
        .as_ref()
        .and_then(|c| crate::config::providers::resolve_provider(c, &model_id));
    let (endpoint, api_key, resolved_model, context_window) = match &resolved {
        Some(r) => (
            r.endpoint.clone(),
            r.api_key.clone(),
            r.model.clone(),
            r.context_window,
        ),
        None => (
            "https://open.bigmodel.cn/api/anthropic".to_string(),
            String::new(),
            model_id.clone(),
            None,
        ),
    };
    // Tool selection discipline lives in the BASE prompt so it always reads as
    // the agent's standing rule, not incidental context. Without it the model
    // under-uses skills (it doesn't realize skill__lark-doc exists until it
    // scans every tool description) and leans on raw bash for tasks a skill
    // already covers — the lark-cli-vs-skill regression.
    let mut sys_prompt = String::from(BASE_SYSTEM_PROMPT);
    // gap1: inject project context (name + cwd + git branch + stack fingerprint)
    // right after the BASE identity/discipline so the agent knows WHERE it is
    // working from turn 1. Regression: session 016ab47e — the prompt never
    // named the project, so a weaker model replied "I can't see the current
    // project" and burned 1953 blocks rediscovering its cwd via glob/read_file.
    // Strong models had masked the gap by exploring; this fixes it for all
    // tiers. Mirrors CCB's opening environment banner.
    sys_prompt.push_str(&project_context_suffix(working_dir));

    // T10 cost budget hard limit: clone the pool before db moves into the cost
    // sink. Called at the top of every turn; if month-to-date spend has reached
    // the configured budget, the agent halts gracefully. A DB read failure is
    // treated as "not exhausted" — never block the run on a transient DB error.
    let budget_db = db.clone();
    let budget_check: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
        budget_db
            .as_ref()
            .and_then(|d| d.get().ok())
            .map(|conn| crate::cost::agentfare::is_budget_exhausted(&conn).unwrap_or(false))
            .unwrap_or(false)
    });
    // D2 user-configurable lifecycle hooks: cloned before `db` moves into the
    // cost sink (same pattern as budget_db above) so we can load the enabled
    // user_hooks rows and register a UserCommandHook per row after the
    // HookManager is built. Best-effort — a DB read failure yields no user hooks.
    let hooks_db = db.clone();
    // Shared as `Arc<dyn ChatModel>` so the subagent dispatcher (v2.0 T2) can
    // hand the SAME model handle to child agents instead of re-wrapping it.
    // Trace + cost sinks both need `db`; cost moves it, so build the trace sink
    // from a clone first. conversation_id is cost's attribution key; session_id
    // is trace's per-turn row key — a failing session is ONE turn, so traces
    // must key on session_id or the failed turn's req/resp is unfindable.
    let conversation_id_owned = conversation_id.map(|s| s.to_string());
    let session_id_owned = session_id.map(|s| s.to_string());
    let trace_sink = crate::trace::sink::optional_shared(db.clone(), conversation_id_owned.clone());
    // Whether the resolved provider declared a Strong+Cheap tier pair — gates
    // the per-step router below. This is the data-driven replacement for the old
    // `starts_with("glm-")` family guard: ANY provider that declares both tiers
    // (Z.AI glm-4.6/flash, or any other pair) gets routing; a single-model
    // provider doesn't, so no foreign id is ever pushed onto its endpoint.
    let tierable = resolved
        .as_ref()
        .map(|r| r.strong_model.is_some() && r.cheap_model.is_some())
        .unwrap_or(false);
    // The provider's tier pair, materialized once for both the per-step router
    // (wired on the agent below) and the sub-agent dispatcher (inherited by
    // SubAgentTool so children + grandchildren route within the same pair).
    let tier_ctx: Option<crate::kernel_impl::model_router::TierCtx> = if tierable {
        let r = resolved
            .as_ref()
            .expect("tierable implies the provider resolved with a tier pair");
        Some(crate::kernel_impl::model_router::TierCtx {
            strong: r.strong_model.clone().unwrap(),
            cheap: r.cheap_model.clone().unwrap(),
        })
    } else {
        None
    };
    // Pick the ChatModel impl by the resolved provider's wire protocol
    // (Anthropic vs OpenAI). A None resolved (no usable provider/key) defaults
    // to Anthropic against the caller's endpoint so construction never panics —
    // the call fails at HTTP time instead of crashing the run.
    let protocol = resolved.as_ref().map(|r| r.protocol).unwrap_or_default();
    let chat: Arc<dyn kernel_core::ChatModel> = build_chat_model(
        protocol,
        &endpoint,
        &api_key,
        &resolved_model,
        // P0 model orchestration: cost sink records token usage + derived cost
        // per request (conversation_id is the attribution key).
        crate::cost::sink::optional_shared(db, "react_kernel", conversation_id_owned),
        trace_sink,
        session_id_owned,
        // B3 timing observability: flags slow LLM turns (latency > 60s, or ttfb
        // > 30s) via a warn log. Pure observability (no gating) so a hardcoded
        // default threshold is honest here.
        std::sync::Arc::new(crate::trace::TimingChecker::default_threshold()),
        "agent",
    );

    // Build the tool registry: skills + MCP tools + the subagent dispatcher.
    // Skills search every dir `skill_catalog` (skills_cmds.rs) scans (see
    // skills_search_dirs) — not just the always-empty `~/.dev-workbench/skills`
    // — so the agent actually sees installed skills instead of reporting "I
    // only have dispatch_subagent, I can't see skills". load_dir is resilient
    // to malformed SKILL.md frontmatter (skill_tool fallback), so no skill is
    // dropped on a parse hiccup. An empty registry leaves the agent chat-only;
    // a populated one activates the tool loop + ToolCall events end-to-end.
    let mut registry = ToolRegistry::new();
    // Built-in coding tools FIRST: read_file/glob/grep are read-only so they
    // enter the sub-agent's read_only_subset (snapshot below), letting a child
    // investigate too. bash/write_file are NOT read-only → auto-excluded from
    // the child (a sub-agent can't mutate). Without these the agent could only
    // dispatch_subagent — it had no way to read a file or run a command itself.
    registry.push(crate::kernel_impl::builtin_tools::ReadFileTool);
    registry.push(crate::kernel_impl::builtin_tools::GlobTool);
    registry.push(crate::kernel_impl::builtin_tools::GrepTool);
    // Mission Phase 2 (Executing): the controller must NOT run shell commands
    // itself — it delegates all work to sub-agents. BashTool is the strongest
    // implementation tool, so we withhold it; read/glob/grep/write (the latter
    // to update prd.json `passes`) stay available. Mirrors QwenPaw's "deactivate
    // implementation tools" in Phase 2 (write_file kept; edit_file/browser not).
    if mode != crate::kernel_impl::hooks::PermissionMode::Executing {
        registry.push(crate::kernel_impl::builtin_tools::BashTool);
    }
    registry.push(crate::kernel_impl::builtin_tools::WriteFileTool);
    let home = crate::commands::projects::dirs_home();
    for dir in skills_search_dirs(&home, working_dir, &data_dir) {
        for skill in SkillTool::load_dir(&dir) {
            // Per-node skill filter: only register skills in the filter list
            // (by name). None = register all installed skills.
            let name = skill.info().name.clone();
            if skill_filter.map_or(true, |list| list.iter().any(|s| s == &name)) {
                registry.push(skill);
            }
        }
    }
    if let Some(reg) = mcp {
        // get_tools() is synchronous stdio I/O — acceptable as a one-shot cost
        // at agent construction. A slow/unreachable server is logged + skipped
        // inside the registry, never fatal.
        if let Ok(all) = reg.get_tools() {
            for (server, list_json) in all {
                if let Some(client) = reg.get_client(&server) {
                    for tool in McpTool::from_list_result(&server, &list_json, client) {
                        // Per-node MCP filter: "server/tool" exact or "server/*" wildcard
                        let info = tool.info();
                        let full_name = format!(
                            "{}/{}",
                            server,
                            info.name
                                .strip_prefix(&format!("mcp__{}__", server))
                                .unwrap_or(&info.name)
                        );
                        if mcp_filter.map_or(true, |list| {
                            list.iter().any(|pat| {
                                pat == &full_name
                                    || pat == &format!("{}/*", server)
                                    || pat == &info.name
                            })
                        }) {
                            registry.push(tool);
                        }
                    }
                }
            }
        }
    }

    // v2.0 T2: subagent dispatch. The child reuses this agent's model handle
    // and gets the read-only tool subset — it can investigate (search/read) but
    // not mutate, and cannot dispatch further subagents (the dispatcher isn't
    // read-only), bounding recursion at depth 1. Snapshot the subset BEFORE
    // pushing the dispatcher itself so it isn't included.
    //
    // D1: named sub-agents from .agents/subagents/<name>/AGENT.md let the agent
    // delegate BY NAME via dispatch_subagent {subagent: "researcher"}. An empty
    // list (no such dir on disk) preserves the legacy anonymous-worker behavior.
    let mut subagents: Vec<crate::kernel_impl::subagent_spec::SubAgentSpec> = Vec::new();
    for dir in subagents_search_dirs(&home, working_dir, &data_dir) {
        subagents.extend(crate::kernel_impl::subagent_spec::load_subagents(&dir));
    }
    // C2/D3 subagent concurrency limiter: a parent that fans out N
    // dispatch_subagent calls in ONE turn runs them concurrently (see
    // ReactAgent::execute_call_set), bounded to 4 in-flight children. Wide
    // enough to parallelize a real fan-out, narrow enough not to blow the
    // model rate budget or starve the parent's own turns.
    let subagent_concurrency = Arc::new(tokio::sync::Semaphore::new(4));
    registry.push(
        crate::kernel_impl::react_agent::SubAgentTool::new_with_concurrency(
            Arc::clone(&chat),
            registry.read_only_subset(),
            8,
            subagents.clone(),
            Arc::clone(&subagent_concurrency),
            tier_ctx.clone(),
        ),
    );
    // C1 — dispatch_acp_agent: delegate a sub-task to an EXTERNAL ACP-speaking
    // coding agent (codex-acp / claude via ACP) over stdio JSON-RPC. Sibling of
    // dispatch_subagent, but drives a separate agent the kernel can't become.
    registry.push(crate::kernel_impl::acp_tool::AcpAgentTool::default());

    // Surface installed skills + MCP tools BY NAME in the system prompt, not just
    // in the tool-list descriptions. The model otherwise can't tell which skill__
    // names exist without scanning every tool's description, so it under-uses
    // skills and falls back to bash (the discipline above only sticks if the
    // agent can SEE the skill exists). Built-in read_file/bash/etc. and the
    // subagent dispatcher are deliberately omitted — they're already prominent.
    sys_prompt.push_str(&installed_skills_appendix(&registry.infos()));
    sys_prompt.push_str(&subagents_appendix(&subagents));

    let ctx = ToolContext {
        working_dir: Some(working_dir.to_string()),
        conversation_id: conversation_id.map(|s| s.to_string()),
    };
    // Mount the kernel hook pipeline (the Forge task-guard / bash-guard /
    // assertion-check analogs). CommandGuard vetoes destructive shell commands;
    // AssertionGuard watches file-write diffs for assertion weakening; TaskGuard
    // gates file writes by task boundary — a session bound to a task (task_ref)
    // may write inside its working_dir, writes outside are blocked, and a
    // session with NO task only warns (never bricks the agent's own file-writing
    // tools, the reason it was previously deferred).
    let mut hooks = crate::kernel_impl::hooks::HookManager::new().with_mode(mode);
    hooks.register(Box::new(
        crate::kernel_impl::hooks::CommandGuardHook::default(),
    ));
    hooks.register(Box::new(crate::kernel_impl::hooks::AssertionGuardHook));
    hooks.register(Box::new(crate::kernel_impl::hooks::TaskGuardHook::new(
        task_ref.map(|s| s.to_string()),
        Some(std::path::PathBuf::from(working_dir)),
    )));
    // D2: register every ENABLED user_hooks row as a UserCommandHook. Each hook
    // no-ops on events it isn't bound to, so loading all four events into one
    // HookManager is correct — the manager dispatches every HookEvent and each
    // row only acts on its configured event (Pre/PostToolUse rows additionally
    // gate on their matcher at dispatch time). A DB read failure is swallowed
    // (the run proceeds with built-in hooks only).
    if let Some(dbs) = hooks_db.as_ref() {
        if let Ok(conn) = dbs.get() {
            use crate::models::UserHookEvent;
            let mut rows = Vec::new();
            for ev in [
                UserHookEvent::UserPromptSubmit,
                UserHookEvent::PreToolUse,
                UserHookEvent::PostToolUse,
                UserHookEvent::Stop,
            ] {
                rows.extend(
                    crate::user_hooks::registry::load_enabled_by_event(&conn, ev)
                        .unwrap_or_default(),
                );
            }
            for row in rows {
                hooks.register(Box::new(
                    crate::user_hooks::UserCommandHook::new(
                        row.name,
                        row.event,
                        row.command,
                        row.shell,
                        row.timeout_secs,
                        Some(std::path::PathBuf::from(working_dir)),
                    )
                    .with_matcher(row.matcher.clone()),
                ));
            }
        }
    }
    let agent = ReactAgent::new_shared(chat, registry, sys_prompt)
        .with_context(ctx)
        .with_history(history)
        .with_thinking(2048)
        .with_max_verify(1);
    // T9 per-step routing: the cheap tier for low-stakes turns (tool-result
    // echoes, confirmations), the strong tier for planning/reasoning. Provider-
    // agnostic since the multi-protocol refactor — route_step swaps within the
    // resolved provider's OWN declared tier pair (Z.AI glm-4.6↔flash, or any
    // other), so endpoint/key stay constant. Wire-time gate: only providers that
    // declared BOTH tiers (`tierable`) get the router; a single-model provider
    // keeps opts.model=None and the ChatModel falls back to its resolved id.
    // route_step's own base guard (base ≠ strong ⇒ returned unchanged) is the
    // second line of defense against pushing a pair's id onto a foreign endpoint
    // (session 1ef23cbc: a DeepSeek endpoint must never receive glm-4.6).
    let agent = if let Some(tier) = tier_ctx.clone() {
        agent
            // Carry the pair on the agent first (clones the ids, doesn't move
            // tier), so dispatched sub-agents inherit it; then move tier into the
            // router closure.
            .with_tier_ctx(tier.strong.clone(), tier.cheap.clone())
            .with_model_router(Arc::new(move |h, b| {
                crate::kernel_impl::model_router::route_step(h, b, &tier)
            }))
    } else {
        agent
    };
    // P1: budget snapshot already taken at function top — see lines just below
    // the signature. Here we only consume `budget.max_steps` inside the chain.
    let agent = agent
        .with_budget_check(budget_check)
        // v1.3 C1 + B-plan §4.2 缺项4: the compaction hard ceiling is the
        // model's EFFECTIVE context window — `window − reserved_output` (CCB
        // `getEffectiveContextWindowSize`), NOT 75% of the window. The soft
        // trigger then subtracts the 13k autocompact buffer (`trigger_threshold`
        // in context_compact.rs). Old 75%-then-60% double-discounted to a 90k
        // trigger on a 200k window (45% utilization); the flat-output-reserve
        // model compacts at ~167k (84%), matching CCB's 80–92% band. Unknown
        // window → 24k (legacy default). See `effective_context_window`.
        .with_context_compaction(effective_context_window(context_window), 8)
        // P1: budget.max_steps replaces the old fixed-30. The 30-cap is gone.
        .with_max_steps(budget.max_steps)
        .with_hooks(Arc::new(hooks));
    // v1.3 C2: wire the compaction archive sink for the chat path only. All
    // three (session_id + app + compaction_blocks) are present → the sink
    // archives dropped原文, emits the Compact agent:event, and appends to the
    // driver's final_blocks. ACP/test agents pass None for at least one → skip,
    // and compaction stays silent (the original behavior).
    let agent = match (session_id, app, compaction_blocks) {
        (Some(sid), Some(handle), Some(buf)) => {
            agent.with_compaction_archive(sid.to_string(), handle, buf)
        }
        _ => agent,
    };
    // v2 Human Gate: wire the approval registry for any path that supplies one
    // and isn't in a guard-skipping mode. Chat path passes `approval` (the
    // ApprovalMap from app state); replay/ACP/test paths pass None → ungated.
    // The gate attaches in Default/Plan/DryRun/HumanGate so destructive ops
    // (rm, git push --force, git reset --hard, shred, …) surface an
    // ApprovalModal before running — closing the dead path where ChatView always
    // sent mode=undefined → Default → the registry was never wired → the modal
    // never fired (the "破坏性操作由 ApprovalModal 承接" comment was a wrong
    // self-description). SkipPermissions (yolo) waives the INTERACTIVE approval;
    // the CatastropheGuard hard floor in HookManager::before still blocks
    // irreversible system-destruction regardless of mode. The HumanGate
    // PermissionMode variant stays as an explicit declarator for a future
    // stricter mode.
    let agent = match approval {
        Some(ap) if !mode.skips_guards() => agent.with_human_gate(ap),
        _ => agent,
    };
    Ok(agent)
}

/// Skill directories a kernel agent searches, in priority order: global
/// `~/.agents/skills`, project `<cwd>/.agents/skills`, then the app-private
/// `~/.dev-workbench/skills` legacy fallback. Mirrors what `skill_catalog`
/// (skills_cmds.rs) scans so the agent's registry matches the Skills Market —
/// the previous single `~/.dev-workbench/skills` lookup was always empty, which
/// is why the agent reported "I only have dispatch_subagent, I can't see
/// skills". Extracted from build_react_agent so the directory set is testable.
fn skills_search_dirs(home: &Path, working_dir: &str, data_dir: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".agents").join("skills"),
        PathBuf::from(working_dir).join(".agents").join("skills"),
        data_dir.join("skills"),
    ]
}

/// D1: directories scanned for named sub-agents (`.agents/subagents/<name>/
/// AGENT.md`). Same three-tier layout as [skills_search_dirs] — global → project
/// → app-private — so a project can shadow a global sub-agent of the same name.
/// `dispatch_subagent` resolves the first match, so global takes precedence on a
/// duplicate (consistent with how skills_search_dirs orders skill loading).
fn subagents_search_dirs(home: &Path, working_dir: &str, data_dir: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".agents").join("subagents"),
        PathBuf::from(working_dir).join(".agents").join("subagents"),
        data_dir.join("subagents"),
    ]
}

/// The kernel agent's standing system prompt: identity + tool-selection
/// discipline. Asserted by `base_prompt_carries_tool_selection_discipline`.
const BASE_SYSTEM_PROMPT: &str = concat!(
    "You are a Dev Workbench kernel agent. Complete the task concisely.\n",
    "\n",
    "Tool selection discipline (IMPORTANT):\n",
    "- Prefer a matching domain skill (skill__*) or MCP tool (mcp__*) over raw ",
    "bash. A skill packages the canonical call sequence and pre-checks; invoking ",
    "it returns its how-to, so you don't reinvent the invocation or guess its flags.\n",
    "- Only fall back to `bash` for a CLI when NO skill/mcp covers it, and check ",
    "the CLI's --help first if you are unsure of its flags.\n",
    "- Do NOT replay a command just because a prior turn used it — judge each task ",
    "fresh and route through the matching abstraction. Conversation history carries ",
    "NO tool calls precisely so past tool choices cannot bias the next run.\n",
    "- For code work: investigate with read_file/glob/grep before writing ",
    "(write_file/bash), then verify with the project's own tests/build.\n",
    "\n",
    "Response length discipline (P2 source-level guard against stream truncation):\n",
    "- Keep each reply under 600 tokens of plain prose. Long reasoning belongs in ",
    "`thinking`, not in `text`.\n",
    "- If a tool result is large, summarize it inline before continuing — don't ",
    "echo 10k tokens of grep output back as text.\n",
    "- Final answers should be a tight paragraph or a bullet list, not an essay. ",
    "The session replays the last assistant text block; long blocks raise the ",
    "odds of stream truncation and proxy buffer overflow.\n",
    "\n",
    "Sub-agent delegation discipline (dispatch_subagent):\n",
    "- The child ReactAgent is capped at 8 steps for FOCUSED, bounded work only ",
    "(read a few files / answer one scoped question). It is NOT for broad ",
    "multi-source research or any task needing 5+ tool calls.\n",
    "- If a task is open-ended — '调研业界做法', '摸清整个项目', multi-file ",
    "synthesis, or you cannot name its single concrete deliverable — do it ",
    "YOURSELF with your own budget. Handing such a task to the child ",
    "exhausts its 8 steps without a final answer and wastes the whole turn ",
    "(regression: a parent once dispatched 'evaluate the project' to the child, ",
    "which ran 8 steps and failed; the parent redid the work itself anyway).\n",
);

/// Project name = directory basename of the working dir. Falls back to the
/// working_dir string itself when the basename is empty (root or trailing-sep
/// path), so the prompt always carries SOMETHING. Pure over `&str`.
fn project_name(working_dir: &str) -> String {
    Path::new(working_dir)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| working_dir.to_string())
}

/// Detect the project's stack fingerprint by probing well-known manifests in
/// the working directory (existence only — no parse, so it stays cheap and
/// never panics on a malformed manifest). Order is specificity-first:
/// `src-tauri/tauri.conf.json` wins over bare Cargo/package because a Tauri
/// app is both Rust and web frontend, and that framing is the actionable bit.
/// Returns None when no manifest is recognized. Pure over `&Path`.
fn detect_project_stack(project: &Path) -> Option<String> {
    let has_pkg = project.join("package.json").exists();
    let has_cargo = project.join("Cargo.toml").exists();
    if project.join("src-tauri/tauri.conf.json").exists() {
        return Some("Tauri (Rust + web frontend)".to_string());
    }
    if has_cargo && has_pkg {
        return Some("Rust + Node".to_string());
    }
    if has_cargo {
        return Some("Rust".to_string());
    }
    if has_pkg {
        return Some("Node".to_string());
    }
    if project.join("go.mod").exists() {
        return Some("Go".to_string());
    }
    if project.join("pyproject.toml").exists() || project.join("requirements.txt").exists() {
        return Some("Python".to_string());
    }
    if project.join("pom.xml").exists() || project.join("build.gradle").exists() {
        return Some("JVM".to_string());
    }
    None
}

/// `git rev-parse --abbrev-ref HEAD` — the current branch name, for system-prompt
/// context injection so the model knows which branch it is on WITHOUT running
/// git itself (it lacks a shell-by-shell cwd guarantee and weak models fumble
/// the invocation). Returns None on any failure (non-repo, detached HEAD `HEAD`,
/// git missing); CREATE_NO_WINDOW on Windows so no console flashes.
fn git_current_branch(project: &Path) -> Option<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("rev-parse")
        .arg("--abbrev-ref")
        .arg("HEAD")
        .current_dir(project);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        // Detached HEAD resolves to the literal "HEAD" — not a branch name, so
        // omit it (the model would only be confused by "- Git branch: HEAD").
        .filter(|s| !s.is_empty() && s != "HEAD")
}

/// Build the project-context block injected into the system prompt so the agent
/// knows WHERE it is working without probing (regression: session 016ab47e — a
/// weaker model replied "I can't see the current project" because the prompt
/// never named the project; strong models had masked the gap by glob/read_file
/// exploration). Mirrors CCB's opening environment banner (working dir / git
/// branch / platform). Every sub-field degrades to omission rather than failing
/// agent build — a non-repo or unrecognized stack just yields a shorter block.
/// No mutation; deterministic given the filesystem state (reads git/manifest
/// but writes nothing), so the shape is unit-testable with a temp dir.
fn project_context_suffix(working_dir: &str) -> String {
    let project = Path::new(working_dir);
    let mut lines: Vec<String> = vec![
        "\n\nProject context (you are working inside THIS project):".to_string(),
        format!("- Project: {}", project_name(working_dir)),
        format!("- Current working directory: {working_dir}"),
    ];
    if let Some(branch) = git_current_branch(project) {
        lines.push(format!("- Git branch: {branch}"));
    }
    if let Some(stack) = detect_project_stack(project) {
        lines.push(format!("- Stack: {stack}"));
    }
    lines.join("\n")
}

/// Format the installed `skill__*` and `mcp__*` tools into a system-prompt
/// appendix that names them up front. The model otherwise has to scan every
/// tool's description to learn which skills exist, so it under-uses them and
/// falls back to bash. Built-in tools (read_file/bash/…) and the subagent
/// dispatcher are excluded — they're already prominent, and listing them would
/// bury the skills. Returns "" when no skills/mcp are registered. Pure over
/// `&[ToolInfo]` so it is unit-testable without building a real ToolRegistry.
fn installed_skills_appendix(infos: &[kernel_core::ToolInfo]) -> String {
    let installed: Vec<(&str, &str)> = infos
        .iter()
        .filter(|i| i.name.starts_with("skill__") || i.name.starts_with("mcp__"))
        .map(|i| (i.name.as_str(), i.description.as_str()))
        .collect();
    if installed.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\nInstalled skills & MCP tools available now:");
    for (name, desc) in &installed {
        // One line each: first non-empty line of the description keeps the prompt
        // bounded even when a SKILL.md frontmatter description is multi-line.
        let one_line = desc
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        out.push_str(&format!("\n- {name}: {one_line}"));
    }
    out
}

/// D1: surface named sub-agents in the system prompt so the agent delegates BY
/// NAME (`dispatch_subagent {subagent: "researcher"}`) instead of always falling
/// back to the anonymous worker. Empty list → empty string (no dangling header),
/// matching [installed_skills_appendix]. First non-empty description line keeps
/// the prompt bounded even when AGENT.md frontmatter description is multi-line.
fn subagents_appendix(specs: &[crate::kernel_impl::subagent_spec::SubAgentSpec]) -> String {
    if specs.is_empty() {
        return String::new();
    }
    let mut out =
        String::from("\n\nNamed sub-agents available for dispatch_subagent {subagent: name}:");
    for s in specs {
        let one_line = s
            .description
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        out.push_str(&format!("\n- {}: {one_line}", s.name));
    }
    out
}

#[cfg(test)]
mod executor_tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn skills_search_dirs_covers_global_project_and_legacy_in_priority_order() {
        let dirs = skills_search_dirs(
            Path::new("/home/u"),
            "/proj",
            Path::new("/home/u/.dev-workbench"),
        );
        // global first, project second, app-private legacy last.
        assert_eq!(dirs[0], PathBuf::from("/home/u/.agents/skills"));
        assert_eq!(dirs[1], PathBuf::from("/proj/.agents/skills"));
        assert_eq!(dirs[2], PathBuf::from("/home/u/.dev-workbench/skills"));
    }

    fn info(name: &str, desc: &str) -> kernel_core::ToolInfo {
        kernel_core::ToolInfo {
            name: name.to_string(),
            description: desc.to_string(),
            parameters_schema: serde_json::json!({}),
        }
    }

    #[test]
    fn base_prompt_carries_tool_selection_discipline() {
        // The discipline must live in the base prompt so it reads as a standing
        // rule. Assert the literal fragments the model is graded against — if the
        // wording changes, this test must change with it (no fuzzy match).
        assert!(BASE_SYSTEM_PROMPT.contains("skill__*"));
        assert!(BASE_SYSTEM_PROMPT.contains("mcp__*"));
        assert!(BASE_SYSTEM_PROMPT.contains("Do NOT replay a command"));
        assert!(BASE_SYSTEM_PROMPT.contains("--help"));
        // History stripping is enforced in react_chat, but the prompt must TELL
        // the agent not to lean on past tool calls — the two changes are a pair.
        assert!(BASE_SYSTEM_PROMPT.contains("NO tool calls"));
        // Sub-agent delegation discipline must also be a standing rule so the
        // agent doesn't hand open-ended research to the 8-step child.
        assert!(BASE_SYSTEM_PROMPT.contains("dispatch_subagent"));
        assert!(BASE_SYSTEM_PROMPT.contains("8 steps"));
        assert!(BASE_SYSTEM_PROMPT.contains("YOURSELF"));
    }

    #[test]
    fn installed_skills_appendix_lists_skills_and_mcp_excludes_builtin() {
        let infos = vec![
            info("read_file", "Read a file"),
            info("bash", "Run a shell command"),
            info("skill__lark-doc", "Read a Feishu/Lark doc"),
            info("mcp__github__create_issue", "Create a GitHub issue"),
        ];
        let app = installed_skills_appendix(&infos);
        // skills + mcp named up front...
        assert!(app.contains("skill__lark-doc"));
        assert!(app.contains("mcp__github__create_issue"));
        assert!(app.starts_with("\n\nInstalled skills"));
        // ...built-ins excluded (already prominent in the tool list / discipline).
        assert!(!app.contains("read_file"));
        assert!(!app.contains("bash"));
    }

    #[test]
    fn installed_skills_appendix_empty_when_only_builtin_tools() {
        // No skills/mcp → no appendix (don't emit a dangling header line).
        let infos = vec![info("read_file", "x"), info("grep", "y")];
        assert_eq!(installed_skills_appendix(&infos), "");
    }

    #[test]
    fn installed_skills_appendix_trims_multiline_description_to_first_line() {
        // A SKILL.md frontmatter description may be multi-line; only the first
        // non-empty line is used so the appendix stays bounded.
        let infos = vec![info(
            "skill__big",
            "   \nDoes the big thing\nand more detail here",
        )];
        let app = installed_skills_appendix(&infos);
        assert!(app.contains("skill__big: Does the big thing"));
        assert!(!app.contains("and more detail here"));
    }

    #[test]
    fn subagents_search_dirs_mirrors_skills_three_tier_layout() {
        // D1: subagent dirs follow the same global → project → app-private
        // ordering as skills_search_dirs (a project can shadow a global).
        let dirs = subagents_search_dirs(
            Path::new("/home/u"),
            "/proj",
            Path::new("/home/u/.dev-workbench"),
        );
        assert_eq!(dirs[0], PathBuf::from("/home/u/.agents/subagents"));
        assert_eq!(dirs[1], PathBuf::from("/proj/.agents/subagents"));
        assert_eq!(dirs[2], PathBuf::from("/home/u/.dev-workbench/subagents"));
    }

    #[test]
    fn subagents_appendix_empty_when_no_named_specs() {
        // No named sub-agents → no appendix (no dangling header in the prompt).
        assert_eq!(subagents_appendix(&[]), "");
    }

    #[test]
    fn subagents_appendix_lists_names_and_first_desc_line() {
        use crate::kernel_impl::subagent_spec::SubAgentSpec;
        let specs = vec![
            SubAgentSpec {
                name: "researcher".into(),
                description: "Deep web research\nlonger detail".into(),
                system_prompt: "x".into(),
                tools_allow: vec![],
            },
            SubAgentSpec {
                name: "test-writer".into(),
                description: "Writes tests".into(),
                system_prompt: "y".into(),
                tools_allow: vec![],
            },
        ];
        let app = subagents_appendix(&specs);
        assert!(app.starts_with("\n\nNamed sub-agents"));
        assert!(app.contains("- researcher: Deep web research"));
        assert!(app.contains("- test-writer: Writes tests"));
        // multi-line description trimmed to the first non-empty line
        assert!(!app.contains("longer detail"));
    }

    #[test]
    fn project_name_extracts_basename() {
        assert_eq!(project_name("/home/u/my-project"), "my-project");
        assert_eq!(project_name("/proj"), "proj");
    }

    #[test]
    fn project_name_falls_back_to_full_path_when_no_basename() {
        // Root "/" has no basename → fall back to the input rather than empty,
        // so the prompt always carries SOMETHING in the Project line.
        assert_eq!(project_name("/"), "/");
        assert_eq!(project_name(""), "");
    }

    #[test]
    fn detect_project_stack_tauri_wins_over_bare_manifests() {
        // src-tauri/tauri.conf.json present alongside bare Cargo+package → the
        // Tauri fingerprint wins (more specific, and the actionable framing).
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src-tauri")).unwrap();
        std::fs::write(root.join("src-tauri/tauri.conf.json"), "{}").unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("package.json"), "{}").unwrap();
        assert_eq!(
            detect_project_stack(root).as_deref(),
            Some("Tauri (Rust + web frontend)")
        );
    }

    #[test]
    fn detect_project_stack_classifies_bare_manifests() {
        fn stack(files: &[&str]) -> Option<String> {
            let tmp = tempfile::TempDir::new().unwrap();
            for f in files {
                std::fs::write(tmp.path().join(f), "").unwrap();
            }
            detect_project_stack(tmp.path())
        }
        assert_eq!(stack(&["Cargo.toml"]).as_deref(), Some("Rust"));
        assert_eq!(stack(&["package.json"]).as_deref(), Some("Node"));
        assert_eq!(
            stack(&["Cargo.toml", "package.json"]).as_deref(),
            Some("Rust + Node")
        );
        assert_eq!(stack(&["go.mod"]).as_deref(), Some("Go"));
        assert_eq!(stack(&["pyproject.toml"]).as_deref(), Some("Python"));
        assert_eq!(stack(&["requirements.txt"]).as_deref(), Some("Python"));
        assert_eq!(stack(&["pom.xml"]).as_deref(), Some("JVM"));
    }

    #[test]
    fn detect_project_stack_unknown_for_empty_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(detect_project_stack(tmp.path()), None);
    }

    #[test]
    fn project_context_suffix_names_project_dir_and_stack() {
        // The suffix MUST name the cwd + stack up front — the whole point of
        // gap1. A temp dir is not a git repo, so the branch line is omitted
        // rather than mislabelled "HEAD".
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "").unwrap();
        let working_dir = tmp.path().to_str().unwrap();
        let suffix = project_context_suffix(working_dir);
        assert!(suffix.contains("Project context"));
        assert!(suffix.contains(&format!("- Current working directory: {working_dir}")));
        assert!(suffix.contains("- Stack: Rust"));
        assert!(!suffix.contains("Git branch"));
    }
}

/// CCB parity (`autoCompact.ts:getEffectiveContextWindowSize`): the model's
/// declared context window MINUS the output tokens reserved for the model's own
/// reply + compaction summary. This is the HARD ceiling passed to
/// `maybe_compact` as `max_tokens`.
///
/// The old `75% of window` heuristic **double-discounted**: 200k → 150k hard
/// ceiling, then `trigger_threshold` took another 60% → 90k soft trigger, so
/// compaction fired at 45% window utilization. Reserving a flat output budget
/// instead leaves the full input headroom: 200k − 20k output = **180k effective**
/// (hard ceiling), then `trigger_threshold` subtracts the 13k autocompact buffer
/// → **167k soft trigger** (84% utilization — matches CCB, which compacts in
/// the 80–92% band).
///
/// `RESERVED_OUTPUT_TOKENS = 20_000` mirrors CCB `MAX_OUTPUT_TOKENS_FOR_SUMMARY`
/// (p99.99 of compact-summary output is 17,387 tokens). CCB does
/// `min(perModelMaxOutput, 20k)`; DW has no per-model maxOutput field, so the
/// 20k ceiling IS the value. Small windows (no real model declares <40k, but
/// misconfigs/guard against 0) cap the reserve at `window/4` so a tiny window
/// isn't entirely eaten by the output reservation (8k → reserve 2k → 6k
/// effective, vs the old 6k — unchanged for the small-model case). Unknown/zero
/// window → 24k (the old hardcoded value, backward-compat for configs that
/// declare no window).
fn effective_context_window(context_window: Option<usize>) -> usize {
    const DEFAULT_WINDOW: usize = 32_000;
    const RESERVED_OUTPUT_TOKENS: usize = 20_000;
    const FALLBACK_EFFECTIVE: usize = 24_000;
    let window = context_window.unwrap_or(DEFAULT_WINDOW);
    if window == 0 {
        return FALLBACK_EFFECTIVE;
    }
    let reserved = RESERVED_OUTPUT_TOKENS.min(window / 4);
    window.saturating_sub(reserved)
}

#[cfg(test)]
mod compact_threshold_tests {
    use super::effective_context_window;

    #[test]
    fn unknown_window_falls_back_to_24k_legacy_default() {
        // A model that never declared a window must behave exactly as before —
        // the old hardcoded 24k constant. This is the backward-compat guarantee.
        assert_eq!(effective_context_window(None), 24_000);
    }

    #[test]
    fn large_window_subtracts_flat_20k_output_reserve() {
        // CCB parity: 200k Claude → 200k − 20k = 180k effective (hard ceiling).
        // The old 75% gave 150k — this recovers 30k of input headroom.
        assert_eq!(effective_context_window(Some(200_000)), 180_000);
        // 128k GLM → 108k (was 96k under 75%).
        assert_eq!(effective_context_window(Some(128_000)), 108_000);
    }

    #[test]
    fn small_window_caps_reserve_at_quarter() {
        // 8k model: reserve = min(20k, 8k/4=2k) = 2k → 6k effective. The flat
        // 20k reserve would eat the whole window; the window/4 cap keeps 6k
        // for input (unchanged from the old 75% = 6k for the small case).
        assert_eq!(effective_context_window(Some(8_000)), 6_000);
    }

    #[test]
    fn zero_window_is_guarded_not_panic() {
        // A misconfigured `context_window = 0` must not underflow; fall back to
        // the legacy effective value rather than crashing the agent build.
        assert_eq!(effective_context_window(Some(0)), 24_000);
    }
}
