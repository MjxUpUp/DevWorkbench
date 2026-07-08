//! ReactAgent → chat wire-event mapping.
//!
//! The transparent ReactAgent (kernel layer) emits kernel-core `AgentEvent`s.
//! The chat UI consumes `ChatStreamEvent`s — the same wire schema claude's
//! stream-json parser produces (see `pty.rs`). This module is the bridge: a pure
//! `map_agent_event` turning one `AgentEvent` into zero or more
//! `ChatStreamEvent`s, so the ReactAgent reuses the EXACT BlocksView rendering
//! path claude uses. No second presentation layer, no terminal serialization.
//!
//! Design note (plan D4): kernel-core's `AgentEvent` deliberately has NO serde
//! derive — it's a domain model that must evolve independently of the wire
//! schema. So we map by hand here instead of coupling the two with a derive on
//! the enum. `ChatStreamEvent` is the UI schema; this fn is the only thing that
//! knows both sides.

use crate::agents::pty::ChatStreamEvent;
use crate::kernel_impl::context_compact::summary_with_fence;
use crate::models::{Session, SessionStatus};
use kernel_core::{
    AgentEvent, AgentRunStatus, CompactBoundaryMeta, FunctionCall, Message, Role, ToolCall,
    ToolCallEvent, ToolCallStatus,
};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};

// ---------------------------------------------------------------------------
// Defect ③: Pairing invariant check — orphans detected, not silently stripped.
// ---------------------------------------------------------------------------

/// Describes a tool-call pair violation found during validation.
#[derive(Clone, Debug, PartialEq)]
pub enum PairingViolation {
    /// A ToolUse exists but its ToolResult never arrived (interrupt/crash).
    OrphanToolCall { id: Option<String>, name: String },
    /// A ToolResult has no matching ToolUse in the block list.
    DanglingToolResult { id: Option<String> },
}

impl PairingViolation {
    pub(crate) fn detail(&self) -> String {
        match self {
            Self::OrphanToolCall { id, name } => format!(
                "Orphaned ToolUse(id={:?}, name=\"{}\"): no matching ToolResult arrived (stream cut short)",
                id, name
            ),
            Self::DanglingToolResult { id } => format!(
                "Dangling ToolResult(id={:?}): no matching ToolUse in blocks",
                id
            ),
        }
    }
}

/// Validate pairing integrity of a flat slice of `ChatStreamEvent` blocks.
/// Returns any violations found (empty = all pairs balanced).
///
/// Two violation kinds:
/// - OrphanToolCall: a ToolUse whose ToolResult never arrived (stream cut short)
/// - DanglingToolResult: a ToolResult with no matching ToolUse before it in the sequence
pub(crate) fn validate_block_pairs(blocks: &[ChatStreamEvent]) -> Vec<PairingViolation> {
    // Track id → index mapping for fast lookup.
    let mut tool_use_by_id: std::collections::HashMap<&str, (usize, Option<String>, String)> =
        std::collections::HashMap::new();
    let mut fifo_stack: Vec<(usize, String)> = Vec::new();
    // ids of results that arrived without a prior ToolUse declaration.
    let mut dangling_result_ids: Vec<Option<String>> = Vec::new();

    for (i, ev) in blocks.iter().enumerate() {
        match ev {
            ChatStreamEvent::ToolUse { id, name, .. } => {
                if let Some(tid) = id {
                    tool_use_by_id.insert(tid.as_str(), (i, Some(tid.clone()), name.clone()));
                } else {
                    fifo_stack.push((i, name.clone()));
                }
            }
            ChatStreamEvent::ToolResult {
                tool_use_id,
                is_error,
                ..
            } => {
                // Skip error results (is_error=true means stream was cut at this result)
                if *is_error {
                    continue;
                }
                match tool_use_id {
                    Some(tid) => {
                        // Was there a prior ToolUse with this id? Check only blocks before current position.
                        let has_prior_decl = blocks[..i].iter().any(|b| matches!(b, ChatStreamEvent::ToolUse { id: Some(oid), .. } if oid.as_str() == tid));
                        if has_prior_decl {
                            tool_use_by_id.remove(tid.as_str());
                        } else {
                            // This result arrived before its ToolUse declaration.
                            dangling_result_ids.push(Some(tid.clone()));
                        }
                    }
                    None => {
                        // FIFO: pop the oldest id-less ToolUse.
                        if fifo_stack.remove(0).1.is_empty() {
                            /* consumed */
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let mut violations: Vec<PairingViolation> = Vec::new();
    // Remaining in tool_use_by_id → orphaned ToolCalls (no result arrived).
    for (_id, (_, uid, name)) in tool_use_by_id {
        violations.push(PairingViolation::OrphanToolCall { id: uid, name });
    }
    // FIFO stack remaining → also orphans.
    for (_idx, name) in fifo_stack {
        violations.push(PairingViolation::OrphanToolCall { id: None, name });
    }
    // Results that arrived without prior ToolUse declaration → dangling.
    for rid in &dangling_result_ids {
        violations.push(PairingViolation::DanglingToolResult { id: rid.clone() });
    }

    violations
}

/// Drain pending_tools and return descriptions of any orphaned entries.
/// Call at end-of-stream (after the last ChatStreamEvent) to catch cuts-short pairs.
pub(crate) fn drain_pending_orphans(
    pending_tools: &mut VecDeque<(Option<String>, String, String)>,
) -> Vec<PairingViolation> {
    let mut orphans = Vec::new();
    while let Some((id, name, _args)) = pending_tools.pop_front() {
        orphans.push(PairingViolation::OrphanToolCall { id, name });
    }
    orphans
}

/// Map one kernel-core `AgentEvent` to zero or more chat wire events for the
/// `agent:event` channel. Pure + testable: the caller passes `secs` (elapsed
/// since the run started) so the Result block's duration is deterministic under
/// test — this fn has no time side-effect of its own.
///
/// Mapping:
/// - `Token(s)`           → `[Text{content: s}]`
/// - `ToolCall` Started   → `[ToolUse{name, input: parse(arguments)}]`
/// - `ToolCall` Succeeded → `[ToolResult{content: "(ok)",   is_error: false}]`
/// - `ToolCall` Failed    → `[ToolResult{content: "(failed)", is_error: true}]`
/// - `FileChanged(p)`     → `[FileChanged{path: p}]` (per-write mutation line)
/// - `TurnBoundary`       → `[]` (same)
/// - `Done(outcome)`      → `[Result{is_error: status != Completed, secs}]`
///
/// NB: the transparent ReactAgent now fills `ToolCallEvent.result` with the real
/// tool output (see `react_agent::run`), so Succeeded/Failed map to the actual
/// content. The `"(ok)"/"(failed)"` fallback only applies when an emitter
/// reports status without a result (e.g. some opaque-agent reverse-mapping paths).
pub fn map_agent_event(ev: AgentEvent, secs: u64) -> Vec<ChatStreamEvent> {
    match ev {
        AgentEvent::Token(s) => vec![ChatStreamEvent::Text { content: s }],
        AgentEvent::Reasoning(s) => vec![ChatStreamEvent::Thinking { content: s }],
        AgentEvent::ToolCall(tc) => match tc.status {
            ToolCallStatus::Started => vec![ChatStreamEvent::ToolUse {
                // id round-trips end-to-end on BOTH paths: OpaqueAgent (claude
                // wire `id` / gemini `tool_id`, preserved via pty to_event →
                // chat_event_to_agent_events → here) AND ReactKernel (ToolCall.id
                // → ToolCallEvent.id, forwarded by react_agent). None only for
                // legacy/pre-id wire or ACP/test construction. Carrying it here
                // keeps persisted (DB) blocks pairable on replay by id instead
                // of degrading to FIFO (defect ①).
                id: tc.id.clone(),
                name: tc.tool,
                input: parse_tool_arguments(&tc.arguments),
            }],
            ToolCallStatus::Succeeded => vec![ChatStreamEvent::ToolResult {
                tool_use_id: tc.id.clone(),
                content: tc.result.unwrap_or_else(|| "(ok)".to_string()),
                is_error: false,
            }],
            ToolCallStatus::Failed => vec![ChatStreamEvent::ToolResult {
                tool_use_id: tc.id.clone(),
                content: tc.result.unwrap_or_else(|| "(failed)".to_string()),
                is_error: true,
            }],
        },
        AgentEvent::FileChanged(p) => vec![ChatStreamEvent::FileChanged {
            path: p.display().to_string(),
        }],
        AgentEvent::TurnBoundary => Vec::new(),
        AgentEvent::Done(outcome) => {
            vec![ChatStreamEvent::Result {
                is_error: outcome.status != AgentRunStatus::Completed,
                secs,
            }]
        }
    }
}

/// Parse a tool's raw arguments string into a JSON value for the ToolUse card.
/// The transparent agent always emits valid JSON (LLM tool-call arguments); if
/// it's ever empty or malformed, fall back to `null` rather than panicking the
/// stream — the card renders `null` harmlessly.
fn parse_tool_arguments(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Value::Null;
    }
    serde_json::from_str(trimmed).unwrap_or(Value::Null)
}

/// Reverse of `map_agent_event`: turn a claude `agent:event` wire block back
/// into kernel-core `AgentEvent`s for the OpaqueAgent stream. Unlike the
/// forward map (ReactAgent Succeeded/Failed → placeholder "(ok)"), claude's
/// ToolResult.content is the REAL tool output — but `ToolCallEvent` has no
/// content field (kernel-core/agent.rs:81), so the reverse map restores only
/// name/arguments via positional pairing. The workflow path's downstream
/// `map_agent_to_chunks` then re-emits the ReactAgent "(ok)"/"(failed)"
/// placeholder for the tool_result card (inherited behavior, same as a
/// transparent agent's tool call).
///
/// `pending_tools`: queue of `(id, name, arguments_json)` — enqueued on
/// ToolUse, dequeued on ToolResult. Pairing is **id-first, FIFO-fallback**:
///
/// - When the ToolResult carries `tool_use_id` (OpaqueAgent path — claude's
///   stream-json always emits it, now preserved end-to-end via `to_event`),
///   dequeue the pending entry whose id matches. This is order-independent:
///   batched `use(A), use(B), result(B), result(A)` now pairs correctly, not
///   just the same-order case the old FIFO hack handled.
/// - When `tool_use_id` is absent (ReactKernel forward replays a wire that
///   never carried an id, or legacy pre-id session blocks), fall back to FIFO
///   `pop_front` — preserving the old positional behavior as a safety net.
///
/// This closes defect ①'s root cause: the wire schema no longer drops the
/// pairing key, so the reverse map stops guessing.
///
/// Mapping:
/// - `Text{content}`                 → `[Token(content)]`
/// - `ToolUse{id, name, input}`      → enqueue; `[ToolCall(Started)]`
/// - `ToolResult{tool_use_id, content, is_error}` → id-match (or FIFO) paired
///   `[ToolCall(Succeeded|Failed)]`; orphan (no match) `[Token(content)]`
///   (demote — never drop the signal)
/// - `Result{..}`                    → `[]` (Done owned by agent:completed;
///   emitting here would duplicate the terminal event and double-end the stream)
pub fn chat_event_to_agent_events(
    ev: &ChatStreamEvent,
    pending_tools: &mut VecDeque<(Option<String>, String, String)>,
) -> Vec<AgentEvent> {
    match ev {
        ChatStreamEvent::Text { content } => vec![AgentEvent::Token(content.clone())],
        ChatStreamEvent::Thinking { content } => vec![AgentEvent::Reasoning(content.clone())],
        ChatStreamEvent::ToolUse { id, name, input } => {
            let args = serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
            pending_tools.push_back((id.clone(), name.clone(), args.clone()));
            vec![AgentEvent::ToolCall(ToolCallEvent {
                tool: name.clone(),
                arguments: args,
                status: ToolCallStatus::Started,
                // Carry the wire id through so map_agent_event can re-emit it on
                // the persisted ChatStreamEvent (DB replay pairs by id, not FIFO).
                id: id.clone(),
                result: None,
            })]
        }
        ChatStreamEvent::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            // Id-first: locate the pending ToolUse whose id matches tool_use_id.
            // FIFO fallback when the wire carries no id (legacy/forward replay).
            let paired = match tool_use_id {
                Some(tid) => pending_tools
                    .iter()
                    .position(|(pid, _, _)| pid.as_deref() == Some(tid.as_str()))
                    .and_then(|i| pending_tools.remove(i)),
                None => pending_tools.pop_front(),
            };
            match paired {
                // Paired: restore name/arguments/id from the matched ToolUse.
                Some((id, name, args)) => {
                    let status = if *is_error {
                        ToolCallStatus::Failed
                    } else {
                        ToolCallStatus::Succeeded
                    };
                    vec![AgentEvent::ToolCall(ToolCallEvent {
                        tool: name,
                        arguments: args,
                        status,
                        // The matched ToolUse's id — carried through so the
                        // forward map re-emits tool_use_id on the persisted block.
                        id,
                        // claude's ToolResult.content IS the real tool output —
                        // carry it through so downstream renders the actual result.
                        result: Some(content.clone()),
                    })]
                }
                // Orphan (no pending ToolUse matches): demote content to a Token
                // so it surfaces as text rather than vanishing. Do NOT fabricate
                // a ToolCall(Started) — would desync downstream use/result counts.
                None => {
                    log::warn!(
                        "[chat_event_to_agent_events] orphan ToolResult (tool_use_id={:?}); content demoted to text: {}",
                        tool_use_id,
                        content.chars().take(80).collect::<String>()
                    );
                    vec![AgentEvent::Token(content.clone())]
                }
            }
        }
        // Done is owned by agent:completed; emitting here would duplicate the
        // terminal event and double-end the stream.
        ChatStreamEvent::Result { .. } => Vec::new(),
        // FileChanged never arrives on the chat wire from an opaque CLI (CLIs
        // don't surface per-write events); it's emitted only by the transparent
        // ReactAgent forward path. The reverse map exists for the OpaqueAgent
        // stream, so this arm keeps the match exhaustive as a no-op rather than
        // fabricating a kernel event.
        ChatStreamEvent::FileChanged { .. } => Vec::new(),
        // Compact is a meta-event emitted by the compaction sink, never by an
        // opaque CLI — kept exhaustive. It carries no kernel AgentEvent (it
        // never enters the model's stream, only the UI's block list).
        ChatStreamEvent::Compact { .. } => Vec::new(),
        // §4.2 缺项3: CompactBoundary is the boundary-marker meta-event emitted
        // alongside Compact. Never an AgentEvent — same bypass. It's
        // reconstructed into a boundary Message by blocks_to_history (below),
        // not mapped to a kernel event here.
        ChatStreamEvent::CompactBoundary { .. } => Vec::new(),
        // Human-Gate control signal — UI-only (opens a modal); never an
        // AgentEvent, never model history. Same bypass as Compact.
        ChatStreamEvent::ApprovalRequired { .. } => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Multi-turn history — turn prior Sessions back into kernel-core `Message`s so
// the ReactAgent resumes a conversation with real context. Symmetric to the
// CLI path's `inject_conversation_context` (pty.rs), but structured: blocks
// (text/tool_use/tool_result) round-trip into assistant.tool_calls + tool
// messages instead of a flattened output_summary string. Without this the
// self-built agent sees each turn in isolation — the last structural gap vs
// the CLI agents.
// ---------------------------------------------------------------------------

/// Per-turn char budget for one prior turn's assistant text. Large enough for a
/// full reply, small enough that a runaway turn doesn't eat the whole history.
pub const REACT_HISTORY_TURN_TEXT_CAP: usize = 2000;
/// Total chars across ALL prior turns. Mirrors the CLI path's 8000 overall cap
/// but lifted a bit — structured tool turns carry more useful signal per char
/// than a flat summary does.
pub const REACT_HISTORY_TOTAL_TEXT_CAP: usize = 12000;
/// Hard cap on prior-turn messages before we start dropping whole turns.
pub const REACT_HISTORY_TOTAL_MESSAGES: usize = 40;

// ---------------------------------------------------------------------------
// Defect ①/③: blocks_to_history — structured reconstruction from ChatStreamEvent
// blocks that preserves tool_use/tool_result pairs instead of stripping them.
// ---------------------------------------------------------------------------

/// Convert a session's persisted `[ChatStreamEvent]` blocks into a sequence of
/// kernel-core `Message`s that reconstructs the turn's STRUCTURE:
///   user(prompt) + assistant(text+reasoning, with tool_calls) + [tool(result)]…
///
/// Emits proper `role::Tool` messages so the model sees exactly what happened
/// in prior turns — same format as the live stream. (The legacy approach
/// stripped all tool events to prose, which was defect ①'s root cause: each
/// turn seen in isolation by a fresh agent.)
///
/// Meta-events (`Result`, `FileChanged`, `Compact`, `ApprovalRequired`) are
/// dropped (they never entered the model's original history).
pub(crate) fn blocks_to_history(
    blocks: &[ChatStreamEvent],
    turn_text_cap: usize,
) -> Vec<Message> {
    if blocks.is_empty() {
        return Vec::new();
    }

    let mut assistant_text = String::new();
    let mut assistant_reasoning = String::new();

    // §4.2 缺项3 + resume Compact summary 重建: turn 末尾的 compaction meta
    // events 按 blocks 出现顺序重建为 [summary(User+fence), boundary(System)] ——
    // 与 live maybe_compact 的 history 顺序一致(summarize_middle: summary@start,
    // 然后 maybe_compact insert boundary@start+1)。仅 Summarize path
    // (is_error=false + 配对 boundary)的 summary 进 history;HardTruncate/
    // BreakerTripped(is_error)是错误/暂停提示、MicroClear(无 boundary)是 UI
    // 描述,都不进模型历史。
    let mut compaction_tail: Vec<Message> = Vec::new();
    let mut pending_summary: Option<String> = None;

    // Ordered record of every ToolUse in arrival order. Tuple:
    //   (synth_id, name, input, matched_result)
    // `synth_id` is the real id when the stream carried one, else a synthesized
    // `__fifo_N__` so that (a) multiple id-less calls don't collide on one key
    // (F3) and (b) provider-facing tool_call ids are never empty (F5).
    let mut tool_uses: Vec<(String, String, serde_json::Value, Option<String>)> = Vec::new();
    // id-first lookup: real id → index into tool_uses.
    let mut id_to_index: HashMap<String, usize> = HashMap::new();
    // FIFO queue: indices of id-less ToolUses still awaiting a result, oldest first.
    let mut pending_fifo: Vec<usize> = Vec::new();
    let mut fifo_counter: usize = 0;

    for ev in blocks {
        match ev {
            ChatStreamEvent::Text { content } => assistant_text.push_str(content),
            ChatStreamEvent::Thinking { content } => assistant_reasoning.push_str(content),
            ChatStreamEvent::ToolUse { id, name, input } => {
                let idx = tool_uses.len();
                match id {
                    Some(real) => {
                        tool_uses.push((real.clone(), name.clone(), input.clone(), None));
                        id_to_index.insert(real.clone(), idx);
                    }
                    None => {
                        let synth = format!("__fifo_{fifo_counter}__");
                        fifo_counter += 1;
                        tool_uses.push((synth, name.clone(), input.clone(), None));
                        pending_fifo.push(idx);
                    }
                }
            }
            ChatStreamEvent::ToolResult { tool_use_id, content, is_error: _ } => {
                // id-first: exact id match. FIFO: oldest pending id-less ToolUse.
                let matched_idx = match tool_use_id {
                    Some(tid) => id_to_index.get(tid).copied(),
                    None => pending_fifo.first().copied(),
                };
                if let Some(idx) = matched_idx {
                    tool_uses[idx].3 = Some(content.clone());
                    if tool_use_id.is_none() {
                        // FIFO-consumed: drop from pending queue.
                        if let Some(pos) = pending_fifo.iter().position(|&i| i == idx) {
                            pending_fifo.remove(pos);
                        }
                    }
                }
                // Dangling (no matching ToolUse): drop — validate_block_pairs
                // upstream (session.rs) already warns on the same blocks.
            }
            // §4.2 缺项3 + resume summary 重建: CompactBoundary — pair with a
            // pending Summarize summary (if any) so the rebuilt tail is
            // [summary(User+fence), boundary(System)], matching live
            // maybe_compact's history order (summary@start, boundary@start+1).
            ChatStreamEvent::CompactBoundary {
                trigger,
                pre_tokens,
                preserved_count,
            } => {
                if let Some(s) = pending_summary.take() {
                    compaction_tail.push(summary_with_fence(&s));
                }
                compaction_tail.push(boundary_message(CompactBoundaryMeta {
                    trigger: trigger.clone(),
                    pre_tokens: *pre_tokens,
                    preserved_count: *preserved_count,
                }));
            }
            // resume Compact summary 重建: a non-error Compact carries the
            // Summarize path's summary text — stage it; the following
            // CompactBoundary (same ArchivedChunk) pairs with it to emit
            // [summary, boundary]. is_error=true (HardTruncate/BreakerTripped)
            // is a compaction-failure/pause notice, NOT history — dropped.
            ChatStreamEvent::Compact { summary, is_error, .. } => {
                if !is_error {
                    pending_summary = Some(summary.clone());
                }
            }
            // Meta-events never entered the model's original history.
            ChatStreamEvent::Result { .. }
            | ChatStreamEvent::FileChanged { .. }
            | ChatStreamEvent::ApprovalRequired { .. } => {}
        }
    }

    // F1 fix: assistant.tool_calls carries EVERY ToolUse (matched + orphan),
    // in arrival order. Previously matched ToolUses were dropped from tool_calls,
    // so tool_result messages referenced ids no assistant.tool_calls held →
    // provider 400 ("tool_use ids found without tool_result" / "tool_call_id
    // does not match any tool_call").
    let tool_calls: Vec<ToolCall> = tool_uses
        .iter()
        .map(|(synth_id, name, input, _)| ToolCall {
            id: synth_id.clone(),
            call_type: "function".into(),
            function: FunctionCall {
                name: name.clone(),
                arguments: serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string()),
            },
        })
        .collect();

    // F7 fix: content comes ONLY from assistant text — never fall back to
    // reasoning. Reasoning already rides in its own field; duplicating it as
    // text makes the provider emit identical thinking + text blocks.
    let content = if !assistant_text.is_empty() {
        tail(&assistant_text, turn_text_cap)
    } else {
        String::new()
    };

    let assistant_msg = Message {
        role: Role::Assistant,
        content,
        tool_calls,
        tool_call_id: None,
        reasoning: if !assistant_reasoning.is_empty() {
            Some(tail(&assistant_reasoning, turn_text_cap))
        } else {
            None
        },
        reasoning_signature: None,
        compact_boundary: None,
    };

    let mut messages: Vec<Message> = Vec::new();
    messages.push(assistant_msg);

    // Tool result messages: one per matched ToolUse, in ToolUse arrival order
    // (NOT result arrival order) — F4 fix: ordered iteration, so the
    // assistant.tool_calls[i] ↔ tool(tool_call_id) correspondence is stable
    // and deterministic (no HashMap randomization).
    for (synth_id, _, _, result) in &tool_uses {
        if let Some(result_content) = result {
            messages.push(Message {
                role: Role::Tool,
                content: tail(result_content, turn_text_cap),
                tool_calls: Vec::new(),
                tool_call_id: Some(synth_id.clone()),
                reasoning: None,
                reasoning_signature: None,
                compact_boundary: None,
            });
        }
    }

    // Turn produced zero reconstructable content (no text, no reasoning, no
    // tool calls — blocks were entirely meta-events) → return empty so the
    // caller falls back to output_summary. (F6: the old "drop empty assistant"
    // branch is subsumed — with tool_calls always populated for any ToolUse,
    // an all-tool turn keeps its assistant message rather than leaving a
    // dangling tool_result.)
    let a = &messages[0];
    let vacuous = a.content.is_empty() && a.reasoning.is_none() && a.tool_calls.is_empty();
    // §4.2 缺项3 exception + resume summary: a vacuous turn that STILL carries
    // compaction meta (Compact+CompactBoundary from a Summarize, or a lone
    // boundary) must not collapse to empty — the summary/boundary would vanish
    // and the next resume would re-compact already-summarized history AND lose
    // the prior summary. Return the compaction tail alone (callers tolerate a
    // meta-only turn). The common case (no compaction) returns Vec::new().
    if vacuous {
        return compaction_tail;
    }
    messages.extend(compaction_tail);

    messages
}

/// §4.2 缺项3 / CCB `createCompactBoundaryMessage` parity: rebuild a
/// compact-boundary marker Message from a `ChatStreamEvent::CompactBoundary`'s
/// metadata. System role + the meta. It never reaches the model:
/// anthropic_chat_model filters ALL System messages, and openai_chat_model
/// filters messages whose `compact_boundary` is set (keeping genuine system
/// prompts). It exists so maybe_compact's `last_boundary_index` can find where
/// the LAST compaction happened and summarize only what came after it, avoiding
/// the double-compaction drift (see context_compact::last_boundary_index for
/// the same-run vs resume distinction).
fn boundary_message(meta: CompactBoundaryMeta) -> Message {
    Message {
        role: Role::System,
        content: String::new(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        reasoning: None,
        reasoning_signature: None,
        compact_boundary: Some(meta),
    }
}

/// Keep the tail of `s` up to `max` chars, snapped to a UTF-8 boundary and
/// `...`-prefixed. Mirrors pty.rs's private `tail` — duplicated here to avoid
/// widening the CLI module's visibility just for one helper.
fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    format!("...{}", &s[start..])
}

/// Rebuild ONE turn's message group from its persisted blocks (or
/// output_summary fallback). Shared central rebuilder — used by both agent
/// paths, which is the defect ④ unification point:
///   • [`turns_to_history`] — ReactKernel path → structured `Message[]` for the
///     API;
///   • `pty::inject_conversation_context` — OpaqueAgent path → rendered to text
///     for the CLI subprocess.
/// Both now rebuild from the same `blocks_to_history` core, so OpaqueAgent
/// inherits tool-call context too instead of only the output_summary text tail.
///
/// Returns:
///   • empty `Vec` for a Running turn (no finalized reply) — callers filter it;
///   • `[user(prompt), assistant(text+reasoning, tool_calls), tool(result)..]`
///     when persisted blocks reconstruct successfully;
///   • `[user(prompt), assistant(output_summary)]` when blocks are absent/empty
///     but the turn has a finalized summary (legacy / OpaqueAgent turns, or
///     all-meta blocks) — also fixes a prior hidden bug where the `None` arm
///     returned only `[user]` and dropped the summary;
///   • `[user(prompt)]` when there's nothing else to show.
pub(crate) fn rebuild_turn_messages(sess: &Session, turn_text_cap: usize) -> Vec<Message> {
    // Skip turns that haven't finalized — they have no assistant reply yet, and
    // emitting a lone user message would hand the model an unanswered question.
    if sess.status == SessionStatus::Running {
        return Vec::new();
    }
    // F2 fix: the user prompt must lead every turn's group. blocks_to_history
    // returns only [assistant, tool..]; we prepend the user prompt so roles
    // alternate correctly.
    let reconstructed: Option<Vec<Message>> = sess
        .blocks
        .as_ref()
        .filter(|v| !v.is_null())
        .and_then(|v| serde_json::from_value::<Vec<ChatStreamEvent>>(v.clone()).ok())
        .map(|blocks| blocks_to_history(&blocks, turn_text_cap));

    match reconstructed {
        Some(messages) if !messages.is_empty() => {
            // user prompt + reconstructed [assistant, tool..]
            let mut g = Vec::with_capacity(messages.len() + 1);
            g.push(Message::user(sess.prompt.clone()));
            g.extend(messages);
            g
        }
        // No usable blocks — None (never persisted), parse failure, or all-meta
        // events. Fall back to output_summary (the finalized reply) when present;
        // otherwise the lone user prompt. The `None` arm previously returned only
        // [user] and dropped the summary — unified here so legacy / OpaqueAgent
        // turns inherit their output text (matches blocks_none_falls_back...).
        _ => sess
            .output_summary
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| {
                vec![
                    Message::user(sess.prompt.clone()),
                    Message::assistant(tail(s, turn_text_cap)),
                ]
            })
            .unwrap_or_else(|| vec![Message::user(sess.prompt.clone())]),
    }
}

/// Convert prior conversation turns (ASC by started_at, as returned by
/// `pty::load_prior_turns`) into kernel-core `Message`s for the ReactAgent's
/// `run()` history. Each completed/failed turn expands to:
///   user(prompt) + assistant(text+reasoning, with tool_calls) + [tool(result)..]  (from blocks)
///   user(prompt) + assistant(output_summary)                                    (legacy fallback)
///   user(prompt)                                                                (no output at all)
/// Running turns are skipped (no finalized content yet). When the result exceeds
/// the message or total-char caps, the OLDEST whole turns are dropped — turns
/// are never split mid-way, so a prompt and its assistant reply always travel together.
pub fn turns_to_history(
    turns: &[Session],
    turn_text_cap: usize,
    total_text_cap: usize,
) -> Vec<Message> {
    // Build per-turn message groups, oldest-first. Each group is a whole turn:
    // user + assistant (+ its tool messages). Caps operate on whole groups.
    // Each turn → its message group (user + assistant [+ tools]). Running turns
    // yield an empty group (filtered out). Defect ④: rebuild_turn_messages is
    // shared with pty::inject_conversation_context so both agent paths rebuild
    // history identically from the same central blocks_to_history.
    let groups: Vec<Vec<Message>> = turns
        .iter()
        .map(|sess| rebuild_turn_messages(sess, turn_text_cap))
        .filter(|g| !g.is_empty())
        .collect();

    // Greedily keep newest turns until we breach a cap; then stop adding older.
    // This preserves the most recent context (most relevant for a follow-up)
    // and drops oldest whole turns.
    let mut kept: Vec<&Vec<Message>> = Vec::new();
    let mut msg_count = 0usize;
    let mut char_count = 0usize;
    for group in groups.iter().rev() {
        let group_chars: usize = group.iter().map(|m| m.content.len()).sum();
        let would_msgs = msg_count + group.len();
        let would_chars = char_count + group_chars;
        // Stop before adding a turn that would breach EITHER cap — unless we
        // have nothing yet (always keep at least the most recent turn).
        if !kept.is_empty()
            && (would_msgs > REACT_HISTORY_TOTAL_MESSAGES || would_chars > total_text_cap)
        {
            break;
        }
        msg_count = would_msgs;
        char_count = would_chars;
        kept.push(group);
    }
    // kept is newest-first; reverse back to chronological for the history.
    let mut out: Vec<Message> = Vec::new();
    for group in kept.into_iter().rev() {
        out.extend(group.iter().cloned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_core::{AgentOutcome, AgentRunStatus, ToolCallEvent};
    use std::path::PathBuf;

    #[test]
    fn token_maps_to_text_block() {
        let out = map_agent_event(AgentEvent::Token("hello".to_string()), 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::Text { content } => assert_eq!(content, "hello"),
            other => panic!("expected Text, got {:?}", other),
        }
    }

    #[test]
    fn reasoning_maps_to_thinking_block() {
        // GLM Interleaved Thinking surfaces as AgentEvent::Reasoning; the chat
        // layer maps it onto the Thinking wire block (collapsible UI), NOT Text.
        let out = map_agent_event(AgentEvent::Reasoning("why".to_string()), 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::Thinking { content } => assert_eq!(content, "why"),
            other => panic!("expected Thinking, got {:?}", other),
        }
    }

    #[test]
    fn thinking_wire_block_round_trips_to_reasoning_event() {
        // Reverse map (opaque → kernel): a Thinking wire block becomes a
        // Reasoning AgentEvent, independent of the tool-use pairing queue.
        let mut pending = std::collections::VecDeque::new();
        let evs = chat_event_to_agent_events(
            &ChatStreamEvent::Thinking { content: "deliberation".into() },
            &mut pending,
        );
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            AgentEvent::Reasoning(s) => assert_eq!(s, "deliberation"),
            other => panic!("expected Reasoning, got {:?}", other),
        }
        assert!(pending.is_empty(), "thinking must not enqueue a tool pairing");
    }

    #[test]
    fn tool_call_started_maps_to_tool_use_with_parsed_input() {
        let ev = AgentEvent::ToolCall(ToolCallEvent {
            tool: "Read".to_string(),
            arguments: r#"{"file_path":"/a.txt"}"#.to_string(),
            status: ToolCallStatus::Started,
            id: None,
            result: None,
        });
        let out = map_agent_event(ev, 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::ToolUse { name, input, .. } => {
                assert_eq!(name, "Read");
                assert_eq!(input["file_path"], "/a.txt");
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }
    }

    #[test]
    fn tool_call_started_carries_id_to_tool_use() {
        // The pairing key round-trips through the forward map: a Started event
        // carrying an id (OpaqueAgent reverse-map path) must surface as the
        // ToolUse wire block's id so persisted (DB) blocks stay pairable by id
        // on replay instead of degrading to FIFO (defect ①). The id=None case
        // (legacy/pre-id wire) still maps to None — covered by the test above.
        let ev = AgentEvent::ToolCall(ToolCallEvent {
            tool: "Read".to_string(),
            arguments: "{}".to_string(),
            status: ToolCallStatus::Started,
            id: Some("toolu_abc".to_string()),
            result: None,
        });
        let out = map_agent_event(ev, 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::ToolUse { id, .. } => {
                assert_eq!(id.as_deref(), Some("toolu_abc"));
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }
    }

    #[test]
    fn tool_call_succeeded_maps_to_ok_result() {
        let ev = AgentEvent::ToolCall(ToolCallEvent {
            tool: "Bash".to_string(),
            arguments: "{}".to_string(),
            status: ToolCallStatus::Succeeded,
            id: None,
            result: None,
        });
        let out = map_agent_event(ev, 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::ToolResult { content, is_error, .. } => {
                assert_eq!(content, "(ok)");
                assert!(!is_error);
            }
            other => panic!("expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn tool_call_succeeded_with_result_maps_real_content() {
        // v1.1: ReactAgent now fills `result` with the real tool output — the
        // mapped ToolResult must carry that content, not the "(ok)" placeholder.
        let ev = AgentEvent::ToolCall(ToolCallEvent {
            tool: "Read".to_string(),
            arguments: "{}".to_string(),
            status: ToolCallStatus::Succeeded,
            id: None,
            result: Some("the file contents".to_string()),
        });
        let out = map_agent_event(ev, 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::ToolResult { content, is_error, .. } => {
                assert_eq!(content, "the file contents");
                assert!(!is_error);
            }
            other => panic!("expected ToolResult with real content, got {:?}", other),
        }
    }

    #[test]
    fn tool_call_failed_with_result_maps_real_error() {
        let ev = AgentEvent::ToolCall(ToolCallEvent {
            tool: "Read".to_string(),
            arguments: "{}".to_string(),
            status: ToolCallStatus::Failed,
            id: None,
            result: Some("permission denied".to_string()),
        });
        let out = map_agent_event(ev, 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::ToolResult { content, is_error, .. } => {
                assert_eq!(content, "permission denied");
                assert!(is_error);
            }
            other => panic!("expected ToolResult with real error, got {:?}", other),
        }
    }

    #[test]
    fn tool_call_failed_maps_to_error_result() {
        let ev = AgentEvent::ToolCall(ToolCallEvent {
            tool: "Bash".to_string(),
            arguments: "{}".to_string(),
            status: ToolCallStatus::Failed,
            id: None,
            result: None,
        });
        let out = map_agent_event(ev, 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::ToolResult { content, is_error, .. } => {
                assert_eq!(content, "(failed)");
                assert!(is_error);
            }
            other => panic!("expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn file_changed_maps_to_file_changed_event() {
        // FileChanged is no longer dropped — it maps to a wire FileChanged
        // block so the chat UI renders per-write mutations as they land.
        let ev = AgentEvent::FileChanged(PathBuf::from("/x.rs"));
        let out = map_agent_event(ev, 0);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::FileChanged { path } => assert_eq!(path, "/x.rs"),
            other => panic!("expected FileChanged, got {:?}", other),
        }
    }

    #[test]
    fn turn_boundary_is_dropped() {
        assert!(map_agent_event(AgentEvent::TurnBoundary, 0).is_empty());
    }

    #[test]
    fn done_completed_maps_to_ok_result_with_elapsed_secs() {
        let outcome = AgentOutcome {
            status: AgentRunStatus::Completed,
            ..Default::default()
        };
        let out = map_agent_event(AgentEvent::Done(outcome), 42);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ChatStreamEvent::Result { is_error, secs } => {
                assert!(!is_error);
                assert_eq!(*secs, 42);
            }
            other => panic!("expected Result, got {:?}", other),
        }
    }

    #[test]
    fn done_failed_maps_to_error_result() {
        let outcome = AgentOutcome {
            status: AgentRunStatus::Failed,
            ..Default::default()
        };
        let out = map_agent_event(AgentEvent::Done(outcome), 5);
        match &out[0] {
            ChatStreamEvent::Result { is_error, secs } => {
                assert!(is_error);
                assert_eq!(*secs, 5);
            }
            other => panic!("expected Result, got {:?}", other),
        }
    }

    #[test]
    fn done_cancelled_is_treated_as_error() {
        // Cancelled != Completed → red result. The UI must not show a misleading
        // green for a stopped run.
        let outcome = AgentOutcome {
            status: AgentRunStatus::Cancelled,
            ..Default::default()
        };
        let out = map_agent_event(AgentEvent::Done(outcome), 3);
        match &out[0] {
            ChatStreamEvent::Result { is_error, .. } => assert!(is_error),
            other => panic!("expected Result, got {:?}", other),
        }
    }

    #[test]
    fn parse_tool_arguments_empty_is_null() {
        assert_eq!(parse_tool_arguments(""), Value::Null);
        assert_eq!(parse_tool_arguments("   "), Value::Null);
    }

    #[test]
    fn parse_tool_arguments_malformed_is_null() {
        assert_eq!(parse_tool_arguments("not json"), Value::Null);
    }

    #[test]
    fn parse_tool_arguments_object_passes_through() {
        let v = parse_tool_arguments(r#"{"a":1,"b":"x"}"#);
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], "x");
    }

    // ---- chat_event_to_agent_events (reverse map for OpaqueAgent) ----

    /// Assert an AgentEvent is a ToolCall with the given name + status.
    fn assert_tool(ev: &AgentEvent, expected_name: &str, expected_status: ToolCallStatus) {
        match ev {
            AgentEvent::ToolCall(tc) => {
                assert_eq!(tc.tool, expected_name, "tool name mismatch");
                assert_eq!(tc.status, expected_status, "status mismatch");
            }
            other => panic!("expected ToolCall({expected_name}), got {:?}", other),
        }
    }

    #[test]
    fn chat_text_maps_to_token() {
        let mut pending = VecDeque::new();
        let out = chat_event_to_agent_events(
            &ChatStreamEvent::Text { content: "hi".into() },
            &mut pending,
        );
        assert_eq!(out.len(), 1);
        match &out[0] {
            AgentEvent::Token(s) => assert_eq!(s, "hi"),
            other => panic!("expected Token, got {:?}", other),
        }
        assert!(pending.is_empty(), "Text must not touch the pending queue");
    }

    #[test]
    fn chat_tool_use_enqueues_and_emits_started() {
        let mut pending = VecDeque::new();
        let out = chat_event_to_agent_events(
            &ChatStreamEvent::ToolUse {
                id: None,
                name: "Read".into(),
                input: serde_json::json!({"file_path": "/x"}),
            },
            &mut pending,
        );
        assert_eq!(out.len(), 1);
        assert_tool(&out[0], "Read", ToolCallStatus::Started);
        // Arguments round-trip the input JSON exactly (serde_json::to_string).
        match &out[0] {
            AgentEvent::ToolCall(tc) => assert_eq!(tc.arguments, r#"{"file_path":"/x"}"#),
            _ => unreachable!(),
        }
        assert_eq!(pending.len(), 1);
        let front = pending.front().unwrap();
        assert_eq!(front.0, None); // id (absent on this wire)
        assert_eq!(front.1, "Read");
        assert_eq!(front.2, r#"{"file_path":"/x"}"#);
    }

    #[test]
    fn chat_tool_use_null_input_enqueues_null_args() {
        let mut pending = VecDeque::new();
        let out = chat_event_to_agent_events(
            &ChatStreamEvent::ToolUse { id: None, name: "X".into(), input: Value::Null },
            &mut pending,
        );
        match &out[0] {
            AgentEvent::ToolCall(tc) => assert_eq!(tc.arguments, "null"),
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn chat_tool_result_dequeues_and_emits_succeeded() {
        let mut pending = VecDeque::new();
        pending.push_back((None, "Read".to_string(), r#"{"file_path":"/x"}"#.to_string()));
        let out = chat_event_to_agent_events(
            &ChatStreamEvent::ToolResult { content: "res".into(), tool_use_id: None, is_error: false },
            &mut pending,
        );
        assert_eq!(out.len(), 1);
        assert_tool(&out[0], "Read", ToolCallStatus::Succeeded);
        match &out[0] {
            AgentEvent::ToolCall(tc) => assert_eq!(tc.arguments, r#"{"file_path":"/x"}"#),
            _ => unreachable!(),
        }
        assert!(pending.is_empty(), "dequeue must drain the paired ToolUse");
    }

    #[test]
    fn chat_tool_result_is_error_emits_failed() {
        let mut pending = VecDeque::new();
        pending.push_back((None, "Bash".to_string(), "{}".to_string()));
        let out = chat_event_to_agent_events(
            &ChatStreamEvent::ToolResult { content: "boom".into(), tool_use_id: None, is_error: true },
            &mut pending,
        );
        assert_tool(&out[0], "Bash", ToolCallStatus::Failed);
    }

    #[test]
    fn chat_tool_result_orphan_demotes_to_token() {
        // Orphan (no pending ToolUse): content must surface as text, never
        // vanish, and must NOT fabricate a Started ToolCall (would desync
        // downstream use/result counts).
        let mut pending = VecDeque::new();
        let out = chat_event_to_agent_events(
            &ChatStreamEvent::ToolResult { content: "orphan".into(), tool_use_id: None, is_error: false },
            &mut pending,
        );
        assert_eq!(out.len(), 1);
        match &out[0] {
            AgentEvent::Token(s) => assert_eq!(s, "orphan"),
            other => panic!("orphan must demote to Token, got {:?}", other),
        }
        assert!(pending.is_empty());
    }

    #[test]
    fn chat_result_event_emits_nothing_and_leaves_pending() {
        let mut pending = VecDeque::new();
        pending.push_back((None, "Read".to_string(), "{}".to_string()));
        let out = chat_event_to_agent_events(
            &ChatStreamEvent::Result { is_error: false, secs: 5 },
            &mut pending,
        );
        assert!(out.is_empty(), "Result must not emit — Done is owned by agent:completed");
        assert_eq!(pending.len(), 1, "Result must leave the pending queue untouched");
    }

    #[test]
    fn chat_multiple_tools_pair_fifo_fallback() {
        // FIFO fallback path: when the wire carries NO tool_use_id (ReactKernel
        // forward replay, or legacy pre-id session blocks), pairing degrades to
        // positional FIFO. Claude batches tool_uses then returns results in order:
        //   use(A), use(B), result(A), result(B)
        // FIFO dequeue (front) pairs result(A)→A, result(B)→B. A LIFO stack
        // would mis-pair result(A) onto B — this test guards that regression.
        let mut pending = VecDeque::new();
        let a = chat_event_to_agent_events(
            &ChatStreamEvent::ToolUse { name: "A".into(), input: serde_json::json!({}), id: None },
            &mut pending,
        );
        let b = chat_event_to_agent_events(
            &ChatStreamEvent::ToolUse { name: "B".into(), input: serde_json::json!({}), id: None },
            &mut pending,
        );
        let r1 = chat_event_to_agent_events(
            &ChatStreamEvent::ToolResult { content: "ra".into(), tool_use_id: None, is_error: false },
            &mut pending,
        );
        let r2 = chat_event_to_agent_events(
            &ChatStreamEvent::ToolResult { content: "rb".into(), tool_use_id: None, is_error: false },
            &mut pending,
        );
        assert_tool(&a[0], "A", ToolCallStatus::Started);
        assert_tool(&b[0], "B", ToolCallStatus::Started);
        // FIFO: first result dequeues A (front), second dequeues B.
        assert_tool(&r1[0], "A", ToolCallStatus::Succeeded);
        assert_tool(&r2[0], "B", ToolCallStatus::Succeeded);
        assert!(pending.is_empty());
    }

    #[test]
    fn chat_tools_pair_by_id_out_of_order() {
        // Id-first pairing (OpaqueAgent path — claude wire always emits
        // tool_use_id): results arriving OUT of pending order still pair to the
        // right ToolUse by id, not by position. The old FIFO hack only happened
        // to be right when use/result order matched; id pairing is
        // order-independent (defect ① root cause).
        let mut pending = VecDeque::new();
        let _a = chat_event_to_agent_events(
            &ChatStreamEvent::ToolUse { id: Some("u_A".into()), name: "A".into(), input: json!({}) },
            &mut pending,
        );
        let _b = chat_event_to_agent_events(
            &ChatStreamEvent::ToolUse { id: Some("u_B".into()), name: "B".into(), input: json!({}) },
            &mut pending,
        );
        // result(B) arrives BEFORE result(A) — FIFO would mis-pair it onto A.
        let rb = chat_event_to_agent_events(
            &ChatStreamEvent::ToolResult { tool_use_id: Some("u_B".into()), content: "rb".into(), is_error: false },
            &mut pending,
        );
        let ra = chat_event_to_agent_events(
            &ChatStreamEvent::ToolResult { tool_use_id: Some("u_A".into()), content: "ra".into(), is_error: false },
            &mut pending,
        );
        assert_tool(&rb[0], "B", ToolCallStatus::Succeeded); // id u_B → B (not FIFO's A)
        assert_tool(&ra[0], "A", ToolCallStatus::Succeeded); // id u_A → A
        assert!(pending.is_empty());
    }

    #[test]
    fn chat_tool_result_orphan_when_id_unmatched() {
        // tool_use_id present but no pending ToolUse with that id (the match was
        // already consumed, or its ToolUse never arrived): content demotes to
        // Token — never silently dropped, never fabricated as Started (would
        // desync use/result counts).
        let mut pending = VecDeque::new();
        let _ = chat_event_to_agent_events(
            &ChatStreamEvent::ToolUse { id: Some("u_X".into()), name: "X".into(), input: json!({}) },
            &mut pending,
        );
        // id u_Y was never enqueued — orphan by id mismatch (not by empty queue).
        let out = chat_event_to_agent_events(
            &ChatStreamEvent::ToolResult { tool_use_id: Some("u_Y".into()), content: "stray".into(), is_error: false },
            &mut pending,
        );
        assert_eq!(out.len(), 1);
        match &out[0] {
            AgentEvent::Token(s) => assert_eq!(s, "stray"),
            other => panic!("unmatched-id ToolResult must demote to Token, got {:?}", other),
        }
        // The pending u_X is left untouched (its result never arrived).
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn chat_roundtrip_text_preserves_token() {
        // AgentEvent::Token → map_agent_event → Text → chat_event_to_agent_events
        // → Token. Round-trip on the Text/Token axis (the axis claude's wire
        // actually exercises) proves the two maps are consistent inverses.
        let forward = map_agent_event(AgentEvent::Token("x".to_string()), 0);
        assert_eq!(forward.len(), 1);
        let mut pending = VecDeque::new();
        let back = chat_event_to_agent_events(&forward[0], &mut pending);
        assert_eq!(back.len(), 1);
        match &back[0] {
            AgentEvent::Token(s) => assert_eq!(s, "x"),
            other => panic!("roundtrip lost Token, got {:?}", other),
        }
        assert!(pending.is_empty());
    }

    // ---- turns_to_history ----

    use crate::models::{AgentType, ContextSnapshot, Session};
    use serde_json::json;

    /// Minimal completed session with the fields turns_to_history reads.
    fn turn(id: &str, prompt: &str, blocks: Option<Value>, summary: Option<&str>) -> Session {
        Session {
            id: id.to_string(),
            project_path: "/p".to_string(),
            agent_type: AgentType::ClaudeCode,
            status: SessionStatus::Completed,
            prompt: prompt.to_string(),
            model: None,
            started_at: id.to_string(), // lexical ASC == chronological for tests
            finished_at: None,
            exit_code: Some(0),
            output_summary: summary.map(|s| s.to_string()),
            context_snapshot: None as Option<ContextSnapshot>,
            linked_requirement_id: None,
            parent_session_id: None,
            conversation_id: None,
            blocks,
            task_ref: None,
        }
    }

    fn blocks_json(events: &[ChatStreamEvent]) -> Value {
        serde_json::to_value(events).unwrap()
    }

    #[test]
    fn empty_turns_yields_empty_history() {
        let out = turns_to_history(&[], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        assert!(out.is_empty());
    }

    #[test]
    fn single_turn_with_blocks_preserves_tool_pairs() {
        // NEW: blocks_to_history preserves tool_use/tool_result pairing instead of
        // stripping them. Each turn expands to user + assistant(with tool_calls) + [tool(result)..].
        let t = turn(
            "t0",
            "read the file",
            Some(blocks_json(&[
                ChatStreamEvent::Text { content: "reading now".into() },
                ChatStreamEvent::ToolUse { id: None, name: "Read".into(), input: json!({"file_path":"/x"}) },
                ChatStreamEvent::ToolResult { content: "file contents".into(), tool_use_id: None, is_error: false },
                ChatStreamEvent::Text { content: "done".into() },
            ])),
            None,
        );
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        // user + assistant(with tool_calls) + tool(result) = 3 messages.
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].role, Role::User);
        assert_eq!(out[0].content, "read the file");
        assert_eq!(out[1].role, Role::Assistant);
        // F1: the FIFO-matched ToolUse appears in assistant.tool_calls.
        assert_eq!(out[1].tool_calls.len(), 1);
        assert_eq!(out[2].role, Role::Tool);
        assert_eq!(out[2].content, "file contents");
        // tool_result references the same id as the assistant tool_call (F1 invariant).
        assert_eq!(out[2].tool_call_id.as_deref(), Some(out[1].tool_calls[0].id.as_str()));
    }

    #[test]
    fn blocks_none_falls_back_to_output_summary() {
        let t = turn("t0", "ask", None, Some("the answer is 42"));
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, Role::User);
        assert_eq!(out[1].role, Role::Assistant);
        assert_eq!(out[1].content, "the answer is 42");
    }

    #[test]
    fn blocks_none_and_summary_none_keeps_only_user_message() {
        // No fabricated empty assistant turn — some providers reject them.
        let t = turn("t0", "ask", None, None);
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].role, Role::User);
    }

    #[test]
    fn rebuild_turn_messages_running_empty_blocks_rebuild_none_fallback() {
        // Defect ④: rebuild_turn_messages is the shared central rebuilder for
        // both turns_to_history (ReactKernel) and inject_conversation_context
        // (OpaqueAgent). Lock its three branches directly.

        // Running turn → empty (caller filters). turn() defaults to Completed,
        // so override to Running.
        let mut running = turn("r", "p", None, None);
        running.status = SessionStatus::Running;
        assert!(rebuild_turn_messages(&running, REACT_HISTORY_TURN_TEXT_CAP).is_empty());

        // Blocks with a tool pair → [user, assistant(tool_calls), tool(result)].
        let t = turn(
            "t0",
            "read the file",
            Some(blocks_json(&[
                ChatStreamEvent::Text { content: "reading now".into() },
                ChatStreamEvent::ToolUse { id: None, name: "Read".into(), input: json!({"file_path":"/x"}) },
                ChatStreamEvent::ToolResult { content: "file contents".into(), tool_use_id: None, is_error: false },
            ])),
            None,
        );
        let out = rebuild_turn_messages(&t, REACT_HISTORY_TURN_TEXT_CAP);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].role, Role::User);
        assert_eq!(out[1].role, Role::Assistant);
        assert_eq!(out[1].tool_calls.len(), 1);
        assert_eq!(out[2].role, Role::Tool);

        // Bugfix: None blocks + summary → [user, assistant(summary)]. Previously
        // the `None` arm returned only [user] and dropped the summary (the test
        // binary's 0xc0000139 hid this — react_chat tests never ran). Unified
        // here so legacy / OpaqueAgent turns inherit their output text.
        let legacy = turn("l", "ask", None, Some("the answer is 42"));
        let out2 = rebuild_turn_messages(&legacy, REACT_HISTORY_TURN_TEXT_CAP);
        assert_eq!(out2.len(), 2);
        assert_eq!(out2[0].role, Role::User);
        assert_eq!(out2[1].role, Role::Assistant);
        assert_eq!(out2[1].content, "the answer is 42");
    }

    #[test]
    fn two_turns_stay_chronological_oldest_first() {
        let t0 = turn("a", "first prompt", Some(blocks_json(&[
            ChatStreamEvent::Text { content: "first reply".into() },
        ])), None);
        let t1 = turn("b", "second prompt", Some(blocks_json(&[
            ChatStreamEvent::Text { content: "second reply".into() },
        ])), None);
        let out = turns_to_history(&[t0, t1], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        // [user(a), assistant(first), user(b), assistant(second)]
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].content, "first prompt");
        assert_eq!(out[1].content, "first reply");
        assert_eq!(out[2].content, "second prompt");
        assert_eq!(out[3].content, "second reply");
    }

    #[test]
    fn multiple_tool_uses_and_results_preserves_pairs() {
        // Two FIFO-matched pairs: assistant carries BOTH tool_calls (F1), and both
        // results survive without overwriting each other (F3 — synthesized ids).
        let t = turn("t0", "do two things", Some(blocks_json(&[
            ChatStreamEvent::Text { content: "ok".into() },
            ChatStreamEvent::ToolUse { name: "A".into(), input: json!({}), id: None },
            ChatStreamEvent::ToolUse { name: "B".into(), input: json!({}), id: None },
            ChatStreamEvent::ToolResult { content: "resA".into(), tool_use_id: None, is_error: false },
            ChatStreamEvent::ToolResult { content: "resB".into(), tool_use_id: None, is_error: false },
        ])), None);
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        // user(1) + assistant(2 tool_calls)(1) + 2 tool_results = 4 messages.
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].role, Role::User);
        assert_eq!(out[1].role, Role::Assistant);
        assert_eq!(out[1].tool_calls.len(), 2);
        assert_eq!(out[1].content, "ok");
        assert_eq!(out[2].role, Role::Tool);
        assert_eq!(out[3].role, Role::Tool);
        // F3: both pairs survive in ToolUse arrival order (resA then resB).
        assert_eq!(out[2].content, "resA");
        assert_eq!(out[3].content, "resB");
    }

    #[test]
    fn total_text_cap_drops_oldest_whole_turn_keeps_newest() {
        // Tiny total cap so only the newest turn fits; the older one must be
        // dropped as a whole (user+assistant together), never split.
        let t0 = turn("a", "old prompt that is fairly long", Some(blocks_json(&[
            ChatStreamEvent::Text { content: "old reply also fairly long".into() },
        ])), None);
        let t1 = turn("b", "new prompt that is fairly long", Some(blocks_json(&[
            ChatStreamEvent::Text { content: "new reply also fairly long".into() },
        ])), None);
        let out = turns_to_history(&[t0, t1], REACT_HISTORY_TURN_TEXT_CAP, 30);
        // Only the newest turn survives, intact.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].content, "new prompt that is fairly long");
        assert_eq!(out[1].role, Role::Assistant);
    }

    #[test]
    fn message_count_cap_drops_oldest_whole_turns() {
        // NEW: Each turn generates 3 messages (user + assistant with tool_calls + tool result).
        // Force the message-count cap by making many turns; assert we never
        // exceed REACT_HISTORY_TOTAL_MESSAGES and never split a turn.
        let turns: Vec<Session> = (0..30)
            .map(|i| turn(&format!("t{:02}", i), &format!("p{}", i),
                Some(blocks_json(&[
                    ChatStreamEvent::Text { content: format!("r{}", i) },
                    ChatStreamEvent::ToolUse { name: "X".into(), input: json!({}), id: None },
                    ChatStreamEvent::ToolResult { content: "z".into(), tool_use_id: None, is_error: false },
                ])), None))
            .collect();
        // 30 turns × 3 messages (tool pairs preserved) = 90 > cap of 40.
        let out = turns_to_history(&turns, REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        assert!(out.len() <= REACT_HISTORY_TOTAL_MESSAGES);
        // Newest turn's prompt must be present; oldest must be gone.
        let prompts: Vec<&str> = out.iter().filter(|m| m.role == Role::User).map(|m| m.content.as_str()).collect();
        assert!(prompts.contains(&"p29"));
        assert!(!prompts.contains(&"p0"));
    }

    #[test]
    fn turn_with_only_tool_calls_keeps_assistant_with_tool_calls() {
        // F1 fix: a tool-only turn (no assistant text) STILL emits the assistant
        // message carrying tool_calls — it is NOT dropped. Dropping it would
        // orphan the tool_result (no preceding tool_use) → provider 400.
        let t = turn("t0", "ask", Some(blocks_json(&[
            ChatStreamEvent::ToolUse { name: "Read".into(), input: json!({}), id: None },
            ChatStreamEvent::ToolResult { content: "file contents".into(), tool_use_id: None, is_error: false },
        ])), None);
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        // user + assistant(empty text, 1 tool_call) + tool = 3.
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].role, Role::User);
        assert_eq!(out[1].role, Role::Assistant);
        assert_eq!(out[1].content, "");
        assert_eq!(out[1].tool_calls.len(), 1);
        assert_eq!(out[2].role, Role::Tool);
        assert_eq!(out[2].content, "file contents");
    }

    #[test]
    fn long_assistant_text_is_truncated_to_turn_cap() {
        // The tail() cap still bounds the kept assistant text — a long reply
        // doesn't blow past turn_text_cap even after tool calls are stripped.
        // (Replaces the old tool-result truncation test: tool messages no longer
        // exist, so the truncation guarantee now applies to the assistant text.)
        let long = "x".repeat(REACT_HISTORY_TURN_TEXT_CAP * 2);
        let t = turn("t0", "ask", Some(blocks_json(&[
            ChatStreamEvent::Text { content: long.clone() },
        ])), None);
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        let assistant = out.iter().find(|m| m.role == Role::Assistant).unwrap();
        // tail() prepends "..." (3 chars) + the trailing turn_text_cap chars.
        assert!(assistant.content.len() <= REACT_HISTORY_TURN_TEXT_CAP + 4);
        assert!(assistant.content.starts_with("..."));
    }

    #[test]
    fn malformed_blocks_json_does_not_panic() {
        // A blocks column that isn't a valid ChatStreamEvent array must degrade
        // gracefully — fall back to output_summary, never panic the run.
        let t = turn("t0", "ask", Some(json!({"not": "an array"})), Some("fallback summary"));
        let out = turns_to_history(&[t], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        // from_value::<Vec<_>> fails on a non-array → summary fallback path.
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].role, Role::Assistant);
        assert_eq!(out[1].content, "fallback summary");
    }

    #[test]
    fn running_turns_are_skipped() {
        // A turn still running has no finalized reply; it must not appear as a
        // lone user message in the resumed history.
        let mut running = turn("r", "in-flight prompt", None, None);
        running.status = SessionStatus::Running;
        let done = turn("d", "settled prompt", Some(blocks_json(&[
            ChatStreamEvent::Text { content: "settled reply".into() },
        ])), None);
        let out = turns_to_history(&[running, done], REACT_HISTORY_TURN_TEXT_CAP, REACT_HISTORY_TOTAL_TEXT_CAP);
        // Only the settled turn's 2 messages appear.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].content, "settled prompt");
    }

    // ---- blocks_to_history specific unit tests ----

    #[test]
    fn blocks_to_history_empty_blocks() {
        assert!(blocks_to_history(&[], REACT_HISTORY_TURN_TEXT_CAP).is_empty());
    }

    #[test]
    fn blocks_to_history_text_only() {
        let msgs = blocks_to_history(&[
            ChatStreamEvent::Text { content: "hello world".into() },
        ], REACT_HISTORY_TURN_TEXT_CAP);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, Role::Assistant);
        assert_eq!(msgs[0].content, "hello world");
    }

    #[test]
    fn blocks_to_history_id_matched_pairs() {
        // Id-based pairing: tool_use_id matches ToolUse.id exactly.
        let msgs = blocks_to_history(&[
            ChatStreamEvent::Text { content: "processing".into() },
            ChatStreamEvent::ToolUse { id: Some("t1".into()), name: "Read".into(), input: json!({"path":"x"}) },
            ChatStreamEvent::ToolResult { tool_use_id: Some("t1".into()), content: "file body".into(), is_error: false },
        ], REACT_HISTORY_TURN_TEXT_CAP);
        assert_eq!(msgs.len(), 2); // assistant + tool
        assert_eq!(msgs[0].role, Role::Assistant);
        // F1 invariant: matched ToolUse MUST appear in assistant.tool_calls —
        // the tool_result below references "t1", so the assistant must carry it.
        assert_eq!(msgs[0].tool_calls.len(), 1);
        assert_eq!(msgs[0].tool_calls[0].id, "t1");
        assert_eq!(msgs[0].tool_calls[0].function.name, "Read");
        assert_eq!(msgs[1].role, Role::Tool);
        assert_eq!(msgs[1].content, "file body");
        assert_eq!(msgs[1].tool_call_id.as_deref(), Some("t1"));
    }

    #[test]
    fn blocks_to_history_fifo_match() {
        // No ids → FIFO match: first ToolUse gets paired with first ToolResult.
        let msgs = blocks_to_history(&[
            ChatStreamEvent::Text { content: "ok".into() },
            ChatStreamEvent::ToolUse { id: None, name: "Bash".into(), input: json!({"cmd":"ls"}) },
            ChatStreamEvent::ToolResult { tool_use_id: None, content: "out1".into(), is_error: false },
        ], REACT_HISTORY_TURN_TEXT_CAP);
        assert_eq!(msgs.len(), 2); // assistant + tool
        assert_eq!(msgs[0].role, Role::Assistant);
        // F1+F5: the FIFO-matched ToolUse appears in tool_calls with a synthesized
        // non-empty id (never ""), so the tool_result can link back to it.
        assert_eq!(msgs[0].tool_calls.len(), 1);
        assert_eq!(msgs[0].tool_calls[0].id, "__fifo_0__");
        assert_eq!(msgs[1].role, Role::Tool);
        assert_eq!(msgs[1].content, "out1");
        assert_eq!(msgs[1].tool_call_id.as_deref(), Some("__fifo_0__"));
    }

    #[test]
    fn blocks_to_history_orphan_ending_in_tool_calls() {
        // ToolUse without matching result → appears in assistant.tool_calls.
        let msgs = blocks_to_history(&[
            ChatStreamEvent::Text { content: "done".into() },
            ChatStreamEvent::ToolUse { id: Some("t1".into()), name: "Write".into(), input: json!({"path":"f"}) },
            // Missing ToolResult for t1.
        ], REACT_HISTORY_TURN_TEXT_CAP);
        assert_eq!(msgs.len(), 1); // assistant only, no tool result message
        assert_eq!(msgs[0].role, Role::Assistant);
        assert_eq!(msgs[0].tool_calls.len(), 1);
        assert_eq!(msgs[0].tool_calls[0].id, "t1");
    }

    #[test]
    fn blocks_to_history_meta_events_dropped() {
        // Result, FileChanged, Compact, ApprovalRequired → dropped.
        let msgs = blocks_to_history(&[
            ChatStreamEvent::Text { content: "hi".into() },
            ChatStreamEvent::Result { is_error: false, secs: 1 },
            ChatStreamEvent::FileChanged { path: "/x".to_string() },
            ChatStreamEvent::Compact { summary: "compacted".into(), archived_at: None, dropped_count: 5, is_error: false },
            ChatStreamEvent::ApprovalRequired { tool: "bash".into(), arguments: "{}".into(), resume_token: "r1".into(), summary: "destr".into() },
        ], REACT_HISTORY_TURN_TEXT_CAP);
        assert_eq!(msgs.len(), 1); // only assistant text
        assert_eq!(msgs[0].role, Role::Assistant);
    }

    #[test]
    fn blocks_to_history_compact_boundary_becomes_marker_message() {
        // §4.2 缺项3: a CompactBoundary event is NOT dropped like the other
        // meta-events — it's reconstructed into a System-role boundary Message
        // carrying the compact_boundary meta, appended after the turn's real
        // content. This is what lets maybe_compact's last_boundary_index find
        // it on resume and avoid re-compacting already-summarized history.
        let msgs = blocks_to_history(
            &[
                ChatStreamEvent::Text { content: "assistant work".into() },
                ChatStreamEvent::CompactBoundary {
                    trigger: "auto".into(),
                    pre_tokens: 4500,
                    preserved_count: 3,
                },
            ],
            REACT_HISTORY_TURN_TEXT_CAP,
        );
        // assistant text + the boundary marker.
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::Assistant);
        assert_eq!(msgs[0].content, "assistant work");
        assert_eq!(msgs[1].role, Role::System);
        let meta = msgs[1]
            .compact_boundary
            .as_ref()
            .expect("boundary meta must be present on the marker");
        assert_eq!(meta.trigger, "auto");
        assert_eq!(meta.pre_tokens, 4500);
        assert_eq!(meta.preserved_count, 3);
    }

    #[test]
    fn blocks_to_history_boundary_survives_vacuous_turn() {
        // §4.2 缺项3: a turn that is ONLY a CompactBoundary (no assistant
        // content) must still emit the marker — returning empty would lose it
        // and the next resume would re-compact already-summarized history. The
        // empty assistant is dropped (would corrupt provider role-ordering);
        // the marker stands alone.
        let msgs = blocks_to_history(
            &[ChatStreamEvent::CompactBoundary {
                trigger: "auto".into(),
                pre_tokens: 9000,
                preserved_count: 2,
            }],
            REACT_HISTORY_TURN_TEXT_CAP,
        );
        assert_eq!(msgs.len(), 1, "boundary survives a vacuous turn");
        assert_eq!(msgs[0].role, Role::System);
        assert!(msgs[0].compact_boundary.is_some());
    }

    #[test]
    fn blocks_to_history_compact_summary_rebuilds_as_fenced_user_message() {
        // resume Compact summary 重建: Compact(is_error=false, Summarize path)
        // + CompactBoundary → [summary(User+反注入围栏), boundary(System)],
        // matching live maybe_compact's history order (summary@start, then
        // boundary@start+1). Without this, a resumed session's blocks_to_history
        // dropped the Compact event and the prior turn's compaction summary was
        // lost entirely — resume saw the boundary but not what it summarized.
        let msgs = blocks_to_history(
            &[
                ChatStreamEvent::Text { content: "assistant turn".into() },
                ChatStreamEvent::Compact {
                    summary: "用户问了 X，我读了 a.rs".into(),
                    archived_at: None,
                    dropped_count: 4,
                    is_error: false,
                },
                ChatStreamEvent::CompactBoundary {
                    trigger: "auto".into(),
                    pre_tokens: 5000,
                    preserved_count: 3,
                },
            ],
            REACT_HISTORY_TURN_TEXT_CAP,
        );
        // assistant + summary(User) + boundary(System); summary BEFORE boundary.
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, Role::Assistant);
        assert_eq!(msgs[1].role, Role::User, "summary rebuilds as a User message");
        assert!(
            msgs[1].content.starts_with("[此前对话摘要"),
            "summary must carry the anti-injection fence preamble; got: {}",
            msgs[1].content,
        );
        assert!(
            msgs[1].content.contains("用户问了 X，我读了 a.rs"),
            "fenced summary must contain the original summary text; got: {}",
            msgs[1].content,
        );
        assert_eq!(msgs[2].role, Role::System);
        assert!(msgs[2].compact_boundary.is_some());
    }

    #[test]
    fn blocks_to_history_compact_error_not_rebuilt_into_history() {
        // is_error=true Compact (HardTruncate / BreakerTripped) carries a
        // failure/pause notice, NOT a history summary — never rebuilt into the
        // model's history. Only its paired boundary (if any) survives.
        let msgs = blocks_to_history(
            &[
                ChatStreamEvent::Text { content: "work".into() },
                ChatStreamEvent::Compact {
                    summary: "压缩已暂停且仍超上限，紧急丢弃最早历史".into(),
                    archived_at: None,
                    dropped_count: 10,
                    is_error: true,
                },
                ChatStreamEvent::CompactBoundary {
                    trigger: "auto".into(),
                    pre_tokens: 6000,
                    preserved_count: 2,
                },
            ],
            REACT_HISTORY_TURN_TEXT_CAP,
        );
        // assistant + boundary only — the error notice is dropped.
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::Assistant);
        assert_eq!(msgs[1].role, Role::System);
        assert!(msgs[1].compact_boundary.is_some());
        assert!(
            !msgs.iter().any(|m| m.content.contains("紧急丢弃")),
            "error notice must not enter history",
        );
    }

    #[test]
    fn blocks_to_history_microclear_compact_without_boundary_dropped() {
        // MicroClear emits Compact(is_error=false) with NO CompactBoundary (no
        // structural change). Its summary is a UI description ("已压缩 N 条"),
        // not a history 摘要 — pending_summary never pairs with a boundary and
        // is dropped at end-of-turn.
        let msgs = blocks_to_history(
            &[
                ChatStreamEvent::Text { content: "turn".into() },
                ChatStreamEvent::Compact {
                    summary: "已压缩 3 条陈旧工具输出".into(),
                    archived_at: None,
                    dropped_count: 3,
                    is_error: false,
                },
            ],
            REACT_HISTORY_TURN_TEXT_CAP,
        );
        // assistant only — the unpaired MicroClear summary is dropped.
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, Role::Assistant);
        assert!(!msgs[0].content.contains("陈旧工具输出"));
    }

    #[test]
    fn blocks_to_history_vacuous_turn_compact_returns_summary_and_boundary() {
        // A turn whose blocks are ONLY Compact+CompactBoundary (no assistant
        // content) is vacuous but must still rebuild [summary, boundary] —
        // collapsing to empty would lose both the prior summary AND the boundary
        // anchor on resume.
        let msgs = blocks_to_history(
            &[
                ChatStreamEvent::Compact {
                    summary: "上轮摘要".into(),
                    archived_at: None,
                    dropped_count: 2,
                    is_error: false,
                },
                ChatStreamEvent::CompactBoundary {
                    trigger: "auto".into(),
                    pre_tokens: 7000,
                    preserved_count: 4,
                },
            ],
            REACT_HISTORY_TURN_TEXT_CAP,
        );
        assert_eq!(msgs.len(), 2, "vacuous turn returns [summary, boundary]");
        assert_eq!(msgs[0].role, Role::User);
        assert!(msgs[0].content.contains("上轮摘要"));
        assert_eq!(msgs[1].role, Role::System);
        assert!(msgs[1].compact_boundary.is_some());
    }

    #[test]
    fn blocks_to_history_truncates_tool_result_content() {
        let long_content = "x".repeat(5000);
        let msgs = blocks_to_history(&[
            ChatStreamEvent::ToolUse { id: Some("t1".into()), name: "Read".into(), input: json!({}) },
            ChatStreamEvent::ToolResult { tool_use_id: Some("t1".into()), content: long_content, is_error: false },
        ], REACT_HISTORY_TURN_TEXT_CAP);
        assert_eq!(msgs.len(), 2);
        // F1: matched ToolUse in tool_calls.
        assert_eq!(msgs[0].tool_calls.len(), 1);
        assert_eq!(msgs[0].tool_calls[0].id, "t1");
        assert!(msgs[1].content.len() <= REACT_HISTORY_TURN_TEXT_CAP + 4); // "..." prefix
    }

    #[test]
    fn blocks_to_history_preserves_multiple_pairs_order() {
        // Multiple complete pairs: tool results should appear after assistant.
        let msgs = blocks_to_history(&[
            ChatStreamEvent::Text { content: "running".into() },
            ChatStreamEvent::ToolUse { id: Some("a1".into()), name: "A".into(), input: json!({}) },
            ChatStreamEvent::ToolResult { tool_use_id: Some("a1".into()), content: "ra".into(), is_error: false },
            ChatStreamEvent::ToolUse { id: Some("b2".into()), name: "B".into(), input: json!({}) },
            ChatStreamEvent::ToolResult { tool_use_id: Some("b2".into()), content: "rb".into(), is_error: false },
        ], REACT_HISTORY_TURN_TEXT_CAP);
        assert_eq!(msgs.len(), 3); // assistant + 2 tool results
        assert_eq!(msgs[0].role, Role::Assistant);
        // F1: both matched ToolUses in tool_calls, in arrival order.
        assert_eq!(msgs[0].tool_calls.len(), 2);
        assert_eq!(msgs[0].tool_calls[0].id, "a1");
        assert_eq!(msgs[0].tool_calls[1].id, "b2");
        // F4: tool results in ToolUse arrival order (a1 then b2).
        assert_eq!(msgs[1].content, "ra");
        assert_eq!(msgs[1].tool_call_id.as_deref(), Some("a1"));
        assert_eq!(msgs[2].content, "rb");
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("b2"));
    }

    // ---- Defect ③: Pairing invariant validation tests ----

    #[test]
    fn validate_block_pairs_empty_is_clean() {
        let blocks: Vec<ChatStreamEvent> = vec![ChatStreamEvent::Text { content: "hello".into() }];
        let violations = validate_block_pairs(&blocks);
        assert!(violations.is_empty(), "non-tool events do not trigger violations");
    }

    #[test]
    fn validate_block_pairs_complete_pair_is_clean() {
        let blocks = vec![
            ChatStreamEvent::ToolUse { id: Some("u1".into()), name: "Read".into(), input: json!({}) },
            ChatStreamEvent::ToolResult { tool_use_id: Some("u1".into()), content: "file body".into(), is_error: false },
        ];
        let violations = validate_block_pairs(&blocks);
        assert!(violations.is_empty(), "complete pair must not trigger violations");
    }

    #[test]
    fn validate_block_pairs_orphan_tool_call_detected() {
        // ToolUse without matching result → orphan.
        let blocks = vec![
            ChatStreamEvent::ToolUse { id: Some("u1".into()), name: "Read".into(), input: json!({}) },
            ChatStreamEvent::ToolResult { tool_use_id: Some("u2".into()), content: "ok".into(), is_error: false }, // matches nothing — dangling, not orphaned read
            // u1 has no result → orphan
        ];
        let violations = validate_block_pairs(&blocks);
        assert_eq!(violations.len(), 2, "expecting 1 orphan + 1 dangling");

        let orphans: Vec<_> = violations.iter().filter_map(|v| match v {
            PairingViolation::OrphanToolCall { .. } => Some(v),
            _ => None,
        }).collect();
        assert_eq!(orphans.len(), 1);
        match &orphans[0] {
            PairingViolation::OrphanToolCall { id, name } => {
                assert_eq!(id.as_deref(), Some("u1"));
                assert_eq!(name.as_str(), "Read");
            }
            _ => panic!("expected OrphanToolCall"),
        }

        let dangling: Vec<_> = violations.iter().filter_map(|v| match v {
            PairingViolation::DanglingToolResult { .. } => Some(v),
            _ => None,
        }).collect();
        assert_eq!(dangling.len(), 1);
    }

    #[test]
    fn validate_block_pairs_dangling_result_detected() {
        // Result appears before its ToolUse → dangling.
        let blocks = vec![
            ChatStreamEvent::ToolResult { tool_use_id: Some("u1".into()), content: "ok".into(), is_error: false },
            ChatStreamEvent::ToolUse { id: Some("u1".into()), name: "Read".into(), input: json!({}) },
        ];
        let violations = validate_block_pairs(&blocks);
        assert_eq!(violations.len(), 1, "result before tool_use → dangling");
        assert!(matches!(&violations[0], PairingViolation::DanglingToolResult { id: Some(id) } if id == "u1"));
    }

    #[test]
    fn validate_block_pairs_fifo_orphan_detected() {
        // FIFO path: id-less ToolUse without result.
        let blocks = vec![
            ChatStreamEvent::ToolUse { id: None, name: "Bash".into(), input: json!({}) },
        ];
        let violations = validate_block_pairs(&blocks);
        assert_eq!(violations.len(), 1);
        assert!(matches!(&violations[0], PairingViolation::OrphanToolCall { id: None, name } if name == "Bash"));
    }

    #[test]
    fn validate_block_pairs_error_result_ignored() {
        // is_error=true results are skipped — they mark the cut point, not a dangling result.
        let blocks = vec![
            ChatStreamEvent::ToolUse { id: Some("u1".into()), name: "Read".into(), input: json!({}) },
            ChatStreamEvent::ToolResult { tool_use_id: Some("u1".into()), content: "boom".into(), is_error: true },
        ];
        let violations = validate_block_pairs(&blocks);
        assert!(violations.is_empty(), "error result does not create dangling or orphan");
    }

    #[test]
    fn drain_pending_orphans_finds_all() {
        let mut pending = VecDeque::new();
        pending.push_back((Some("a".into()), "Read".to_string(), "{}".to_string()));
        pending.push_back((None, "Bash".to_string(), "ls".to_string()));
        pending.push_back((Some("b".into()), "Edit".to_string(), "{}".to_string()));

        let violations = drain_pending_orphans(&mut pending);
        assert_eq!(violations.len(), 3);
        // All should be OrphanToolCall.
        for v in &violations {
            assert!(matches!(v, PairingViolation::OrphanToolCall { .. }));
        }
        assert!(pending.is_empty());
    }

    #[test]
    fn drain_pending_orphans_empty_when_all_paired() {
        let mut pending = VecDeque::new();
        let violations = drain_pending_orphans(&mut pending);
        assert!(violations.is_empty(), "empty queue → no orphans");
    }
}
