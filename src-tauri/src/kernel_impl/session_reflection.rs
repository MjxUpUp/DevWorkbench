//! Post-completion reflection extractor (D6). The memory flywheel already stores
//! a completed session's NATURAL-LANGUAGE output (`react_session`); this adds a
//! STRUCTURED companion — which tools the agent reached for, which files it
//! touched, how many tool calls errored — distilled from the chat blocks the run
//! already produced. No extra LLM call: the signal is already in `final_blocks`,
//! we just surface it in a form FTS can retrieve and the next session's memory
//! suffix can reuse ("this project's agent tends to edit a.rs/b.ts and uses
//! read_file/write_file/bash").

use crate::agents::pty::ChatStreamEvent;
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
            ChatStreamEvent::FileChanged { path } => {
                if files_seen.insert(path.as_str()) {
                    files.push(path.clone());
                }
            }
            _ => {}
        }
    }

    // No tools used and nothing changed → no behavioral signal worth a lesson.
    if tool_counts.is_empty() && files.is_empty() {
        return None;
    }

    let task_line: String = prompt.lines().next().unwrap_or(prompt).chars().take(80).collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tu(name: &str) -> ChatStreamEvent {
        ChatStreamEvent::ToolUse { name: name.into(), input: json!({}) }
    }
    fn tr(err: bool) -> ChatStreamEvent {
        ChatStreamEvent::ToolResult { content: "x".into(), is_error: err }
    }
    fn fc(path: &str) -> ChatStreamEvent {
        ChatStreamEvent::FileChanged { path: path.into() }
    }

    #[test]
    fn summarize_returns_none_for_pure_chat() {
        let blocks = vec![ChatStreamEvent::Text { content: "hi".into() }];
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
        assert!(!content.contains("工具:"), "no tool line when no tools: {}", content);
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
            ChatStreamEvent::Text { content: "done".into() },
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
}
