//! Per-step model routing (v1.2 T9). AgentFare-inspired rule-based tiering:
//! look at the conversation so far and pick the cheap model for turns that
//! don't need the strong one, keeping the strong model for planning/reasoning/
//! final answers.
//!
//! Provider-agnostic since the multi-protocol refactor: the strong + cheap ids
//! come from a [`TierCtx`] built by the executor from the resolved provider's
//! declared `ModelTier`s (Z.AI: glm-4.6 ↔ glm-4-flash; any provider declaring
//! both tiers routes the same way). Same-provider routing only, so endpoint /
//! api_key stay constant and no second credential is needed. This is AgentFare's
//! "rules → tier → same-provider model" path, ported 1:1; its proxy/hook layer
//! and LLM secondary router are out of scope (a ReactAgent's turn type is
//! structurally known from history, so the rules suffice — no extra LLM call).

use kernel_core::{Message, Role};

/// The strong + cheap model ids for one provider's per-step routing. Replaces
/// the old hardcoded `STRONG_MODEL`/`CHEAP_MODEL` constants: each provider now
/// declares its own pair via `ModelTier` on its `ModelEntry`s, and the executor
/// builds a `TierCtx` from the resolved provider's declared tiers. Routing is
/// `tier.strong ↔ tier.cheap` for ANY provider, not just Z.AI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierCtx {
    pub strong: String,
    pub cheap: String,
}

/// Planning / reasoning / review keywords (EN + ZH) → keep the strong model.
/// Mirrors AgentFare's PLANNING/REASONING/REVIEWING patterns.
const POWERFUL_HINTS: &[&str] = &[
    "plan", "design", "architect", "refactor", "debug", "analyze", "review", "rewrite", "migrate",
    "why", "investigate", "规划", "设计", "重构", "排查", "分析", "审查", "为什么",
];

/// Short yes/no/go confirmations → cheap model. (ZH: 继续/好的/确认/可以.)
const CHEAP_CONFIRM: &[&str] = &[
    "yes", "ok", "done", "continue", "go ahead", "sure", "next", "继续", "好的", "确认", "可以",
    "下一步", "继续吧",
];

/// Decide the model id for the next turn, given the conversation so far, the
/// agent's configured base model, and the provider's tier pair.
///
/// - Base model isn't this provider's declared strong model → returned unchanged
///   (don't silently swap a user's explicit pick; routing only applies within a
///   provider's own tier pair, so a different model can't receive the pair's ids).
/// - A powerful hint in the recent window → strong (planning/reasoning needs it).
/// - Otherwise, if the last message is a tool result (agent branching on tool
///   output) or the current instruction is a short confirmation → cheap.
/// - Default (incl. the first turn parsing a fresh task) → strong.
pub fn route_step(history: &[Message], base_model: &str, tier: &TierCtx) -> String {
    // Only route within the provider's own tier pair.
    if !base_model.eq_ignore_ascii_case(&tier.strong) {
        return base_model.to_string();
    }
    let recent = recent_text(history);
    if POWERFUL_HINTS.iter().any(|h| recent.contains(h)) {
        return tier.strong.clone();
    }
    if last_is_tool_result(history) {
        return tier.cheap.clone();
    }
    if is_short_confirmation(history) {
        return tier.cheap.clone();
    }
    tier.strong.clone()
}

/// Lowercased concatenation of the last few messages' text, for keyword scan.
fn recent_text(history: &[Message]) -> String {
    let mut acc = String::new();
    for m in history.iter().rev().take(4) {
        acc.push_str(&m.content);
        acc.push(' ');
    }
    acc.to_lowercase()
}

/// True when the most recent message is a tool result (a user-role message
/// carrying a tool_call_id — i.e. tool output the agent is about to react to).
fn last_is_tool_result(history: &[Message]) -> bool {
    history
        .last()
        .map(|m| m.role == Role::User && m.tool_call_id.is_some())
        .unwrap_or(false)
}

/// True when the current instruction (last real user message, not a tool result)
/// is a short confirmation like "ok" / "继续".
fn is_short_confirmation(history: &[Message]) -> bool {
    let Some(inst) = last_instruction(history) else {
        return false;
    };
    let c = inst.content.trim().to_lowercase();
    if c.len() >= 32 {
        return false;
    }
    CHEAP_CONFIRM.iter().any(|k| c == *k || c.starts_with(k))
}

/// The most recent user message that is NOT a tool result (the live instruction).
fn last_instruction(history: &[Message]) -> Option<&Message> {
    history
        .iter()
        .rev()
        .find(|m| m.role == Role::User && m.tool_call_id.is_none())
}

// ---------------------------------------------------------------------------
// Sub-agent dispatch tiering (the "model half" of dispatch_subagent)
// ---------------------------------------------------------------------------
//
// Per-turn [`route_step`] above decides which model fits ONE turn of a
// conversation. Dispatch tiering decides, up-front from the TASK text, which
// router a *child* ReactAgent should carry for its whole run — so a dispatched
// sub-agent's labor turns (the bulk of search/extraction) run on the cheap
// model instead of cloning the parent's strong model for every turn.
//
// The motivating frame: a sub-agent is mostly 劳动 token (search/read/extract),
// so it should not burn the strong model on grunt work; the strong model belongs
// at the 裁决 nodes, which sit on the MAIN agent (it re-reasons over the
// child's `[子 agent 结论]`). Mis-tiering is safe: a cheap run that produces a
// weak conclusion is judged and re-dispatched by the controller.

/// Unambiguously grunt-work keywords (EN + ZH) → the child is pure labor and
/// can run the cheap model for every turn. Deliberately NARROW: ambiguous verbs
/// (summarize/test/analyze/check) are excluded so an unrecognized reasoning
/// task is never silently downgraded. A task with BOTH a grunt word and a
/// [`POWERFUL_HINTS`] word is treated as reasoning (see [`classify_dispatch`]).
const GRUNT_KEYWORDS: &[&str] = &[
    "search", "find", "grep", "glob", "list", "read", "lookup", "fetch", "extract", "enumerate",
    "collect", "gather", "搜索", "查找", "查询", "读取", "列出", "枚举", "收集", "抽取", "获取",
];

/// The model tier chosen for a sub-agent dispatch. See [`classify_dispatch`].
pub enum DispatchTier {
    /// Pure grunt task → the child runs the cheap model for ALL turns (it never
    /// needs to reason; even the opening parse is grunt work). Maximum savings,
    /// low risk because grunt tasks are mechanical.
    CheapOnly,
    /// The task may need reasoning → the child carries [`route_step`] (strong
    /// for the opening/planning turn, cheap for tool-echo turns). Still saves
    /// on the bulk labor turns while preserving the child's reasoning ability.
    Routed,
}

/// Decide a sub-agent dispatch's model tier from the task text (NOT the
/// conversation — the child starts with a fresh empty history).
///
/// Priority: reasoning beats grunt, and ambiguous defaults to [`Routed`].
/// Reuses [`POWERFUL_HINTS`] so dispatch tiering and [`route_step`]'s per-turn
/// tiering agree on what counts as "reasoning". Defaulting ambiguous tasks to
/// Routed (not CheapOnly) is deliberate: a task we don't recognize as grunt
/// keeps its reasoning capability — route_step still sends its labor turns to
/// the cheap model, so we capture most of the savings without risking a silent
/// quality regression on an unrecognized reasoning task.
pub fn classify_dispatch(task: &str) -> DispatchTier {
    let lower = task.to_lowercase();
    if POWERFUL_HINTS.iter().any(|h| lower.contains(h)) {
        return DispatchTier::Routed;
    }
    if GRUNT_KEYWORDS.iter().any(|h| lower.contains(h)) {
        return DispatchTier::CheapOnly;
    }
    DispatchTier::Routed
}

/// Per-turn router for a [`DispatchTier::CheapOnly`] child: always the cheap
/// model within the provider's tier pair, ignoring history — the task was
/// already judged grunt work by [`classify_dispatch`], so no turn deserves the
/// strong model. Self-gates exactly like [`route_step`]: a base that isn't the
/// declared strong model is returned unchanged, so attaching it is never harmful.
pub fn force_cheap_router(_history: &[Message], base_model: &str, tier: &TierCtx) -> String {
    if base_model.eq_ignore_ascii_case(&tier.strong) {
        tier.cheap.clone()
    } else {
        base_model.to_string()
    }
}

/// Resolve a dispatch to its tier, or `None` when the child's model isn't the
/// tierable strong model. Combines the tier-pair gate (only the provider's
/// declared strong model is tierable — a different model keeps the user's pick
/// and can't receive the pair's ids) with [`classify_dispatch`]. `None` ⇒ the
/// caller attaches NO router and the child runs its own model uniformly
/// (symmetric with executor.rs's wire-time tierable gate and [`route_step`]'s
/// own base guard).
pub fn dispatch_tier_for(base_model: &str, task: &str, tier: &TierCtx) -> Option<DispatchTier> {
    if !base_model.eq_ignore_ascii_case(&tier.strong) {
        return None;
    }
    Some(classify_dispatch(task))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_core::FunctionCall;

    /// The Z.AI tier pair used throughout these tests (strong=glm-4.6,
    /// cheap=glm-4-flash). Post-refactor the ids are data, not constants —
    /// this helper stands in for the TierCtx the executor builds.
    fn tier() -> TierCtx {
        TierCtx {
            strong: "glm-4.6".to_string(),
            cheap: "glm-4-flash".to_string(),
        }
    }

    fn user(text: &str) -> Message {
        Message::user(text)
    }
    fn tool_result() -> Message {
        Message {
            role: Role::User,
            content: "tool output".into(),
            tool_calls: Vec::new(),
            tool_call_id: Some("call_1".into()),
            reasoning: None,
            reasoning_signature: None,
            compact_boundary: None,
        }
    }
    fn assistant_tool_call() -> Message {
        Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![kernel_core::ToolCall {
                id: "call_1".into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "Read".into(),
                    arguments: "{}".into(),
                },
            }],
            tool_call_id: None,
            reasoning: None,
            reasoning_signature: None,
            compact_boundary: None,
        }
    }

    #[test]
    fn non_strong_base_is_returned_unchanged() {
        // The user explicitly mapped to a model outside this provider's tier
        // pair — don't swap it.
        let h = [user("ok")];
        let t = tier();
        assert_eq!(route_step(&h, "claude-opus-4", &t), "claude-opus-4");
        assert_eq!(route_step(&h, "gpt-5", &t), "gpt-5");
        // Regression (session 1ef23cbc, 2026-06-19): a DeepSeek base model must
        // NEVER be swapped to glm-4.6 — that sends a GLM model id to the DeepSeek
        // endpoint → 400 invalid_request_error. The guard here is the runtime
        // backstop; executor.rs additionally gates the router off at wire-time
        // when the provider has no tier pair. Both must hold — a power-hint turn
        // on a DeepSeek base must still return the DeepSeek id, not the strong id.
        assert_eq!(
            route_step(&[user("plan the refactor")], "deepseek-v4-flash", &t),
            "deepseek-v4-flash"
        );
        assert_eq!(route_step(&h, "deepseek-v4-flash", &t), "deepseek-v4-flash");
    }

    #[test]
    fn non_flagship_same_provider_base_is_returned_unchanged() {
        // Regression (session 7f51a5d2, 2026-06-21): a user who picked glm-5.2
        // must NOT be silently swapped to glm-4.6. The guard returns any base
        // that isn't the declared strong model unchanged, so glm-5.2 → glm-5.2
        // (the router stays out of the way instead of forcing the pair).
        let h = [user("summarize the project goals")];
        let t = tier();
        assert_eq!(route_step(&h, "glm-5.2", &t), "glm-5.2");
        // A powerful-hint turn on a glm-5.2 base still keeps glm-5.2 — the guard
        // fires BEFORE the hint scan, so a non-strong same-provider model is
        // never "upgraded" to the strong id.
        assert_eq!(route_step(&[user("plan the refactor")], "glm-5.2", &t), "glm-5.2");
    }

    #[test]
    fn planning_keyword_keeps_strong() {
        let t = tier();
        let h = [user("plan the migration to the new schema")];
        assert_eq!(route_step(&h, "glm-4.6", &t), "glm-4.6");
        // ZH hint too.
        let h2 = [user("帮我重构这个模块")];
        assert_eq!(route_step(&h2, "glm-4.6", &t), "glm-4.6");
    }

    #[test]
    fn tool_result_routes_cheap() {
        // Assistant called a tool, tool result is now the last message → the
        // next turn is the agent reacting to tool output → cheap.
        let h = [user("list files"), assistant_tool_call(), tool_result()];
        let t = tier();
        assert_eq!(route_step(&h, "glm-4.6", &t), "glm-4-flash");
    }

    #[test]
    fn short_confirmation_routes_cheap() {
        let t = tier();
        assert_eq!(route_step(&[user("ok")], "glm-4.6", &t), "glm-4-flash");
        assert_eq!(route_step(&[user("继续")], "glm-4.6", &t), "glm-4-flash");
        assert_eq!(
            route_step(&[user("yes, go ahead")], "glm-4.6", &t),
            "glm-4-flash"
        );
    }

    #[test]
    fn first_real_task_routes_strong() {
        // No powerful hint, no tool result, not a confirmation → strong (parse
        // the task properly on the opening turn).
        let h = [user("add a hello world endpoint")];
        let t = tier();
        assert_eq!(route_step(&h, "glm-4.6", &t), "glm-4.6");
    }

    #[test]
    fn long_message_is_not_treated_as_confirmation() {
        // A long message that merely starts with "ok" is not a confirmation.
        let h = [user("ok so here is the detailed plan for the whole refactor ...")];
        let t = tier();
        assert_eq!(route_step(&h, "glm-4.6", &t), "glm-4.6");
    }

    #[test]
    fn tier_pair_is_data_driven_any_ids_work() {
        // Post-refactor proof: the router works for ANY strong/cheap pair a
        // provider declares, not just Z.AI's. A hypothetical provider with
        // strong=big-model, cheap=small-model routes identically.
        let t = TierCtx {
            strong: "big-model".to_string(),
            cheap: "small-model".to_string(),
        };
        assert_eq!(route_step(&[user("ok")], "big-model", &t), "small-model");
        assert_eq!(
            route_step(&[user("plan the work")], "big-model", &t),
            "big-model"
        );
    }

    // ---- sub-agent dispatch tiering (classify_dispatch / force_cheap_router /
    // dispatch_tier_for) ----

    #[test]
    fn classify_explicit_grunt_task_is_cheap_only() {
        assert!(matches!(
            classify_dispatch("search the repo for callers of model_router"),
            DispatchTier::CheapOnly
        ));
        assert!(matches!(
            classify_dispatch("读取 src 下所有 toml 文件并列出依赖"),
            DispatchTier::CheapOnly
        ));
    }

    #[test]
    fn classify_reasoning_task_is_routed() {
        // POWERFUL_HINTS keyword → needs to think → Routed.
        assert!(matches!(
            classify_dispatch("重构 dispatch 模块,拆出独立分类器"),
            DispatchTier::Routed
        ));
        assert!(matches!(
            classify_dispatch("analyze why the sub-agent 400s on deepseek"),
            DispatchTier::Routed
        ));
    }

    #[test]
    fn classify_reasoning_beats_grunt_when_both_present() {
        // "analyze the search results" is reasoning, not grunt — the powerful
        // hint wins so the child keeps its reasoning capability.
        assert!(matches!(
            classify_dispatch("analyze the search results and refactor"),
            DispatchTier::Routed
        ));
    }

    #[test]
    fn classify_ambiguous_defaults_to_routed_not_cheap() {
        // A task with NEITHER a grunt word nor a reasoning word must NOT be
        // silently downgraded — defaulting to Routed preserves reasoning while
        // route_step still sends the labor turns cheap.
        assert!(matches!(
            classify_dispatch("把这个子任务处理一下"),
            DispatchTier::Routed
        ));
    }

    #[test]
    fn force_cheap_router_returns_cheap_for_strong_base() {
        let t = tier();
        assert_eq!(force_cheap_router(&[], "glm-4.6", &t), "glm-4-flash");
        // History is ignored — grunt work means every turn is cheap.
        assert_eq!(
            force_cheap_router(&[user("plan the refactor")], "glm-4.6", &t),
            "glm-4-flash"
        );
    }

    #[test]
    fn force_cheap_router_passes_through_non_strong_base() {
        // A base that isn't the declared strong model (user's explicit pick or
        // a foreign model) is returned unchanged — symmetric with route_step's
        // base guard, so attaching the router to a non-tierable child is a
        // harmless no-op.
        let t = tier();
        assert_eq!(force_cheap_router(&[], "glm-5.2", &t), "glm-5.2");
        assert_eq!(force_cheap_router(&[], "claude-opus-4", &t), "claude-opus-4");
        assert_eq!(
            force_cheap_router(&[], "deepseek-v4-flash", &t),
            "deepseek-v4-flash"
        );
    }

    #[test]
    fn dispatch_tier_for_non_strong_model_is_none() {
        // Non-tierable models → no router attached (the child runs uniformly).
        // Covers the foreign-endpoint-400 trap: a deepseek/claude child must
        // never receive the provider's strong id via a router.
        let t = tier();
        assert!(dispatch_tier_for("glm-5.2", "搜索文件", &t).is_none());
        assert!(dispatch_tier_for("claude-opus-4", "搜索文件", &t).is_none());
        assert!(dispatch_tier_for("deepseek-v4-flash", "analyze the bug", &t).is_none());
    }

    #[test]
    fn dispatch_tier_for_strong_model_routes_by_task_type() {
        // Declared strong model → tier by task. Grunt → CheapOnly, reasoning → Routed.
        let t = tier();
        assert!(matches!(
            dispatch_tier_for("glm-4.6", "grep for callers", &t),
            Some(DispatchTier::CheapOnly)
        ));
        assert!(matches!(
            dispatch_tier_for("glm-4.6", "重构这个模块", &t),
            Some(DispatchTier::Routed)
        ));
    }
}
