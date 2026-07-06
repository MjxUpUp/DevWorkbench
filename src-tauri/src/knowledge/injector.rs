use crate::models::AgentType;

/// Maximum total characters for injected knowledge content (excluding prompt).
const MAX_INJECT_TOTAL_CHARS: usize = 1500;
/// Maximum characters for a single injected entry's content.
const MAX_INJECT_ENTRY_CHARS: usize = 300;
/// Maximum knowledge entries to inject.
const MAX_INJECT_ENTRIES: usize = 5;

/// Inject knowledge context into an agent's prompt before spawning.
///
/// D2 unified retrieval: this opaque path now shares [`super::retrieval::retrieve_relevant`]
/// with the kernel path (`executor.rs memory_prompt_suffix`). FTS5 bm25 + decay +
/// `status='active'` + cross-project补全 all live in `retrieve_relevant`; this
/// function keeps only the opaque-specific bits — char-budget fill + agent-typed
/// rendering. Uses FTS5 to find prompt-relevant entries, applies time-decay, and
/// supplements with cross-project knowledge when project-local entries are
/// insufficient.
pub fn inject_for_agent(
    conn: &rusqlite::Connection,
    agent_type: &AgentType,
    project_path: &str,
    prompt: &str,
) -> String {
    let project_hash = crate::activity::hash_project_path(project_path);

    // is_continuation=false: opaque (PTY) agents have no ReactKernel history, so
    // the react_session isolation gate doesn't apply here.
    // exclude_categories empty: the opaque path has no separate experience lane,
    // so quality_failure lessons ARE injected here (the kernel path excludes them
    // because experience_prompt_suffix carries them separately).
    let candidates = super::retrieval::retrieve_relevant(
        conn,
        prompt,
        &project_hash,
        false,
        &[],
        0.5,
    );

    if candidates.is_empty() {
        return prompt.to_string();
    }

    // Char-budget greedy fill (opaque semantics: user-prompt prefix budget, NOT
    // the kernel's sys_prompt token budget). MAX_INJECT_* constants preserved.
    let mut selected: Vec<TruncatedEntry> = Vec::new();
    let mut total_chars = 0usize;
    for e in &candidates {
        let content_len = e.content.len().min(MAX_INJECT_ENTRY_CHARS);
        if total_chars + content_len > MAX_INJECT_TOTAL_CHARS {
            break;
        }
        total_chars += content_len;
        let mut entry = e.clone();
        // Truncate by CHAR count, not byte length: String::truncate(n) panics
        // (is_char_boundary) when n lands inside a multibyte CJK char. At 300
        // bytes that's ~2/3 of CJK content — the spawn-thread panicked,
        // rx.recv_timeout timed out, and knowledge injection silently failed
        // behind a "timed out" warn (store.rs:391 uses the same chars().take()).
        if entry.content.chars().count() > MAX_INJECT_ENTRY_CHARS {
            entry.content = entry.content.chars().take(MAX_INJECT_ENTRY_CHARS).collect();
            entry.content.push_str("...");
        }
        selected.push(TruncatedEntry {
            entry,
            // cross-project inferred from project_hash: retrieval returns the
            // entry's own hash, so comparing against the current project's hash
            // is the source of truth (no longer a flag threaded through Candidate).
            cross_project: e.project_hash != project_hash,
        });
        if selected.len() >= MAX_INJECT_ENTRIES {
            break;
        }
    }

    if selected.is_empty() {
        return prompt.to_string();
    }

    // I5: bump access_count for the entries actually injected, so the
    // effectiveness feedback loop (task6) can weight by reuse and access_count
    // is no longer a write-never field. Best-effort — a failed bump must not
    // block memory injection.
    let injected_ids: Vec<String> = selected.iter().map(|te| te.entry.id.clone()).collect();
    let _ = super::store::bump_access_counts(conn, &injected_ids);

    let knowledge_block = format_knowledge_block(agent_type, &selected);

    match agent_type {
        AgentType::ClaudeCode => {
            format!(
                "{}\n\n---\n\n## Project Knowledge Context\n\n{}\n\n---\n\n{}",
                knowledge_block.header,
                knowledge_block.entries.join("\n\n"),
                prompt,
            )
        }
        _ => {
            format!(
                "[Project Knowledge]\n{}\n\n[End Knowledge]\n\n{}",
                knowledge_block.entries.join("\n"),
                prompt,
            )
        }
    }
}

struct TruncatedEntry {
    entry: crate::models::KnowledgeEntry,
    cross_project: bool,
}

struct KnowledgeBlock {
    header: String,
    entries: Vec<String>,
}

fn format_knowledge_block(
    agent_type: &AgentType,
    entries: &[TruncatedEntry],
) -> KnowledgeBlock {
    let header = "Previously learned insights from this project:".to_string();

    let formatted: Vec<String> = entries
        .iter()
        .map(|te| {
            let cross_label = if te.cross_project { " [Cross-project]" } else { "" };
            match agent_type {
                AgentType::ClaudeCode => {
                    format!(
                        "### {}{} (from {}, confidence: {:.0}%)\n{}",
                        te.entry.title,
                        cross_label,
                        te.entry.source_agent.display_name(),
                        te.entry.confidence * 100.0,
                        te.entry.content,
                    )
                }
                _ => {
                    format!(
                        "- [{}] {}{} (from {})",
                        te.entry.category, te.entry.title, cross_label,
                        te.entry.source_agent.display_name()
                    )
                }
            }
        })
        .collect();

    KnowledgeBlock {
        header,
        entries: formatted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::knowledge::store;
    use crate::models::{AgentType, KnowledgeEntry};

    struct TempDb {
        _tmp: tempfile::TempDir,
        conn: rusqlite::Connection,
    }

    impl TempDb {
        fn new() -> Self {
            let tmp = tempfile::TempDir::new().unwrap();
            let db_path = tmp.path().join("test.db");
            let conn = db::init_db(&db_path).expect("init_db failed");
            Self { _tmp: tmp, conn }
        }
    }

    fn make_entry(id: &str, project_hash: &str, title: &str, content: &str) -> KnowledgeEntry {
        KnowledgeEntry {
            id: id.to_string(),
            project_hash: project_hash.to_string(),
            category: "insight".to_string(),
            title: title.to_string(),
            content: content.to_string(),
            source_agent: AgentType::ClaudeCode,
            source_session_id: None,
            source_type: "test".to_string(),
            confidence: 0.8,
            created_at: chrono::Local::now().to_rfc3339(),
            updated_at: chrono::Local::now().to_rfc3339(),
            access_count: 0,
            status: "active".to_string(),
            effectiveness: 0.0,
        }
    }

    // --- inject_for_agent tests ---
    // (extract_keywords / decay_factor unit tests moved to retrieval.rs alongside
    //  retrieve_relevant, which is now their only consumer.)

    #[test]
    fn test_inject_for_claude_code() {
        let db = TempDb::new();
        let hash = crate::activity::hash_project_path("/proj/a");
        store::add_entry(&db.conn, &make_entry("k1", &hash, "Rust tip", "Use thiserror for error handling")).unwrap();

        let result = inject_for_agent(
            &db.conn,
            &AgentType::ClaudeCode,
            "/proj/a",
            "Fix the Rust error bug",
        );

        assert!(result.contains("Project Knowledge Context"));
        assert!(result.contains("Rust tip"));
        assert!(result.contains("Fix the Rust error bug"));
    }

    #[test]
    fn test_inject_for_other_agent() {
        let db = TempDb::new();
        let hash = crate::activity::hash_project_path("/proj/a");
        store::add_entry(&db.conn, &make_entry("k1", &hash, "Tip", "Content")).unwrap();

        let result = inject_for_agent(
            &db.conn,
            &AgentType::Codex,
            "/proj/a",
            "Do the thing",
        );

        assert!(result.contains("[Project Knowledge]"));
        assert!(result.contains("Do the thing"));
    }

    #[test]
    fn test_no_injection_when_empty() {
        let db = TempDb::new();
        let result = inject_for_agent(
            &db.conn,
            &AgentType::ClaudeCode,
            "/proj/empty",
            "Just do it",
        );
        assert_eq!(result, "Just do it");
    }

    #[test]
    fn test_low_confidence_filtered() {
        let db = TempDb::new();
        let hash = crate::activity::hash_project_path("/proj/a");
        let mut entry = make_entry("k1", &hash, "Tool noise", "Some tool output content here");
        entry.confidence = 0.4;
        store::add_entry(&db.conn, &entry).unwrap();

        let result = inject_for_agent(
            &db.conn,
            &AgentType::ClaudeCode,
            "/proj/a",
            "Do work",
        );
        assert!(!result.contains("Tool noise"));
        assert_eq!(result, "Do work");
    }

    #[test]
    fn test_long_content_truncated() {
        let db = TempDb::new();
        let hash = crate::activity::hash_project_path("/proj/a");
        let long_content = "A".repeat(500);
        store::add_entry(&db.conn, &make_entry("k1", &hash, "Long entry", &long_content)).unwrap();

        let result = inject_for_agent(
            &db.conn,
            &AgentType::ClaudeCode,
            "/proj/a",
            "Prompt",
        );
        assert!(result.contains("Project Knowledge Context"));
        assert!(!result.contains(&"A".repeat(400)));
    }

    #[test]
    fn test_total_budget_enforced() {
        let db = TempDb::new();
        let hash = crate::activity::hash_project_path("/proj/a");
        for i in 0..6 {
            let content = format!("Entry number {} with enough text to be meaningful: {}", i, "x".repeat(350));
            store::add_entry(&db.conn, &make_entry(
                &format!("k{}", i), &hash, &format!("Title {}", i), &content,
            )).unwrap();
        }

        let result = inject_for_agent(
            &db.conn,
            &AgentType::ClaudeCode,
            "/proj/a",
            "Prompt",
        );
        assert!(result.contains("Project Knowledge Context"));
        let title_count = ["Title 0", "Title 1", "Title 2", "Title 3", "Title 4", "Title 5"]
            .iter()
            .filter(|t| result.contains(*t))
            .count();
        assert_eq!(title_count, 5);
    }

    #[test]
    fn test_fts_relevance_over_confidence() {
        let db = TempDb::new();
        let hash = crate::activity::hash_project_path("/proj/a");

        // High confidence but unrelated
        let mut unrelated = make_entry("k1", &hash, "CSS theming", "Define CSS custom properties for dark mode theming");
        unrelated.confidence = 0.9;
        store::add_entry(&db.conn, &unrelated).unwrap();

        // Lower confidence but relevant
        let mut relevant = make_entry("k2", &hash, "Rust error handling", "Use thiserror for custom error types in Rust");
        relevant.confidence = 0.7;
        store::add_entry(&db.conn, &relevant).unwrap();

        let result = inject_for_agent(
            &db.conn,
            &AgentType::ClaudeCode,
            "/proj/a",
            "Fix the Rust error handling bug",
        );

        // Both should be present, but Rust entry must appear (FTS matched it)
        assert!(result.contains("Rust error handling"));
    }

    #[test]
    fn test_cross_project_injection() {
        let db = TempDb::new();
        let hash_a = crate::activity::hash_project_path("/proj/a");
        let hash_b = crate::activity::hash_project_path("/proj/b");

        // proj_a has no relevant entries
        store::add_entry(&db.conn, &make_entry("k1", &hash_a, "CSS tip", "Use CSS variables for theming")).unwrap();

        // proj_b has relevant Rust entries
        store::add_entry(&db.conn, &make_entry("k2", &hash_b, "Rust error handling", "Use thiserror for Rust error types")).unwrap();

        let result = inject_for_agent(
            &db.conn,
            &AgentType::ClaudeCode,
            "/proj/a",
            "Fix the Rust error handling",
        );

        assert!(result.contains("Cross-project"));
        assert!(result.contains("Rust error handling"));
    }

    #[test]
    fn test_decay_excludes_old_entries() {
        let db = TempDb::new();
        let hash = crate::activity::hash_project_path("/proj/a");

        // Entry from 100 days ago (past DECAY_END_DAYS)
        let mut old = make_entry("k1", &hash, "Old Rust tip", "Use thiserror for Rust error handling");
        old.updated_at = (chrono::Local::now() - chrono::Duration::days(100)).to_rfc3339();
        store::add_entry(&db.conn, &old).unwrap();

        let result = inject_for_agent(
            &db.conn,
            &AgentType::ClaudeCode,
            "/proj/a",
            "Fix the Rust error",
        );

        // Old entry should be excluded due to decay
        assert!(!result.contains("Old Rust tip"));
    }

    #[test]
    fn test_inject_bumps_access_count() {
        // I5: injected entries must have access_count incremented, so the field
        // is no longer write-never and the effectiveness loop can weight by reuse.
        let db = TempDb::new();
        let hash = crate::activity::hash_project_path("/proj/a");
        store::add_entry(&db.conn, &make_entry("k1", &hash, "Rust tip", "Use thiserror for error handling")).unwrap();

        let _ = inject_for_agent(
            &db.conn,
            &AgentType::ClaudeCode,
            "/proj/a",
            "Fix the Rust error bug",
        );

        let entries = store::get_entries_for_project(&db.conn, &hash).unwrap();
        let injected = entries.iter().find(|e| e.id == "k1").unwrap();
        assert_eq!(
            injected.access_count, 1,
            "access_count must be bumped from 0 to 1 on injection"
        );
    }
}
