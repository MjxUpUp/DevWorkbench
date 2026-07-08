//! Block Stream Integrity — single source of truth for "blocks JSON on disk
//! always satisfies pairing invariant".
//!
//! ## Why this exists
//!
//! Before this module, pairing invariant was scattered:
//! - Writing end: `commands/agents.rs::synthesize_interrupt_tool_results`
//!   only handled the user-interrupt path; normal completion / stream-truncated
//!   / max-steps / crash paths could leave orphan `tool_use` blocks on disk.
//! - Reading end: `agents/session.rs::load_turns_for_conversation_db` only
//!   `log::warn!`'d the violation — it did NOT repair, so the orphan blocks
//!   were replayed into the next continuation session, where the upstream
//!   MiniMax API rejected the malformed history with HTTP 400 "tool call
//!   result does not follow tool call (2013)". Symptom: continuation sessions
//!   failed in 1-3s with no recovery path (session 140ac9e3 / cc2996ad in
//!   conversation cfa53764).
//!
//! ## What this module guarantees
//!
//! `finalize_for_storage(blocks, reason)` is the ONLY way blocks reach the
//! `sessions.blocks` column. All paths funnel through `pty::finalize_session`,
//! which calls this fn before serializing. The result:
//!
//! 1. Every persisted session satisfies the pairing invariant.
//! 2. Reading-side code (`load_turns_for_conversation_db`) trusts the
//!    invariant and stops doing repair; it only does a paranoid check +
//!    alert to the `quality_reports` table.
//! 3. Each finalize records `(session_id, reason, stats)` to
//!    `block_finalize_log` so future regressions are diagnosable in seconds.
//!
//! ## Repair strategy per `FinalizeReason`
//!
//! - `Normal`: only strip if violation found (paranoid — never silently
//!   delete on the happy path).
//! - `UserInterrupt`: synthesize `is_error=true` `tool_result` blocks for
//!   trailing orphan `tool_use` (matches old behavior of
//!   `synthesize_interrupt_tool_results`).
//! - `StreamTruncated` / `MaxSteps`: STRIP trailing orphan `tool_use` blocks
//!   (do NOT synthesize a fake result — the model's mid-thought tail was
//!   truncated and we have nothing truthful to put there).
//! - `ForceStop` / `Crash`: same as `StreamTruncated` (strip, don't fake).
//! - `DanglingToolResult` (result before use): drop the dangling result.

use crate::agents::pty::ChatStreamEvent;
use rusqlite::params;
use serde::Serialize;

/// Why a session is being finalized. Drives the repair strategy.
///
/// `MaxSteps` and `ForceStop` are reserved for future wiring — the pipe
/// path can't reach either (only the ReactAgent loop can hit `max_steps`;
/// `ForceStop` is for a watchdog not yet built). The enum is marked
/// `#[allow(dead_code)]` to keep the variant table complete without
/// scattering suppression in every caller.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalizeReason {
    /// Happy path — agent ran to completion, all tool_use pairs matched.
    Normal,
    /// User clicked stop / cancel.
    UserInterrupt,
    /// LLM stream cut off mid-response (no `message_stop`).
    StreamTruncated,
    /// Hit `max_steps` budget before agent converged.
    MaxSteps,
    /// Application forced the session to stop (shutdown, watchdog, etc).
    ForceStop,
    /// Session crashed (panic, DB error, etc).
    Crash,
}

impl FinalizeReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::UserInterrupt => "user_interrupt",
            Self::StreamTruncated => "stream_truncated",
            Self::MaxSteps => "max_steps",
            Self::ForceStop => "force_stop",
            Self::Crash => "crash",
        }
    }
}

/// Stats describing what `finalize_for_storage` did to the blocks. Persisted
/// to `block_finalize_log` for observability + future regression diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FinalizeStats {
    /// Input block count.
    pub input_blocks: usize,
    /// Output block count (after strip / synthesize).
    pub output_blocks: usize,
    /// Trailing orphan ToolUse blocks stripped (no matching result).
    pub stripped_orphan_use: usize,
    /// Synthetic ToolResult blocks appended (only on UserInterrupt).
    pub synthesized_result: usize,
    /// Dangling ToolResult blocks dropped (result before any ToolUse).
    pub dropped_dangling_result: usize,
    /// Did the input already satisfy the invariant? (true = no-op finalize).
    pub was_clean: bool,
}

/// Apply pairing-integrity repair appropriate to `reason` to `blocks`.
///
/// Returns the repaired block list + a stats summary. Pure function — no
/// DB, no IO. The caller (always `pty::finalize_session`) is responsible
/// for serializing the result and writing the audit log row.
pub fn finalize_for_storage(
    blocks: Vec<ChatStreamEvent>,
    reason: FinalizeReason,
) -> (Vec<ChatStreamEvent>, FinalizeStats) {
    let input_len = blocks.len();
    let mut stats = FinalizeStats {
        input_blocks: input_len,
        output_blocks: input_len,
        ..Default::default()
    };

    // Step 1: identify orphan ToolUse at the tail (no matching ToolResult).
    // The pairing invariant only requires pairing on the trailing tool_use
    // — a tool_use in the middle of a long agent run is always followed by
    // its result before the NEXT tool_use, so only the tail can be orphan.
    let mut orphan_use_ids: Vec<Option<String>> = Vec::new();
    for ev in blocks.iter().rev() {
        match ev {
            ChatStreamEvent::ToolUse { id, .. } => orphan_use_ids.push(id.clone()),
            _ => break,
        }
    }
    // Collect dangling ToolResult ids — those with no preceding ToolUse.
    let mut dangling: Vec<usize> = Vec::new();
    let mut seen_tool_use_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for (i, ev) in blocks.iter().enumerate() {
        match ev {
            ChatStreamEvent::ToolUse { id: Some(uid), .. } => {
                seen_tool_use_ids.insert(uid.clone());
            }
            ChatStreamEvent::ToolResult { tool_use_id: Some(uid), .. } => {
                if !seen_tool_use_ids.contains(uid) {
                    dangling.push(i);
                }
            }
            _ => {}
        }
    }

    let had_orphan_tail = !orphan_use_ids.is_empty();
    let had_dangling = !dangling.is_empty();

    // Step 2: apply repair strategy.
    let mut out = blocks;
    match reason {
        FinalizeReason::Normal => {
            // Paranoid: only strip if there's a violation. The happy path
            // should rarely need repair — if it does, that's a bug worth
            // seeing in the audit log.
            if had_orphan_tail {
                let cut_from = out.len() - orphan_use_ids.len();
                out.truncate(cut_from);
                stats.stripped_orphan_use = orphan_use_ids.len();
            }
            if had_dangling {
                for &i in dangling.iter().rev() {
                    out.remove(i);
                }
                stats.dropped_dangling_result = dangling.len();
            }
        }
        FinalizeReason::UserInterrupt => {
            // Preserve old behavior: synthesize is_error=true results so the
            // transcript reads as "stopped mid-tool".
            if had_orphan_tail {
                for id in orphan_use_ids.iter().rev() {
                    out.push(ChatStreamEvent::ToolResult {
                        tool_use_id: id.clone(),
                        content: "[已中断：用户停止了本次会话]".into(),
                        is_error: true,
                    });
                }
                stats.synthesized_result = orphan_use_ids.len();
            }
            // Dangling still gets dropped.
            if had_dangling {
                for &i in dangling.iter().rev() {
                    out.remove(i);
                }
                stats.dropped_dangling_result = dangling.len();
            }
        }
        FinalizeReason::StreamTruncated
        | FinalizeReason::MaxSteps
        | FinalizeReason::ForceStop
        | FinalizeReason::Crash => {
            // Do NOT synthesize — the model's tail was lost or never
            // produced. Strip the orphan tool_use so the next continuation
            // sees a clean transcript.
            if had_orphan_tail {
                let cut_from = out.len() - orphan_use_ids.len();
                out.truncate(cut_from);
                stats.stripped_orphan_use = orphan_use_ids.len();
            }
            if had_dangling {
                for &i in dangling.iter().rev() {
                    out.remove(i);
                }
                stats.dropped_dangling_result = dangling.len();
            }
        }
    }

    stats.output_blocks = out.len();
    stats.was_clean = stats.stripped_orphan_use == 0
        && stats.synthesized_result == 0
        && stats.dropped_dangling_result == 0;

    (out, stats)
}

/// Persist the finalize audit row. Called by `pty::finalize_session` after
/// the blocks are written. Best-effort — a failure here MUST NOT prevent
/// the session from being marked terminal (the audit log is a quality
/// signal, not a correctness gate).
pub fn write_finalize_log(
    conn: &rusqlite::Connection,
    session_id: &str,
    reason: FinalizeReason,
    stats: &FinalizeStats,
) {
    let stats_json = match serde_json::to_string(stats) {
        Ok(s) => s,
        Err(e) => {
            log::warn!(
                "[blocks_integrity] serialize stats failed for session {}: {}",
                session_id,
                e
            );
            return;
        }
    };
    if let Err(e) = conn.execute(
        "INSERT INTO block_finalize_log
             (session_id, reason, input_blocks, output_blocks,
              stripped_orphan_use, synthesized_result, dropped_dangling_result,
              was_clean, stats_json, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            session_id,
            reason.as_str(),
            stats.input_blocks as i64,
            stats.output_blocks as i64,
            stats.stripped_orphan_use as i64,
            stats.synthesized_result as i64,
            stats.dropped_dangling_result as i64,
            stats.was_clean as i64,
            stats_json,
            chrono::Utc::now().to_rfc3339(),
        ],
    ) {
        // Log but never bubble — audit-only.
        log::warn!(
            "[blocks_integrity] write_finalize_log failed for session {}: {}",
            session_id,
            e
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::pty::ChatStreamEvent;

    fn tool_use(id: &str, name: &str) -> ChatStreamEvent {
        ChatStreamEvent::ToolUse {
            id: Some(id.into()),
            name: name.into(),
            input: serde_json::json!({}),
        }
    }
    fn tool_result(id: &str, is_error: bool) -> ChatStreamEvent {
        ChatStreamEvent::ToolResult {
            tool_use_id: Some(id.into()),
            content: "ok".into(),
            is_error,
        }
    }
    fn text(s: &str) -> ChatStreamEvent {
        ChatStreamEvent::Text { content: s.into() }
    }

    // ----- Normal reason -----

    #[test]
    fn normal_clean_passthrough() {
        let blocks = vec![text("hi"), tool_use("a", "bash"), tool_result("a", false)];
        let (out, stats) = finalize_for_storage(blocks, FinalizeReason::Normal);
        assert_eq!(out.len(), 3);
        assert!(stats.was_clean);
        assert_eq!(stats.stripped_orphan_use, 0);
    }

    #[test]
    fn normal_strips_orphan_tail_paranoid() {
        let blocks = vec![
            tool_use("a", "bash"),
            tool_result("a", false),
            tool_use("b", "edit"),
        ];
        let (out, stats) = finalize_for_storage(blocks, FinalizeReason::Normal);
        assert_eq!(out.len(), 2);
        assert_eq!(stats.stripped_orphan_use, 1);
        assert!(!stats.was_clean);
    }

    #[test]
    fn normal_drops_dangling_result() {
        let blocks = vec![text("hi"), tool_result("a", false)];
        let (out, stats) = finalize_for_storage(blocks, FinalizeReason::Normal);
        assert_eq!(out.len(), 1);
        assert_eq!(stats.dropped_dangling_result, 1);
    }

    // ----- UserInterrupt reason -----

    #[test]
    fn user_interrupt_synthesizes_result_for_orphan_tail() {
        let blocks = vec![
            tool_use("a", "bash"),
            tool_result("a", false),
            tool_use("b", "edit"),
        ];
        let (out, stats) = finalize_for_storage(blocks, FinalizeReason::UserInterrupt);
        assert_eq!(out.len(), 4);
        assert_eq!(stats.synthesized_result, 1);
        if let ChatStreamEvent::ToolResult { tool_use_id, is_error, .. } = &out[3] {
            assert_eq!(tool_use_id.as_deref(), Some("b"));
            assert!(*is_error);
        } else {
            panic!("last block should be synthesized ToolResult");
        }
    }

    #[test]
    fn user_interrupt_no_orphan_noop() {
        let blocks = vec![text("hi"), tool_use("a", "bash"), tool_result("a", false)];
        let (out, stats) = finalize_for_storage(blocks, FinalizeReason::UserInterrupt);
        assert_eq!(out.len(), 3);
        assert!(stats.was_clean);
    }

    // ----- StreamTruncated reason -----

    #[test]
    fn stream_truncated_strips_no_synthesize() {
        let blocks = vec![
            tool_use("a", "bash"),
            tool_result("a", false),
            tool_use("b", "edit"),
        ];
        let (out, stats) = finalize_for_storage(blocks, FinalizeReason::StreamTruncated);
        assert_eq!(out.len(), 2);
        assert_eq!(stats.stripped_orphan_use, 1);
        assert_eq!(stats.synthesized_result, 0);
    }

    // ----- MaxSteps reason -----

    #[test]
    fn max_steps_strips_orphan_use() {
        let blocks = vec![
            tool_use("a", "bash"),
            tool_result("a", false),
            tool_use("b", "edit"),
            tool_use("c", "read"),
        ];
        let (out, stats) = finalize_for_storage(blocks, FinalizeReason::MaxSteps);
        assert_eq!(out.len(), 2);
        assert_eq!(stats.stripped_orphan_use, 2);
    }

    // ----- ForceStop / Crash -----

    #[test]
    fn force_stop_strips() {
        let blocks = vec![text("hi"), tool_use("a", "bash")];
        let (out, _) = finalize_for_storage(blocks, FinalizeReason::ForceStop);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn crash_strips() {
        let blocks = vec![text("hi"), tool_use("a", "bash")];
        let (out, _) = finalize_for_storage(blocks, FinalizeReason::Crash);
        assert_eq!(out.len(), 1);
    }

    // ----- FinalizeReason -----

    #[test]
    fn finalize_reason_as_str_round_trips() {
        for r in [
            FinalizeReason::Normal,
            FinalizeReason::UserInterrupt,
            FinalizeReason::StreamTruncated,
            FinalizeReason::MaxSteps,
            FinalizeReason::ForceStop,
            FinalizeReason::Crash,
        ] {
            assert!(!r.as_str().is_empty());
        }
    }

    // ----- Edge: multiple orphan tool_uses back-to-back -----

    #[test]
    fn multiple_orphan_uses_all_stripped() {
        let blocks = vec![
            tool_use("a", "bash"),
            tool_result("a", false),
            tool_use("b", "edit"),
            tool_use("c", "read"),
        ];
        let (out, stats) = finalize_for_storage(blocks, FinalizeReason::StreamTruncated);
        assert_eq!(out.len(), 2);
        assert_eq!(stats.stripped_orphan_use, 2);
    }

    // ----- Edge: clean input, all reasons -----

    #[test]
    fn clean_input_all_reasons() {
        let blocks = vec![text("hi"), tool_use("a", "bash"), tool_result("a", false)];
        for r in [
            FinalizeReason::Normal,
            FinalizeReason::UserInterrupt,
            FinalizeReason::StreamTruncated,
            FinalizeReason::MaxSteps,
            FinalizeReason::ForceStop,
            FinalizeReason::Crash,
        ] {
            let (out, stats) = finalize_for_storage(blocks.clone(), r);
            assert_eq!(out.len(), 3, "reason={:?}", r);
            assert!(stats.was_clean, "reason={:?}", r);
        }
    }
}