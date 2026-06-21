//! Per-step model routing (v1.2 T9). AgentFare-inspired rule-based tiering:
//! look at the conversation so far and pick a cheaper model for turns that
//! don't need the flagship, keeping glm-4.6 for planning/reasoning/final
//! answers.
//!
//! Same-provider routing only — glm-4.6 ↔ glm-4-flash on the Z.AI endpoint — so
//! endpoint/api_key stay constant and no second `ChatModel` impl is needed
//! (`GlmChatModel` already honors `opts.model` per call). This is AgentFare's
//! "rules → tier → same-provider model" path, ported 1:1; its proxy/hook layer
//! and LLM secondary router are out of scope (a ReactAgent's turn type is
//! structurally known from history, so the rules suffice — no extra LLM call).

use kernel_core::{Message, Role};

/// The strong default (flagship tool-calling GLM) and the cheap fallback within
/// the Z.AI family. glm-4-flash is far cheaper per token yet capable for
/// tool-result echoes, confirmations, and short formatting turns.
pub const STRONG_MODEL: &str = "glm-4.6";
pub const CHEAP_MODEL: &str = "glm-4-flash";

/// Planning / reasoning / review keywords (EN + ZH) → keep the strong model.
/// Mirrors AgentFare's PLANNING/REASONING/REVIEWING patterns.
const POWERFUL_HINTS: &[&str] = &[
    "plan", "design", "architect", "refactor", "debug", "analyze", "review",
    "rewrite", "migrate", "why", "investigate", "规划", "设计", "重构", "排查",
    "分析", "审查", "为什么",
];

/// Short yes/no/go confirmations → cheap model. (ZH: 继续/好的/确认/可以.)
const CHEAP_CONFIRM: &[&str] = &[
    "yes", "ok", "done", "continue", "go ahead", "sure", "next", "继续", "好的",
    "确认", "可以", "下一步", "继续吧",
];

/// Decide the model id for the next turn, given the conversation so far and the
/// agent's configured base model.
///
/// - Non-GLM base → returned unchanged (don't silently swap a user's explicit
///   claude/gpt pick; routing assumes the Z.AI family).
/// - A powerful hint in the recent window → strong (planning/reasoning needs the
///   flagship).
/// - Otherwise, if the last message is a tool result (agent branching on tool
///   output) or the current instruction is a short confirmation → cheap.
/// - Default (incl. the first turn parsing a fresh task) → strong.
pub fn route_step(history: &[Message], base_model: &str) -> String {
    // Only route within the GLM family.
    if !base_model.eq_ignore_ascii_case(STRONG_MODEL) {
        return base_model.to_string();
    }
    let recent = recent_text(history);
    if POWERFUL_HINTS.iter().any(|h| recent.contains(h)) {
        return STRONG_MODEL.to_string();
    }
    if last_is_tool_result(history) {
        return CHEAP_MODEL.to_string();
    }
    if is_short_confirmation(history) {
        return CHEAP_MODEL.to_string();
    }
    STRONG_MODEL.to_string()
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

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_core::FunctionCall;

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
        }
    }

    #[test]
    fn non_glm_base_is_returned_unchanged() {
        // The user explicitly mapped to a non-GLM model — don't swap it.
        let h = [user("ok")];
        assert_eq!(route_step(&h, "claude-opus-4"), "claude-opus-4");
        assert_eq!(route_step(&h, "gpt-5"), "gpt-5");
        // Regression (session 1ef23cbc, 2026-06-19): a DeepSeek base model must
        // NEVER be swapped to glm-4.6 — that sends a GLM model id to the DeepSeek
        // endpoint → 400 invalid_request_error. The guard here is the runtime
        // backstop; executor.rs additionally gates the router off at wire-time for
        // non-GLM providers (only resolved_model.starts_with("glm-") wires it).
        // Both must hold — a power-hint turn on a DeepSeek base must still return
        // the DeepSeek id, not STRONG_MODEL.
        assert_eq!(route_step(&[user("plan the refactor")], "deepseek-v4-flash"), "deepseek-v4-flash");
        assert_eq!(route_step(&h, "deepseek-v4-flash"), "deepseek-v4-flash");
    }

    #[test]
    fn glm_5_2_base_is_returned_unchanged() {
        // Regression (session 7f51a5d2, 2026-06-21): a user who picked glm-5.2
        // must NOT be silently swapped to glm-4.6. The guard returns any non-
        // STRONG base unchanged, so glm-5.2 → glm-5.2 (the router stays out of
        // the way instead of forcing the glm-4.6↔flash family). On a first turn
        // (no hint/tool_result/confirm) this is the exact shape of the failing
        // session.
        let h = [user("summarize the project goals")];
        assert_eq!(route_step(&h, "glm-5.2"), "glm-5.2");
        // A powerful-hint turn on a glm-5.2 base still keeps glm-5.2 — the guard
        // fires BEFORE the hint scan, so a non-flagship GLM is never "upgraded"
        // to STRONG_MODEL.
        assert_eq!(route_step(&[user("plan the refactor")], "glm-5.2"), "glm-5.2");
    }

    #[test]
    fn planning_keyword_keeps_strong() {
        let h = [user("plan the migration to the new schema")];
        assert_eq!(route_step(&h, STRONG_MODEL), STRONG_MODEL);
        // ZH hint too.
        let h2 = [user("帮我重构这个模块")];
        assert_eq!(route_step(&h2, STRONG_MODEL), STRONG_MODEL);
    }

    #[test]
    fn tool_result_routes_cheap() {
        // Assistant called a tool, tool result is now the last message → the
        // next turn is the agent reacting to tool output → cheap.
        let h = [user("list files"), assistant_tool_call(), tool_result()];
        assert_eq!(route_step(&h, STRONG_MODEL), CHEAP_MODEL);
    }

    #[test]
    fn short_confirmation_routes_cheap() {
        assert_eq!(route_step(&[user("ok")], STRONG_MODEL), CHEAP_MODEL);
        assert_eq!(route_step(&[user("继续")], STRONG_MODEL), CHEAP_MODEL);
        assert_eq!(route_step(&[user("yes, go ahead")], STRONG_MODEL), CHEAP_MODEL);
    }

    #[test]
    fn first_real_task_routes_strong() {
        // No powerful hint, no tool result, not a confirmation → strong (parse
        // the task properly on the opening turn).
        let h = [user("add a hello world endpoint")];
        assert_eq!(route_step(&h, STRONG_MODEL), STRONG_MODEL);
    }

    #[test]
    fn long_message_is_not_treated_as_confirmation() {
        // A long message that merely starts with "ok" is not a confirmation.
        let h = [user("ok so here is the detailed plan for the whole refactor ...")];
        assert_eq!(route_step(&h, STRONG_MODEL), STRONG_MODEL);
    }
}
