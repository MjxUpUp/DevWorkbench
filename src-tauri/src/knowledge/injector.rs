use crate::models::AgentType;

/// Maximum total characters for injected knowledge content (excluding prompt).
const MAX_INJECT_TOTAL_CHARS: usize = 1500;
/// Maximum characters for a single injected entry's content.
const MAX_INJECT_ENTRY_CHARS: usize = 300;
/// Maximum knowledge entries to inject.
const MAX_INJECT_ENTRIES: usize = 5;
/// Maximum cross-project entries allowed (out of MAX_INJECT_ENTRIES).
const MAX_CROSS_PROJECT_ENTRIES: usize = 2;

/// Knowledge decay constants (days).
const DECAY_START_DAYS: i64 = 30;
const DECAY_END_DAYS: i64 = 90;

/// Inject knowledge context into an agent's prompt before spawning.
///
/// Uses FTS5 to find prompt-relevant entries, applies time-decay to confidence,
/// and supplements with cross-project knowledge when project-local entries are insufficient.
pub fn inject_for_agent(
    conn: &rusqlite::Connection,
    agent_type: &AgentType,
    project_path: &str,
    prompt: &str,
) -> String {
    let project_hash = crate::activity::hash_project_path(project_path);

    // Step 1: Extract keywords from prompt for FTS5 search
    let keywords = extract_keywords(prompt);

    // Step 2: Find relevant entries via FTS5 within this project
    let mut candidates: Vec<Candidate> = Vec::new();

    if !keywords.is_empty() {
        if let Ok(relevant) = super::store::search_entries_for_project(
            conn, &project_hash, &keywords, 0.5, 10,
        ) {
            for e in relevant {
                candidates.push(Candidate {
                    entry: e,
                    cross_project: false,
                    effective_confidence: 0.0, // computed below
                });
            }
        }
    }

    // Step 3: If FTS didn't find enough, supplement with all project entries
    if candidates.len() < MAX_INJECT_ENTRIES {
        let existing_ids: std::collections::HashSet<String> =
            candidates.iter().map(|c| c.entry.id.clone()).collect();

        if let Ok(all_entries) = super::store::get_entries_for_project(conn, &project_hash) {
            for e in all_entries.into_iter().filter(|e| e.confidence >= 0.5) {
                if existing_ids.contains(&e.id) {
                    continue;
                }
                candidates.push(Candidate {
                    entry: e,
                    cross_project: false,
                    effective_confidence: 0.0,
                });
            }
        }
    }

    // Step 4: Fill remaining slots with cross-project relevant entries
    let project_count = candidates.iter().filter(|c| !c.cross_project).count();
    if project_count < MAX_INJECT_ENTRIES && !keywords.is_empty() {
        let remaining = MAX_INJECT_ENTRIES.saturating_sub(project_count);
        let cross_limit = remaining.min(MAX_CROSS_PROJECT_ENTRIES);

        if let Ok(cross) = super::store::search_entries_cross_project(
            conn, &project_hash, &keywords, 0.6, cross_limit,
        ) {
            for e in cross {
                candidates.push(Candidate {
                    entry: e,
                    cross_project: true,
                    effective_confidence: 0.0,
                });
            }
        }
    }

    if candidates.is_empty() {
        return prompt.to_string();
    }

    // Step 5: Apply decay factor and compute effective confidence
    for c in &mut candidates {
        let decay = decay_factor(&c.entry.updated_at);
        c.effective_confidence = c.entry.confidence * decay;
    }

    // Filter out fully decayed entries (decay_factor returned 0.0)
    candidates.retain(|c| c.effective_confidence > 0.0);

    // Step 6: Sort by effective confidence DESC
    candidates.sort_by(|a, b| {
        b.effective_confidence
            .partial_cmp(&a.effective_confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Step 7: Select entries within char budget
    let mut selected = Vec::new();
    let mut total_chars = 0usize;
    for c in &candidates {
        let content_len = c.entry.content.len().min(MAX_INJECT_ENTRY_CHARS);
        if total_chars + content_len > MAX_INJECT_TOTAL_CHARS {
            break;
        }
        total_chars += content_len;
        selected.push(c);
        if selected.len() >= MAX_INJECT_ENTRIES {
            break;
        }
    }

    if selected.is_empty() {
        return prompt.to_string();
    }

    // Step 8: Truncate individual entry content
    let truncated: Vec<_> = selected
        .into_iter()
        .map(|c| {
            let mut entry = c.entry.clone();
            if entry.content.chars().count() > MAX_INJECT_ENTRY_CHARS {
                // Truncate by CHAR count, not byte length: String::truncate(n)
                // panics (is_char_boundary) when n lands inside a multibyte CJK
                // char. At 300 bytes that's ~2/3 of CJK content — the spawn-thread
                // panicked, rx.recv_timeout timed out, and knowledge injection
                // silently failed behind a "timed out" warn. store.rs:391 already
                // uses this chars().take() fix; this is the same bug's残留.
                entry.content = entry.content.chars().take(MAX_INJECT_ENTRY_CHARS).collect();
                entry.content.push_str("...");
            }
            TruncatedEntry {
                entry,
                cross_project: c.cross_project,
            }
        })
        .collect();

    let knowledge_block = format_knowledge_block(agent_type, &truncated);

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

struct Candidate {
    entry: crate::models::KnowledgeEntry,
    cross_project: bool,
    effective_confidence: f64,
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

/// Compute a time-based decay factor for a knowledge entry.
///
/// - Within `DECAY_START_DAYS` (30): returns 1.0 (no decay)
/// - After `DECAY_END_DAYS` (90): returns 0.0 (fully expired)
/// - Between: linear interpolation from 1.0 to 0.0
fn decay_factor(updated_at: &str) -> f64 {
    let updated = match chrono::DateTime::parse_from_rfc3339(updated_at) {
        Ok(dt) => dt.with_timezone(&chrono::Local),
        Err(_) => return 1.0, // unparseable → treat as recent
    };
    let now = chrono::Local::now();
    let age_days = (now - updated).num_days();

    if age_days <= DECAY_START_DAYS {
        1.0
    } else if age_days >= DECAY_END_DAYS {
        0.0
    } else {
        let range = (DECAY_END_DAYS - DECAY_START_DAYS) as f64;
        let elapsed = (age_days - DECAY_START_DAYS) as f64;
        1.0 - (elapsed / range)
    }
}

/// Extract search keywords from a prompt for FTS5 queries.
///
/// CJK characters are split individually (no word boundaries in Chinese).
/// English words are kept whole. Stop words are filtered. Returns an OR-joined
/// FTS5 query for broad recall.
fn extract_keywords(prompt: &str) -> String {
    const STOP_WORDS: &[&str] = &[
        "the", "is", "at", "which", "on", "a", "an", "and", "or", "but",
        "in", "with", "to", "for", "of", "not", "no", "this", "that", "it",
        "from", "by", "as", "be", "was", "are", "been", "has", "have", "had",
        "do", "does", "did", "will", "would", "can", "could", "should", "may",
        "if", "then", "so", "than", "too", "very", "just", "about", "up",
        "out", "all", "its", "my", "your", "our", "their", "what", "when",
        "where", "how", "who", "which",
        // Chinese stop words
        "的", "了", "是", "在", "有", "和", "不", "这", "我", "你",
        "他", "她", "它", "们", "吗", "呢", "吧", "啊", "把", "被",
        "让", "给", "到", "也", "都", "还", "就", "又", "而", "但",
        "个", "上", "下", "中", "里", "看", "说", "做", "会", "能",
        "要", "用", "去", "来", "过", "着", "得", "地",
    ];

    let mut keywords = Vec::new();
    let mut current_alpha = String::new();

    for ch in prompt.chars() {
        let cp = ch as u32;
        let is_cjk = (0x4E00..=0x9FFF).contains(&cp)      // CJK Unified Ideographs
            || (0x3400..=0x4DBF).contains(&cp)              // CJK Extension A
            || (0x3000..=0x303F).contains(&cp);             // CJK Symbols

        if is_cjk {
            // Flush accumulated alpha word
            if !current_alpha.is_empty() {
                keywords.push(current_alpha.clone());
                current_alpha.clear();
            }
            // Each CJK character is its own token
            keywords.push(ch.to_string());
        } else if ch.is_alphanumeric() {
            current_alpha.push(ch);
        } else {
            // Non-alphanumeric separator
            if !current_alpha.is_empty() {
                keywords.push(current_alpha.clone());
                current_alpha.clear();
            }
        }
    }
    if !current_alpha.is_empty() {
        keywords.push(current_alpha);
    }

    // Filter stop words and short tokens
    keywords.retain(|word| {
        let lower = word.to_lowercase();
        if STOP_WORDS.contains(&lower.as_str()) {
            return false;
        }
        // CJK single chars are valid keywords (already filtered stop words above)
        let is_cjk = word.chars().any(|c| {
            let cp = c as u32;
            (0x4E00..=0x9FFF).contains(&cp)
                || (0x3400..=0x4DBF).contains(&cp)
                || (0x3000..=0x303F).contains(&cp)
        });
        if is_cjk {
            return true;
        }
        word.len() >= 3
    });

    // Deduplicate
    let mut seen = std::collections::HashSet::new();
    keywords.retain(|w| seen.insert(w.to_lowercase()));

    // Take top 10, wrap each in quotes for safe FTS5 phrase matching
    keywords.truncate(10);
    if keywords.is_empty() {
        return String::new();
    }
    let quoted: Vec<String> = keywords
        .into_iter()
        .map(|w| format!("\"{}\"", w.replace('"', "")))
        .collect();
    quoted.join(" OR ")
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
        }
    }

    // --- extract_keywords tests ---

    #[test]
    fn test_extract_keywords_english() {
        let result = extract_keywords("Fix the authentication bug in the login flow");
        let lower = result.to_lowercase();
        // Check keywords present (split by " OR " to get exact tokens)
        assert!(lower.contains("fix"), "should contain 'fix': got '{}'", result);
        assert!(lower.contains("authentication"));
        assert!(lower.contains("bug"));
        assert!(lower.contains("login"));
        assert!(lower.contains("flow"));
        // "the" and "in" are stop words — check they are not standalone tokens
        let tokens: Vec<&str> = lower.split(" or ").collect();
        assert!(!tokens.contains(&"the"), "stop word 'the' should be filtered");
        assert!(!tokens.contains(&"in"), "stop word 'in' should be filtered");
    }

    #[test]
    fn test_extract_keywords_chinese() {
        let result = extract_keywords("看下当前项目是在哪个分支");
        // CJK chars are individual tokens, joined by OR
        assert!(result.contains("当"), "should contain '当': got '{}'", result);
        assert!(result.contains("项"), "should contain '项': got '{}'", result);
        assert!(result.contains("目"), "should contain '目': got '{}'", result);
        assert!(result.contains("分"), "should contain '分': got '{}'", result);
        assert!(result.contains("支"), "should contain '支': got '{}'", result);
        // Stop words should be filtered
        assert!(!result.contains("是"), "stop word '是' should be filtered: got '{}'", result);
        assert!(!result.contains("在"), "stop word '在' should be filtered: got '{}'", result);
    }

    #[test]
    fn test_extract_keywords_mixed() {
        let result = extract_keywords("用 Rust 实现一个 HTTP server");
        // CJK chars are individual tokens, English words kept whole
        assert!(result.contains("Rust"));
        assert!(result.contains("实"), "should contain '实': got '{}'", result);
        assert!(result.contains("现"), "should contain '现': got '{}'", result);
        assert!(result.contains("HTTP"));
        assert!(result.contains("server"));
    }

    #[test]
    fn test_extract_keywords_empty() {
        assert!(extract_keywords("").is_empty());
        assert!(extract_keywords("的 了 是").is_empty());
    }

    // --- decay_factor tests ---

    #[test]
    fn test_decay_factor_recent() {
        let now = chrono::Local::now().to_rfc3339();
        let factor = decay_factor(&now);
        assert!((factor - 1.0).abs() < 0.01, "Recent entry should have factor 1.0, got {}", factor);
    }

    #[test]
    fn test_decay_factor_mid_range() {
        // 60 days old → halfway between DECAY_START (30) and DECAY_END (90)
        let ts = (chrono::Local::now() - chrono::Duration::days(60)).to_rfc3339();
        let factor = decay_factor(&ts);
        assert!((factor - 0.5).abs() < 0.05, "60-day entry should have factor ~0.5, got {}", factor);
    }

    #[test]
    fn test_decay_factor_expired() {
        let ts = (chrono::Local::now() - chrono::Duration::days(100)).to_rfc3339();
        let factor = decay_factor(&ts);
        assert!((factor - 0.0).abs() < 0.01, "100-day entry should have factor 0.0, got {}", factor);
    }

    // --- inject_for_agent tests ---

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
}
