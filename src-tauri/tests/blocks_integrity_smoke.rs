//! Smoke test for `agents::blocks_integrity::finalize_for_storage`.
//!
//! Why this file exists as an integration test instead of a unit test in
//! `blocks_integrity.rs`: the dev-workbench binary on Windows has a
//! pre-existing 0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND crash when running
//! its own unit tests (memory `applib-test-binary-entrypoint-block.md`).
//! Integration tests build a separate binary, sidestepping that issue, and
//! still link against the lib so they exercise the real production code.

use app_lib::agents::blocks_integrity::{finalize_for_storage, FinalizeReason};
use app_lib::agents::pty::ChatStreamEvent;

fn tool_use(id: &str) -> ChatStreamEvent {
    ChatStreamEvent::ToolUse {
        id: Some(id.into()),
        name: "bash".into(),
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

#[test]
fn clean_input_passes_through_all_reasons() {
    let blocks = vec![text("hi"), tool_use("a"), tool_result("a", false)];
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

#[test]
fn stream_truncated_strips_orphan_tail_no_synthesize() {
    let blocks = vec![tool_use("a"), tool_result("a", false), tool_use("b")];
    let (out, stats) = finalize_for_storage(blocks, FinalizeReason::StreamTruncated);
    assert_eq!(out.len(), 2);
    assert_eq!(stats.stripped_orphan_use, 1);
    assert_eq!(stats.synthesized_result, 0);
    assert!(!stats.was_clean);
}

#[test]
fn user_interrupt_synthesizes_error_result() {
    let blocks = vec![tool_use("a"), tool_result("a", false), tool_use("b")];
    let (out, stats) = finalize_for_storage(blocks, FinalizeReason::UserInterrupt);
    assert_eq!(out.len(), 4);
    assert_eq!(stats.synthesized_result, 1);
    if let ChatStreamEvent::ToolResult { tool_use_id, is_error, .. } = &out[3] {
        assert_eq!(tool_use_id.as_deref(), Some("b"));
        assert!(*is_error);
    } else {
        panic!("last block should be synthesized ToolResult, got: {:?}", out[3]);
    }
}

#[test]
fn normal_reason_strips_orphan_paranoid() {
    let blocks = vec![tool_use("a"), tool_result("a", false), tool_use("b")];
    let (out, stats) = finalize_for_storage(blocks, FinalizeReason::Normal);
    assert_eq!(out.len(), 2);
    assert_eq!(stats.stripped_orphan_use, 1);
}

#[test]
fn multiple_trailing_orphans_all_stripped() {
    let blocks = vec![
        tool_use("a"),
        tool_result("a", false),
        tool_use("b"),
        tool_use("c"),
    ];
    let (out, stats) = finalize_for_storage(blocks, FinalizeReason::MaxSteps);
    assert_eq!(out.len(), 2);
    assert_eq!(stats.stripped_orphan_use, 2);
}

#[test]
fn max_steps_strips_no_synthesize() {
    let blocks = vec![tool_use("a"), tool_use("b")];
    let (out, stats) = finalize_for_storage(blocks, FinalizeReason::MaxSteps);
    assert_eq!(out.len(), 0);
    assert_eq!(stats.stripped_orphan_use, 2);
    assert_eq!(stats.synthesized_result, 0);
}

#[test]
fn dangling_result_dropped() {
    let blocks = vec![text("hi"), tool_result("orphan_id", false)];
    let (out, stats) = finalize_for_storage(blocks, FinalizeReason::Normal);
    assert_eq!(out.len(), 1);
    assert_eq!(stats.dropped_dangling_result, 1);
}

#[test]
fn user_interrupt_with_dangling_drops_dangling_synthesizes_tail() {
    // UserInterrupt with BOTH a dangling tool_result AND a trailing orphan
    // tool_use: dangling gets dropped, tail gets synthesized. This was the
    // missing branch in the smoke matrix (review m4).
    let blocks = vec![
        text("hi"),
        tool_result("never_declared", false), // dangling — no prior ToolUse
        tool_use("a"),
    ];
    let (out, stats) = finalize_for_storage(blocks, FinalizeReason::UserInterrupt);
    // Drop index 1 (dangling) → [text, tool_use("a")], then push synthesized
    // result for "a" → [text, tool_use, synthesized_result].
    assert_eq!(out.len(), 3);
    assert_eq!(stats.dropped_dangling_result, 1);
    assert_eq!(stats.synthesized_result, 1);
    // Last block must be the synthesized error result for "a".
    if let ChatStreamEvent::ToolResult { tool_use_id, is_error, .. } = &out[2] {
        assert_eq!(tool_use_id.as_deref(), Some("a"));
        assert!(*is_error);
    } else {
        panic!("expected synthesized ToolResult at end, got {:?}", out[2]);
    }
}

#[test]
fn empty_blocks_all_reasons_clean_noop() {
    for r in [
        FinalizeReason::Normal,
        FinalizeReason::UserInterrupt,
        FinalizeReason::StreamTruncated,
        FinalizeReason::MaxSteps,
        FinalizeReason::ForceStop,
        FinalizeReason::Crash,
    ] {
        let (out, stats) = finalize_for_storage(vec![], r);
        assert_eq!(out.len(), 0, "reason={:?}", r);
        assert!(stats.was_clean, "reason={:?}", r);
        assert_eq!(stats.input_blocks, 0);
    }
}