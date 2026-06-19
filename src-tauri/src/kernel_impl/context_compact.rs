//! Context auto-compaction (v1.3 C1) — the long-task context-overflow guard.
//!
//! Without this, ReactAgent's `history` grows unbounded: every turn appends an
//! assistant message + N tool results, so a 20-step run that reads a few files
//! blows the model's context window. The CLI path has the same disease
//! (`CONTEXT_BRIDGE_TOTAL_MAX_CHARS` hard-truncates at 8000 — it just drops the
//! oldest blocks, losing information).
//!
//! This module compresses the MIDDLE of the conversation into one summary
//! message, preserving:
//!   - the system prompt (index 0) verbatim,
//!   - the most recent `keep_recent` turns verbatim (so the model still sees the
//!     live tool results it's reacting to),
//!   - and a single LLM-generated summary of everything in between.
//!
//! Design choices:
//! - Summarization uses `ChatModel::generate` (blocking), NOT `stream`, and runs
//!   on the RAW model (no tools bound) so the summarizer can't fire tool calls.
//! - On any summarizer failure we SKIP this round (`Ok(false)`) and retry next
//!   turn — never silently truncate. Losing turns mid-run would drop information
//!   the agent may still need; a transient LLM error is preferable to data loss.
//! - Token estimate is a coarse chars/4 heuristic. It's intentionally cheap
//!   (called every turn) and errs toward compacting CJK-heavy history a touch
//!   early, which is safe.

use kernel_core::{ChatModel, Error, Message, ModelOptions, Role};

/// Rough token estimate for a history slice: ~4 chars per token, summing each
/// message's content + tool-call name/arguments. CJK overestimates slightly,
/// which compacts sooner — safe.
pub fn estimate_tokens(history: &[Message]) -> usize {
    let chars: usize = history
        .iter()
        .map(|m| {
            let base = m.content.chars().count();
            let calls: usize = m
                .tool_calls
                .iter()
                .map(|tc| tc.function.name.chars().count() + tc.function.arguments.chars().count())
                .sum();
            base + calls
        })
        .sum();
    chars / 4
}

/// Compress the middle of `history` into a single summary message.
///
/// Result layout: `[system] [summary(user)] ...last keep_recent messages...`.
/// The middle slice `history[1 .. len-keep_recent)` is fed to the summarizer.
///
/// Returns `Ok(None)` when there is nothing worth compacting (history already
/// short relative to `keep_recent`). Returns `Ok(Some(compacted))` on success.
/// Propagates the LLM error only if the caller wants it; `maybe_compact` wraps
/// this to swallow transient errors.
pub async fn summarize_middle(
    history: &[Message],
    model: &dyn ChatModel,
    opts: &ModelOptions,
    keep_recent: usize,
) -> Result<Option<Vec<Message>>, Error> {
    let len = history.len();
    // Need: system(1) + at least 2 middle turns + keep_recent tail. Anything
    // tighter leaves nothing meaningful to summarize.
    if len <= 1 + keep_recent + 2 {
        return Ok(None);
    }
    let summarize_end = len.saturating_sub(keep_recent);
    if summarize_end <= 1 {
        return Ok(None);
    }
    let middle = &history[1..summarize_end];
    if middle.is_empty() {
        return Ok(None);
    }

    // Render the middle turns as a flat transcript for the summarizer.
    let mut transcript = String::new();
    for m in middle {
        let role = match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        transcript.push_str(&format!("[{role}] {}\n", m.content));
        for tc in &m.tool_calls {
            transcript.push_str(&format!(
                "  (tool_call: {} {})\n",
                tc.function.name, tc.function.arguments
            ));
        }
    }

    let summary_request = vec![
        Message::system(
            "你是会话压缩器。把下面的 agent 执行历史压缩成一条简洁摘要：保留关键决策、已读文件、已做改动、未解决问题。\
             不要保留工具原始输出的冗长细节，只留结论。用中文，控制在 300 字以内。"
        ),
        Message::user(format!("历史记录：\n{transcript}\n\n请输出压缩摘要。")),
    ];

    // Summarize WITHOUT thinking (cheap, focused) regardless of the run's opts.
    let mut sum_opts = opts.clone();
    sum_opts.thinking = None;
    let summary_msg = model.generate(&summary_request, &sum_opts).await?;

    // Reassemble: keep system + the summary + the recent tail verbatim.
    let mut compacted = Vec::with_capacity(2 + keep_recent);
    compacted.push(history[0].clone());
    compacted.push(Message::user(format!(
        "[此前对话摘要]\n{}",
        summary_msg.content
    )));
    compacted.extend(history[summarize_end..].iter().cloned());
    Ok(Some(compacted))
}

/// Run-loop entry point. If `history` exceeds `max_tokens`, compress it in
/// place. Returns `true` iff compaction happened.
///
/// Summarizer errors are swallowed (logged) — see module docs for why we
/// prefer skip-over-truncate. Only a critical (non-LLM) bug would surface here.
pub async fn maybe_compact(
    history: &mut Vec<Message>,
    model: &dyn ChatModel,
    opts: &ModelOptions,
    max_tokens: usize,
    keep_recent: usize,
) -> Result<bool, Error> {
    if estimate_tokens(history) <= max_tokens {
        return Ok(false);
    }
    match summarize_middle(history, model, opts, keep_recent).await {
        Ok(Some(compacted)) => {
            *history = compacted;
            Ok(true)
        }
        Ok(None) => Ok(false),
        Err(e) => {
            log::warn!("[context-compact] summarization failed, skipping this round: {e}");
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use std::sync::{Arc, Mutex};

    /// Records every `generate` call's message slice and replies with a fixed
    /// string. `stream` is unused by the compaction path; returns Unsupported.
    struct SummaryChatModel {
        recorded: Arc<Mutex<Vec<Vec<Message>>>>,
        reply: String,
    }

    impl SummaryChatModel {
        fn new(reply: &str) -> Self {
            Self {
                recorded: Arc::new(Mutex::new(Vec::new())),
                reply: reply.to_string(),
            }
        }
        fn calls(&self) -> Vec<Vec<Message>> {
            self.recorded.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ChatModel for SummaryChatModel {
        async fn generate(
            &self,
            messages: &[Message],
            _opts: &ModelOptions,
        ) -> Result<Message, Error> {
            self.recorded.lock().unwrap().push(messages.to_vec());
            Ok(Message::assistant(self.reply.clone()))
        }
        fn stream(
            &self,
            _messages: &[Message],
            _opts: &ModelOptions,
        ) -> Result<BoxStream<'static, Result<Message, Error>>, Error> {
            Err(Error::Unsupported("unused by compaction tests".into()))
        }
    }

    fn msg(role: Role, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning: None,
            reasoning_signature: None,
        }
    }

    #[test]
    fn estimate_tokens_counts_content_and_tool_args() {
        let mut m = msg(Role::User, &"a".repeat(40)); // 40 chars -> 10 tokens
        m.tool_calls.push(kernel_core::ToolCall {
            id: "1".into(),
            call_type: "function".into(),
            function: kernel_core::FunctionCall {
                name: "grep".into(),
                arguments: "{}".into(), // name(4) + args(2) = 6 chars
            },
        });
        // total chars = 40 + 6 = 46 -> 46/4 = 11
        assert_eq!(estimate_tokens(&[m]), 11);
    }

    #[tokio::test]
    async fn summarize_middle_returns_none_when_history_short() {
        let model = SummaryChatModel::new("summary");
        // system + 2 middle + 6 keep_recent tail: len=9, need > 1+6+2=9 → None
        let mut hist = vec![msg(Role::System, "sys")];
        for i in 0..8 {
            hist.push(msg(Role::User, &format!("turn {i}")));
        }
        let out = summarize_middle(&hist, &model, &ModelOptions::default(), 6)
            .await
            .unwrap();
        assert!(out.is_none(), "too short to compact");
        assert!(
            model.calls().is_empty(),
            "no LLM call when nothing to compact"
        );
    }

    #[tokio::test]
    async fn summarize_middle_collapses_middle_keeps_system_and_tail() {
        let model = SummaryChatModel::new("这是摘要");
        // system + 5 middle + 3 tail = 9 messages, keep_recent=3
        let mut hist = vec![msg(Role::System, "sys-prompt")];
        for i in 0..5 {
            hist.push(msg(Role::User, &format!("middle {i}")));
        }
        // tail: last 3 (assistant + tool + user)
        hist.push(msg(Role::Assistant, "tail-a"));
        hist.push(msg(Role::Tool, "tail-tool"));
        hist.push(msg(Role::User, "tail-user"));
        let len_before = hist.len();

        let out = summarize_middle(&hist, &model, &ModelOptions::default(), 3)
            .await
            .unwrap()
            .expect("should compact");

        // system + summary + 3 tail = 5
        assert_eq!(out.len(), 5);
        assert_eq!(out[0].role, Role::System);
        assert_eq!(out[0].content, "sys-prompt");
        assert_eq!(out[1].role, Role::User);
        assert!(
            out[1].content.contains("这是摘要"),
            "summary message present"
        );
        // tail preserved verbatim in order
        assert_eq!(out[2].content, "tail-a");
        assert_eq!(out[3].content, "tail-tool");
        assert_eq!(out[4].content, "tail-user");
        assert!(out.len() < len_before);

        // Exactly one LLM call, and its transcript included the middle turns.
        let calls = model.calls();
        assert_eq!(calls.len(), 1);
        let transcript = &calls[0][1].content;
        assert!(transcript.contains("middle 0"));
        assert!(transcript.contains("middle 4"));
        assert!(
            !transcript.contains("tail-user"),
            "tail must NOT be fed to the summarizer"
        );
    }

    #[tokio::test]
    async fn summarize_middle_strips_thinking_from_summarizer_opts() {
        // If the run has thinking enabled, the summarizer call must NOT — it's a
        // cheap focused call. We assert via a model that inspects opts.
        struct OptSpyModel;
        #[async_trait]
        impl ChatModel for OptSpyModel {
            async fn generate(
                &self,
                _messages: &[Message],
                opts: &ModelOptions,
            ) -> Result<Message, Error> {
                assert!(
                    opts.thinking.is_none(),
                    "summarizer must run without thinking"
                );
                Ok(Message::assistant("sum"))
            }
            fn stream(
                &self,
                _m: &[Message],
                _o: &ModelOptions,
            ) -> Result<BoxStream<'static, Result<Message, Error>>, Error> {
                Err(Error::Unsupported("x".into()))
            }
        }
        let mut hist = vec![msg(Role::System, "sys")];
        for i in 0..6 {
            hist.push(msg(Role::User, &format!("m {i}")));
        }
        let opts = ModelOptions {
            thinking: Some(kernel_core::ThinkingConfig {
                budget_tokens: 1024,
            }),
            ..Default::default()
        };
        let out = summarize_middle(&hist, &OptSpyModel, &opts, 2)
            .await
            .unwrap();
        assert!(out.is_some());
    }

    #[tokio::test]
    async fn maybe_compact_skips_when_under_threshold() {
        let model = SummaryChatModel::new("s");
        let mut hist = vec![msg(Role::System, "sys"), msg(Role::User, "hi")];
        // tiny history, huge threshold → no compaction, no call
        let did = maybe_compact(&mut hist, &model, &ModelOptions::default(), 1_000_000, 3)
            .await
            .unwrap();
        assert!(!did);
        assert!(model.calls().is_empty());
        assert_eq!(hist.len(), 2);
    }

    #[tokio::test]
    async fn maybe_compact_compacts_when_over_threshold() {
        let model = SummaryChatModel::new("压缩结果");
        let mut hist = vec![msg(Role::System, "sys")];
        // ~20kb of content → ~5000 tokens, threshold 100 → over
        for i in 0..20 {
            hist.push(msg(Role::User, &format!("turn {i} ").repeat(50)));
        }
        let did = maybe_compact(&mut hist, &model, &ModelOptions::default(), 100, 4)
            .await
            .unwrap();
        assert!(did, "over-threshold history must compact");
        assert_eq!(hist.len(), 6, "system + summary + 4 tail");
        assert_eq!(model.calls().len(), 1);
    }
}
