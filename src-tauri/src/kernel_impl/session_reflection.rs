//! Post-completion reflection extractor (D6 + I2). The memory flywheel already
//! stores a completed session's NATURAL-LANGUAGE output (`react_session`); this
//! adds a STRUCTURED companion — which tools the agent reached for, which files
//! it touched, how many tool calls errored — distilled from the chat blocks the
//! run already produced, then optionally refined by an LLM.
//!
//! Two layers:
//! - [`summarize`] is the pure-rule floor (tool/file/error counts). It NEVER
//!   makes an LLM call and is always available — it's the fallback AND the
//!   context the LLM layer reasons over, so the loop degrades gracefully when
//!   the model is unreachable.
//! - [`summarize_with_llm`] (I2) feeds those rule stats + the session's prose
//!   to a one-shot `ChatModel` and asks for a reusable lesson in strict
//!   TITLE:/CONTENT: form. Falls back to the pure-rule result on any model error
//!   or unparseable response, so a flaky LLM never blocks the reflection write.

use crate::agents::pty::ChatStreamEvent;
use kernel_core::{ChatModel, Message, ModelOptions, ToolContext};
use std::collections::{BTreeMap, HashSet};

/// Distill a structured reflection (title, content) from a completed session's
/// chat blocks. Returns `None` when the run produced no reusable signal (no tool
/// use AND no file change) — a pure-chat turn shouldn't pollute the knowledge
/// base with an empty row.
pub fn summarize(blocks: &[ChatStreamEvent], prompt: &str) -> Option<(String, String)> {
    let mut tool_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut files: Vec<String> = Vec::new();
    let mut files_seen: HashSet<&str> = HashSet::new();
    let mut tool_errors = 0usize;

    for b in blocks {
        match b {
            ChatStreamEvent::ToolUse { name, .. } => {
                *tool_counts.entry(name.clone()).or_insert(0) += 1;
            }
            ChatStreamEvent::ToolResult { is_error, .. } => {
                if *is_error {
                    tool_errors += 1;
                }
            }
            ChatStreamEvent::FileChanged { path } if files_seen.insert(path.as_str()) => {
                files.push(path.clone());
            }
            _ => {}
        }
    }

    // No tools used and nothing changed → no behavioral signal worth a lesson.
    if tool_counts.is_empty() && files.is_empty() {
        return None;
    }

    let task_line: String = prompt
        .lines()
        .next()
        .unwrap_or(prompt)
        .chars()
        .take(80)
        .collect();
    let title = format!("Reflection: {}", task_line);

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("任务: {}", task_line));
    if !tool_counts.is_empty() {
        // Sort by count desc, then name — stable and most-used-first.
        let mut counts: Vec<(String, usize)> = tool_counts.into_iter().collect();
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let tools = counts
            .iter()
            .map(|(n, c)| format!("{}×{}", n, c))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("工具: {}", tools));
    }
    if !files.is_empty() {
        lines.push(format!("改动文件({}): {}", files.len(), files.join(", ")));
    }
    if tool_errors > 0 {
        lines.push(format!("工具失败: {} 次", tool_errors));
    }

    Some((title, lines.join("\n")))
}

/// LLM-enhanced reflection (I2). Builds on the pure-rule [`summarize`]:
/// - if `summarize` finds no behavioral signal (`None`) the LLM has nothing to
///   reflect on either, so this returns `None` too (a pure-chat turn still
///   writes no reflection row);
/// - otherwise feeds the rule stats + the session's natural-language summary to
///   `chat` as context and asks for a reusable lesson in strict `TITLE:` /
///   `CONTENT:` form.
///
/// Falls back to the pure-rule result on ANY failure (model error or unparseable
/// response), so a flaky or mis-formatted LLM never blocks the write. The pure
/// rule is the floor; the LLM only ever improves on it.
pub async fn summarize_with_llm(
    blocks: &[ChatStreamEvent],
    prompt: &str,
    summary: Option<&str>,
    chat: &dyn ChatModel,
) -> Option<(String, String)> {
    // Pure-rule stats are both the floor (fallback) and the context the LLM
    // reasons over. None → no behavioral signal at all → nothing to reflect on.
    let rule = summarize(blocks, prompt)?;
    let task_line: String = prompt
        .lines()
        .next()
        .unwrap_or(prompt)
        .chars()
        .take(120)
        .collect();
    let prose = summary.filter(|s| !s.is_empty()).unwrap_or("(无)");
    let sys = "你是代码 agent 的复盘助手。根据本次任务的执行轨迹，提炼可复用的经验教训。\
               只输出教训本身，不要复述统计数字。基于已知信息，不要编造没有的细节。";
    let user = format!(
        "任务: {task_line}\n\n\
         执行统计:\n{stats}\n\n\
         agent 自述:\n{prose}\n\n\
         请输出本次任务的可复用教训，严格两行格式:\n\
         TITLE: <一句话标题，≤80 字>\n\
         CONTENT: <3-6 行，含遇到的关键问题、解决方式、下次可复用的模式>",
        stats = rule.1
    );
    let messages = vec![Message::system(sys), Message::user(user)];
    let opts = ModelOptions {
        temperature: Some(0.3),
        ..Default::default()
    };
    match chat.generate(&messages, &opts).await {
        Ok(msg) => parse_reflection(&msg.content).or(Some(rule)),
        Err(_) => Some(rule),
    }
}

/// Parse the model's `TITLE:` / `CONTENT:` response into a (title, content)
/// pair. Returns `None` if either field is missing or empty so the caller falls
/// back to the pure-rule result.
fn parse_reflection(resp: &str) -> Option<(String, String)> {
    let title = resp
        .lines()
        .find_map(|l| l.strip_prefix("TITLE:").map(str::trim))?
        .to_string();
    let content = resp
        .find("CONTENT:")
        .map(|i| resp[i + "CONTENT:".len()..].trim().to_string())?;
    if title.is_empty() || content.is_empty() {
        return None;
    }
    Some((title, content))
}

/// Build the task prompt handed to the forked review sub-agent (I3). Names the
/// task, the files the run touched, the pure-rule stats, and the agent's own
/// prose — then asks for a strict `TITLE:` / `CONTENT:` lesson. The sub-agent
/// holds read-only tools (read_file/grep/glob) so it can verify the lesson
/// against the ACTUAL diff, not just the block metadata.
fn build_review_prompt(
    blocks: &[ChatStreamEvent],
    prompt: &str,
    summary: Option<&str>,
) -> String {
    let task_line: String = prompt
        .lines()
        .next()
        .unwrap_or(prompt)
        .chars()
        .take(120)
        .collect();
    let files: Vec<String> = blocks
        .iter()
        .filter_map(|b| match b {
            ChatStreamEvent::FileChanged { path } => Some(path.clone()),
            _ => None,
        })
        .collect();
    let stats = summarize(blocks, prompt)
        .map(|(_, c)| c)
        .unwrap_or_else(|| "(无工具使用)".to_string());
    let prose = summary.filter(|s| !s.is_empty()).unwrap_or("(无)");
    format!(
        "复盘以下已完成的编码任务,提炼一条可复用的 lesson。\n\n\
         任务: {task_line}\n\
         改动文件: {files}\n\
         执行统计:\n{stats}\n\n\
         agent 自述:\n{prose}\n\n\
         你只有只读工具(read_file/grep/glob),可以查看改动文件的实际内容来核实。\
         基于事实输出,不要编造。严格两行格式:\n\
         TITLE: <一句话标题,≤80 字>\n\
         CONTENT: <3-6 行,含关键问题、解决方式、下次可复用的模式>",
        files = if files.is_empty() {
            "(无)".to_string()
        } else {
            files.join(", ")
        },
    )
}

/// Parse the forked sub-agent's conclusion into a (title, content) lesson. The
/// dispatcher wraps its output as `[子 agent 结论] {out}{cost_line}`; strip that
/// wrapper + the trailing cost line, then reuse [`parse_reflection`] for the
/// `TITLE:` / `CONTENT:` pair. Returns `None` on the `[子 agent 失败: …]` shape
/// or any unparseable reply so the caller falls back to I2 / the pure rule.
fn parse_lessons(text: &str) -> Option<(String, String)> {
    let body = text
        .strip_prefix("[子 agent 结论]")
        .unwrap_or(text)
        .trim();
    // Drop the trailing cost line (📊 子 agent 用量: …) and anything after it.
    let body: String = body
        .lines()
        .take_while(|l| !l.starts_with("📊"))
        .collect::<Vec<_>>()
        .join("\n");
    parse_reflection(&body)
}

/// Fork a READ-ONLY sub-agent to extract a reusable lesson from a Completed
/// session (I3, the CCB `extractMemories` analogue). The child reuses the
/// parent's `ChatModel` handle and gets ONLY the read-only tool subset
/// (read_file/glob/grep) — it can investigate the actual diff but NOT mutate or
/// recurse (the dispatcher is itself non-read-only, so it's excluded, bounding
/// depth at 1). The child RETURNS text; THIS caller (the completion hook) writes
/// the DB, preserving the "子 agent 只读" invariant: a sub-agent never owns a DB
/// write. Best-effort: any failure (dispatch error, max-steps, unparseable
/// reply) returns `None` so the hook falls back to I2 / the pure rule.
pub async fn extract_lessons_via_subagent(
    chat: std::sync::Arc<dyn ChatModel>,
    working_dir: &str,
    session_id: &str,
    blocks: &[ChatStreamEvent],
    prompt: &str,
    summary: Option<&str>,
) -> Option<(String, String)> {
    use crate::kernel_impl::builtin_tools::{GlobTool, GrepTool, ReadFileTool};
    use crate::kernel_impl::react_agent::{SubAgentTool, ToolRegistry};
    use kernel_core::Tool;

    let task = build_review_prompt(blocks, prompt, summary);
    let mut reg = ToolRegistry::new();
    reg.push(ReadFileTool);
    reg.push(GlobTool);
    reg.push(GrepTool);
    let sub = SubAgentTool::new(chat, reg.read_only_subset(), 6, Vec::new());
    let ctx = ToolContext {
        working_dir: Some(working_dir.to_string()),
        conversation_id: Some(session_id.to_string()),
    };
    let args = serde_json::json!({ "task": task }).to_string();
    match sub.invoke(&args, &ctx).await {
        Ok(out) => parse_lessons(&out),
        Err(_) => None,
    }
}

/// Persist a Completed kernel-agent session's knowledge contributions in one
/// call — the natural-language `react_session` memory (what the agent SAID)
/// AND the structured `react_reflection` companion (what it DID). This is the
/// COMPLETE core of the `react_chat` completion hook (`commands/agents.rs`),
/// factored out so it is testable over a plain [`rusqlite::Connection`]: the
/// hook itself lives inline inside a `tokio::spawn`'d closure that holds a
/// Tauri `AppHandle` and drives a live `ReactAgent` stream, which can't be run
/// from `cargo test`. Extracting the logic means the part the hook actually
/// executes is covered by unit tests instead of only by the compiler.
///
/// Returns the count of entries written (0..=2). Best-effort and independent
/// per entry: a reflection still lands when prose is empty, and prose still
/// lands when the run used no tools. `summarize` returns `None` for a pure-chat
/// turn, so such a run writes only the `react_session` row (or nothing if it
/// also has no prose). The caller is responsible for the "only call this on
/// `SessionStatus::Completed`" guard — a Failed run must not pollute memory.
pub fn persist_completion_memory(
    conn: &rusqlite::Connection,
    project_hash: &str,
    session_id: &str,
    prompt: &str,
    summary: Option<&str>,
    final_blocks: &[ChatStreamEvent],
    agent_type: &crate::models::AgentType,
    reflection_override: Option<&(String, String)>,
) -> usize {
    let mut written = 0;
    // 1. Natural-language memory — only when there is prose to store.
    if let Some(out) = summary.filter(|s| !s.is_empty()) {
        let entry = crate::knowledge::store::build_session_memory_entry(
            project_hash,
            session_id,
            prompt,
            out,
            agent_type,
        );
        if crate::knowledge::store::add_entry(conn, &entry).is_ok() {
            written += 1;
        }
    }
    // 2. Structured reflection. An external LLM reflection (I2,
    //    reflection_override) wins when present; otherwise fall back to the
    //    pure-rule summarize, which is None for pure-chat so no empty row. The
    //    behavioral signal lives in the blocks even when prose is empty.
    let reflection = reflection_override
        .cloned()
        .or_else(|| summarize(final_blocks, prompt));
    if let Some((title, content)) = reflection {
        let entry = crate::knowledge::store::build_session_reflection_entry(
            project_hash,
            session_id,
            &title,
            &content,
            agent_type,
        );
        if crate::knowledge::store::add_entry(conn, &entry).is_ok() {
            written += 1;
        }
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tu(name: &str) -> ChatStreamEvent {
        ChatStreamEvent::ToolUse {
            id: None,
            name: name.into(),
            input: json!({}),
        }
    }
    fn tr(err: bool) -> ChatStreamEvent {
        ChatStreamEvent::ToolResult {
            tool_use_id: None,
            content: "x".into(),
            is_error: err,
        }
    }
    fn fc(path: &str) -> ChatStreamEvent {
        ChatStreamEvent::FileChanged { path: path.into() }
    }

    #[test]
    fn summarize_returns_none_for_pure_chat() {
        let blocks = vec![ChatStreamEvent::Text {
            content: "hi".into(),
        }];
        assert!(summarize(&blocks, "hello").is_none());
    }

    #[test]
    fn summarize_counts_tools_and_files_and_errors() {
        let blocks = vec![
            tu("read_file"),
            tu("read_file"),
            tu("write_file"),
            tr(false),
            tr(true),
            fc("src/a.rs"),
            fc("src/b.ts"),
            fc("src/a.rs"), // duplicate path → deduped
        ];
        let (title, content) = summarize(&blocks, "修复 bug\n第二行").unwrap();
        assert!(
            title.starts_with("Reflection: 修复 bug"),
            "title should lead with first prompt line: {}",
            title
        );
        assert!(content.contains("任务: 修复 bug"));
        // read_file used twice → ×2; sorted by count desc so it leads.
        assert!(content.contains("read_file×2"), "content: {}", content);
        assert!(content.contains("write_file×1"));
        // Duplicate src/a.rs collapsed → 2 distinct files.
        assert!(
            content.contains("改动文件(2): src/a.rs, src/b.ts"),
            "dedup must keep 2 distinct: {}",
            content
        );
        assert!(content.contains("工具失败: 1 次"));
    }

    #[test]
    fn summarize_files_only_no_tools_still_some() {
        // A run that only surfaced file changes (no tool_use blocks) still
        // carries signal → Some, with no tool line.
        let blocks = vec![fc("x.txt")];
        let (_title, content) = summarize(&blocks, "t").unwrap();
        assert!(
            !content.contains("工具:"),
            "no tool line when no tools: {}",
            content
        );
        assert!(content.contains("改动文件(1): x.txt"));
    }

    #[test]
    fn summarize_truncates_long_prompt_to_title_budget() {
        let long = "x".repeat(200);
        let blocks = vec![tu("bash")];
        let (title, _) = summarize(&blocks, &long).unwrap();
        assert!(title.chars().count() <= "Reflection: ".chars().count() + 80);
    }

    #[test]
    fn reflection_flows_blocks_through_to_db_entry() {
        // Higher-fidelity than the summarize unit tests above: exercise the FULL
        // data path the completion hook walks (summarize → build_session_reflection
        // _entry → add_entry → get_entries_for_project) so we know a real
        // Completed session's blocks land as one QUERYABLE react_reflection row —
        // not just that summarize formats a string in isolation.
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::init_db(&tmp.path().join("r.db")).unwrap();
        let hash = crate::activity::hash_project_path("/proj");

        let blocks = vec![
            tu("write_file"),
            tu("read_file"),
            tr(false),
            fc("src/a.rs"),
            ChatStreamEvent::Text {
                content: "done".into(),
            },
        ];

        let (title, content) = summarize(&blocks, "改 a.rs 的 bug").unwrap();
        let entry = crate::knowledge::store::build_session_reflection_entry(
            &hash,
            "sid1",
            &title,
            &content,
            &crate::models::AgentType::ClaudeCode,
        );
        crate::knowledge::store::add_entry(&conn, &entry).unwrap();

        let got = crate::knowledge::store::get_entries_for_project(&conn, &hash).unwrap();
        let refl = got
            .iter()
            .find(|e| e.category == "react_reflection")
            .expect("react_reflection row must be written + queryable");
        assert_eq!(refl.source_session_id.as_deref(), Some("sid1"));
        assert!(
            refl.content.contains("write_file×1"),
            "tool counts in content: {}",
            refl.content
        );
        assert!(refl.content.contains("read_file×1"));
        assert!(refl.content.contains("src/a.rs"));
        // The pure-chat Text block carries no behavioral signal — must NOT leak
        // into the structured reflection.
        assert!(!refl.content.contains("done"));
    }

    // ---- persist_completion_memory: covers the completion hook's WRITE core ----
    //
    // The hook itself (commands/agents.rs) is inline in a tokio::spawn'd closure
    // holding a Tauri AppHandle + live ReactAgent stream — undriveable from
    // `cargo test`. These tests exercise the exact logic the hook delegates to,
    // over a real sqlite Connection, so the four summary×blocks combinations a
    // Completed session can hit are all pinned.

    fn fresh_conn() -> rusqlite::Connection {
        let tmp = tempfile::TempDir::new().unwrap();
        crate::db::init_db(&tmp.path().join("c.db")).unwrap()
    }
    fn cats(conn: &rusqlite::Connection, hash: &str) -> Vec<String> {
        crate::knowledge::store::get_entries_for_project(conn, hash)
            .unwrap()
            .into_iter()
            .map(|e| e.category)
            .collect()
    }

    #[test]
    fn persist_writes_both_session_and_reflection_when_prose_and_tools() {
        let conn = fresh_conn();
        let hash = crate::activity::hash_project_path("/p");
        let blocks = vec![tu("write_file"), tr(false), fc("src/a.rs")];

        let n = persist_completion_memory(
            &conn,
            &hash,
            "sid",
            "改 a.rs",
            Some("done, edited a.rs"),
            &blocks,
            &crate::models::AgentType::ClaudeCode,
            None,
        );

        assert_eq!(n, 2, "prose + tools → both entries");
        let mut cats = cats(&conn, &hash);
        cats.sort();
        assert_eq!(
            cats,
            vec!["react_reflection".to_string(), "react_session".to_string()]
        );
    }

    #[test]
    fn persist_writes_only_reflection_when_no_prose() {
        // No prose summary, but tools were used → reflection still lands, no
        // empty react_session row.
        let conn = fresh_conn();
        let hash = crate::activity::hash_project_path("/p");
        let blocks = vec![tu("bash"), tr(false)];

        let n = persist_completion_memory(
            &conn,
            &hash,
            "sid",
            "task",
            None,
            &blocks,
            &crate::models::AgentType::ClaudeCode,
            None,
        );

        assert_eq!(n, 1);
        assert_eq!(cats(&conn, &hash), vec!["react_reflection".to_string()]);
    }

    #[test]
    fn persist_writes_only_session_when_pure_chat_with_prose() {
        // Pure-chat turn (Text only) with prose → summarize is None, so only the
        // react_session memory is written, no empty reflection.
        let conn = fresh_conn();
        let hash = crate::activity::hash_project_path("/p");
        let blocks = vec![ChatStreamEvent::Text {
            content: "just talking".into(),
        }];

        let n = persist_completion_memory(
            &conn,
            &hash,
            "sid",
            "hello",
            Some("hi there"),
            &blocks,
            &crate::models::AgentType::ClaudeCode,
            None,
        );

        assert_eq!(n, 1);
        assert_eq!(cats(&conn, &hash), vec!["react_session".to_string()]);
    }

    #[test]
    fn persist_writes_nothing_when_no_prose_and_no_signal() {
        // No prose AND a pure-chat turn → nothing at all. The DB stays empty.
        let conn = fresh_conn();
        let hash = crate::activity::hash_project_path("/p");
        let blocks = vec![ChatStreamEvent::Text {
            content: "x".into(),
        }];

        let n = persist_completion_memory(
            &conn,
            &hash,
            "sid",
            "q",
            None,
            &blocks,
            &crate::models::AgentType::ClaudeCode,
            None,
        );

        assert_eq!(n, 0);
        assert!(cats(&conn, &hash).is_empty());
    }

    // ---- summarize_with_llm (I2) ----
    use async_trait::async_trait;
    use kernel_core::{Error, MessageStream};

    /// One-shot stub: returns a canned reply from `generate`, errors on `stream`.
    struct ReflMock {
        reply: String,
    }
    impl ReflMock {
        fn new(s: &str) -> Self {
            Self {
                reply: s.to_string(),
            }
        }
    }
    #[async_trait]
    impl ChatModel for ReflMock {
        async fn generate(
            &self,
            _messages: &[Message],
            _opts: &ModelOptions,
        ) -> Result<Message, Error> {
            Ok(Message::assistant(self.reply.clone()))
        }
        fn stream(
            &self,
            _messages: &[Message],
            _opts: &ModelOptions,
        ) -> Result<MessageStream, Error> {
            Err(Error::Unsupported("unused by reflection tests".into()))
        }
    }

    #[tokio::test]
    async fn llm_reflection_parses_title_and_content() {
        let blocks = vec![tu("write_file"), fc("a.rs")];
        let mock = ReflMock::new(
            "TITLE: 修复 a.rs 的空指针\nCONTENT: 根因是 unwrap 了 None\n用 ? 传播错误\n下次先 grep unwrap",
        );
        let (title, content) = summarize_with_llm(&blocks, "修 bug", None, &mock)
            .await
            .expect("parsed reflection");
        assert_eq!(title, "修复 a.rs 的空指针");
        assert!(content.contains("用 ? 传播错误"), "content: {content}");
    }

    #[tokio::test]
    async fn llm_reflection_falls_back_when_unparseable() {
        // No TITLE:/CONTENT: → parse_reflection None → fallback to pure rule.
        let blocks = vec![tu("bash")];
        let mock = ReflMock::new("garbage with no format");
        let (title, content) = summarize_with_llm(&blocks, "t", None, &mock)
            .await
            .expect("fallback to rule");
        assert!(
            title.starts_with("Reflection: "),
            "fallback title should be pure-rule: {title}"
        );
        assert!(content.contains("任务:"), "fallback content: {content}");
    }

    #[tokio::test]
    async fn llm_reflection_none_when_pure_chat() {
        // No tool/file signal → summarize None → summarize_with_llm None too,
        // even though the mock WOULD reply. The LLM is never called.
        let blocks = vec![ChatStreamEvent::Text {
            content: "hi".into(),
        }];
        let mock = ReflMock::new("TITLE: x\nCONTENT: y");
        assert!(
            summarize_with_llm(&blocks, "hello", None, &mock)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn llm_reflection_uses_prose_summary_as_context() {
        // The session's prose is fed in; the model echoes it back so we can
        // confirm the wiring (not asserted on the rule stats).
        let blocks = vec![tu("read_file")];
        let mock = ReflMock::new("TITLE: t\nCONTENT: saw the agent note: did the thing");
        let (_title, content) =
            summarize_with_llm(&blocks, "task", Some("I did the thing"), &mock)
                .await
                .unwrap();
        assert!(content.contains("did the thing"), "content: {content}");
    }

    // ---- I3: forked subagent lesson extraction (pure helpers) ----
    //
    // extract_lessons_via_subagent itself drives a live ReactAgent run loop and
    // can't be exercised from `cargo test` without a scripted multi-turn model;
    // its two pure helpers ARE the testable surface (prompt shape + conclusion
    // parsing). SubAgentTool's own tests cover the read-only-subset invariant.

    #[test]
    fn build_review_prompt_lists_files_stats_and_prose() {
        let blocks = vec![tu("write_file"), fc("src/a.rs"), fc("src/b.ts")];
        let p = build_review_prompt(&blocks, "修 bug\n第二行", Some("done"));
        assert!(p.contains("任务: 修 bug"), "prompt: {p}");
        assert!(p.contains("src/a.rs") && p.contains("src/b.ts"), "files: {p}");
        assert!(p.contains("done"), "prose: {p}");
        assert!(p.contains("TITLE:"), "format ask: {p}");
    }

    #[test]
    fn build_review_prompt_handles_empty_prose_and_no_files() {
        let blocks = vec![tu("bash")];
        let p = build_review_prompt(&blocks, "t", None);
        assert!(p.contains("(无)"), "no prose → (无): {p}");
    }

    #[test]
    fn parse_lessons_strips_wrapper_and_cost_line() {
        let raw = "[子 agent 结论] TITLE: 修复空指针\nCONTENT: 用 ? 传播\n下次 grep unwrap\n📊 子 agent 用量: 10→20 tok · $0.01";
        let (title, content) = parse_lessons(raw).expect("parsed");
        assert_eq!(title, "修复空指针");
        assert!(content.contains("用 ? 传播"), "content: {content}");
        assert!(
            !content.contains("📊"),
            "cost line must be dropped: {content}"
        );
    }

    #[test]
    fn parse_lessons_none_on_failure_or_garbage() {
        assert!(parse_lessons("[子 agent 失败: timeout]").is_none());
        assert!(parse_lessons("garbage no format").is_none());
        assert!(parse_lessons("[子 agent 结论] no title here").is_none());
    }
}
