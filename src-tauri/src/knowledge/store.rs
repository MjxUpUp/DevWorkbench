use crate::error::AppError;
use crate::models::{AgentType, KnowledgeEntry};
use rusqlite::params;

/// Add a knowledge entry to the database. Skips if a near-duplicate already exists
/// (same project_hash and matching first 200 chars of content).
pub fn add_entry(conn: &rusqlite::Connection, entry: &KnowledgeEntry) -> Result<(), AppError> {
    // Dedup check: match on project_hash + first 200 CHARS of content —
    // char-based, NOT byte slicing. It must be chars().take(200) for two reasons:
    //   1. SQLite's SUBSTR(content, 1, 200) counts CHARACTERS, so a byte prefix
    //      would compare against a different string and dedup would silently miss.
    //   2. A byte index of 200 lands inside a 3-byte CJK char (e.g. '的' at bytes
    //      198..201) → panic "byte index 200 is not a char boundary". This
    //      panicked add_entry on EVERY react_kernel completion (CJK output), so
    //      knowledge entries were never inserted and every completion logged a
    //      [PANIC]. chars().take(200) never panics and matches SUBSTR exactly.
    let content_prefix: String = entry.content.chars().take(200).collect();
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM knowledge_entries WHERE project_hash = ?1 AND SUBSTR(content, 1, 200) = ?2",
            params![entry.project_hash, content_prefix],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false);

    if exists {
        return Ok(());
    }

    conn.execute(
        "INSERT INTO knowledge_entries
            (id, project_hash, category, title, content, source_agent,
             source_session_id, source_type, confidence, created_at, updated_at, access_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            entry.id,
            entry.project_hash,
            entry.category,
            entry.title,
            entry.content,
            serde_json::to_string(&entry.source_agent)?.trim_matches('"'),
            entry.source_session_id,
            entry.source_type,
            entry.confidence,
            entry.created_at,
            entry.updated_at,
            entry.access_count,
        ],
    )?;
    // Keep FTS index in sync
    conn.execute(
        "INSERT INTO knowledge_fts (rowid, title, content) VALUES ((SELECT rowid FROM knowledge_entries WHERE id = ?1), ?2, ?3)",
        params![entry.id, entry.title, entry.content],
    )?;
    Ok(())
}

/// Build a `KnowledgeEntry` capturing a completed self-built ReactAgent
/// session's contribution to long-term memory (v1.3 T2). The opaque CLI path
/// feeds the knowledge flywheel via [`collect_from_session`] (it reads their
/// JSONL/sqlite logs); the kernel agent has no such log, so its completed
/// output is written directly as one `react_session` entry — closing the loop
/// so the NEXT session's `memory_prompt_suffix` can surface it.
///
/// `content` is capped at 1000 chars so a verbose run doesn't bloat the FTS
/// index or every future system prompt.
pub fn build_session_memory_entry(
    project_hash: &str,
    session_id: &str,
    title: &str,
    content: &str,
    agent_type: &AgentType,
) -> KnowledgeEntry {
    let title: String = title.lines().next().unwrap_or(title).chars().take(120).collect();
    let content: String = content.chars().take(1000).collect();
    KnowledgeEntry {
        id: uuid::Uuid::new_v4().to_string(),
        project_hash: project_hash.to_string(),
        category: "react_session".to_string(),
        title,
        content,
        source_agent: agent_type.clone(),
        source_session_id: Some(session_id.to_string()),
        source_type: "react_agent".to_string(),
        confidence: 0.6,
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        access_count: 0,
    }
}

/// Build a `react_reflection` KnowledgeEntry — the STRUCTURED companion to a
/// session's `react_session` natural-language memory (D6 reflection). Where
/// [`build_session_memory_entry`] stores what the agent SAID, this stores what
/// it DID (tool usage / files touched / errors) so the next session can match
/// on behavior patterns via FTS. `title`/`content` are pre-formatted by
/// `kernel_impl::session_reflection::summarize`; we only cap + tag here.
pub fn build_session_reflection_entry(
    project_hash: &str,
    session_id: &str,
    title: &str,
    content: &str,
    agent_type: &AgentType,
) -> KnowledgeEntry {
    KnowledgeEntry {
        id: uuid::Uuid::new_v4().to_string(),
        project_hash: project_hash.to_string(),
        category: "react_reflection".to_string(),
        title: title.chars().take(120).collect(),
        content: content.chars().take(1000).collect(),
        source_agent: agent_type.clone(),
        source_session_id: Some(session_id.to_string()),
        source_type: "react_agent".to_string(),
        confidence: 0.6,
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        access_count: 0,
    }
}

/// Search knowledge entries using FTS5 full-text search.
pub fn search_entries(
    conn: &rusqlite::Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<KnowledgeEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT ke.* FROM knowledge_entries ke
         WHERE ke.rowid IN (
            SELECT rowid FROM knowledge_fts WHERE knowledge_fts MATCH ?1
         )
         ORDER BY ke.updated_at DESC
         LIMIT ?2",
    )?;

    let entries = stmt.query_map(params![query, limit as i64], |row| row_to_entry(row))?;
    let mut result = Vec::new();
    for e in entries {
        result.push(e?);
    }
    Ok(result)
}

/// FTS5 search scoped to a single project, filtered by confidence, ranked by bm25 relevance.
pub fn search_entries_for_project(
    conn: &rusqlite::Connection,
    project_hash: &str,
    fts_query: &str,
    confidence_min: f64,
    limit: usize,
) -> Result<Vec<KnowledgeEntry>, AppError> {
    // Use subquery to avoid JOIN column ambiguity with FTS virtual table
    let mut stmt = conn.prepare(
        "SELECT * FROM knowledge_entries
         WHERE rowid IN (
            SELECT rowid FROM knowledge_fts WHERE knowledge_fts MATCH ?1
         )
         AND project_hash = ?2
         AND confidence >= ?3
         ORDER BY updated_at DESC
         LIMIT ?4",
    )?;

    let entries = stmt.query_map(
        params![fts_query, project_hash, confidence_min, limit as i64],
        |row| row_to_entry(row),
    )?;
    let mut result = Vec::new();
    for e in entries {
        result.push(e?);
    }
    Ok(result)
}

/// FTS5 search across all projects except the given one. Used for cross-project knowledge sharing.
pub fn search_entries_cross_project(
    conn: &rusqlite::Connection,
    exclude_project_hash: &str,
    fts_query: &str,
    confidence_min: f64,
    limit: usize,
) -> Result<Vec<KnowledgeEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT * FROM knowledge_entries
         WHERE rowid IN (
            SELECT rowid FROM knowledge_fts WHERE knowledge_fts MATCH ?1
         )
         AND project_hash != ?2
         AND confidence >= ?3
         ORDER BY updated_at DESC
         LIMIT ?4",
    )?;

    let entries = stmt.query_map(
        params![fts_query, exclude_project_hash, confidence_min, limit as i64],
        |row| row_to_entry(row),
    )?;
    let mut result = Vec::new();
    for e in entries {
        result.push(e?);
    }
    Ok(result)
}

/// Delete knowledge entries older than `max_age_days`. Also cleans up FTS rows.
/// Returns the number of deleted entries.
pub fn prune_old_entries(
    conn: &rusqlite::Connection,
    max_age_days: i64,
) -> Result<usize, AppError> {
    let cutoff = chrono::Local::now() - chrono::Duration::days(max_age_days);
    let cutoff_str = cutoff.to_rfc3339();

    // Wrap in transaction to keep FTS and main table consistent
    conn.execute_batch("BEGIN")?;

    let result = (|| -> Result<usize, AppError> {
        // Collect IDs to delete
        let ids: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT id FROM knowledge_entries WHERE updated_at < ?1",
            )?;
            let rows = stmt.query_map(params![cutoff_str], |row| row.get::<_, String>(0))?;
            let mut v = Vec::new();
            for r in rows {
                v.push(r?);
            }
            v
        };

        let count = ids.len();
        if count == 0 {
            return Ok(0);
        }

        // Delete FTS rows first (by rowid)
        for id in &ids {
            let rowid: Result<i64, _> = conn.query_row(
                "SELECT rowid FROM knowledge_entries WHERE id = ?1",
                params![id],
                |row| row.get(0),
            );
            if let Ok(rid) = rowid {
                conn.execute("DELETE FROM knowledge_fts WHERE rowid = ?1", params![rid])?;
            }
        }

        // Delete main entries
        for id in &ids {
            conn.execute("DELETE FROM knowledge_entries WHERE id = ?1", params![id])?;
        }

        log::info!("Knowledge prune: deleted {} entries older than {} days", count, max_age_days);
        Ok(count)
    })();

    match &result {
        Ok(_) => { let _ = conn.execute_batch("COMMIT"); }
        Err(_) => { let _ = conn.execute_batch("ROLLBACK"); }
    }
    result
}

/// Get all knowledge entries for a project.
pub fn get_entries_for_project(
    conn: &rusqlite::Connection,
    project_hash: &str,
) -> Result<Vec<KnowledgeEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT * FROM knowledge_entries WHERE project_hash = ?1 ORDER BY updated_at DESC",
    )?;

    let entries = stmt.query_map(params![project_hash], |row| row_to_entry(row))?;
    let mut result = Vec::new();
    for e in entries {
        result.push(e?);
    }
    Ok(result)
}

/// Delete a knowledge entry by ID.
pub fn delete_entry(conn: &rusqlite::Connection, id: &str) -> Result<(), AppError> {
    // Get rowid for FTS cleanup
    let rowid: i64 = conn.query_row(
        "SELECT rowid FROM knowledge_entries WHERE id = ?1",
        params![id],
        |row| row.get(0),
    ).map_err(|_| AppError::NotFound(format!("Knowledge entry {} 不存在", id)))?;

    conn.execute("DELETE FROM knowledge_entries WHERE id = ?1", params![id])?;
    conn.execute("DELETE FROM knowledge_fts WHERE rowid = ?1", params![rowid])?;
    Ok(())
}

/// Set the confidence of a knowledge entry and bump `updated_at` (D6 improvement
/// tracking). Resolved-but-not-accepted reviews decay their lessons' confidence
/// instead of deleting them, so the experience flywheel keeps a traceable record
/// of what was improved — purge (full exit) is reserved for accepted reviews.
/// Bumps `updated_at` so the decayed row sorts as recent in recency rankings.
pub fn set_entry_confidence(
    conn: &rusqlite::Connection,
    id: &str,
    confidence: f64,
) -> Result<(), AppError> {
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "UPDATE knowledge_entries SET confidence = ?1, updated_at = ?2 WHERE id = ?3",
        params![confidence, now, id],
    )?;
    Ok(())
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> Result<KnowledgeEntry, rusqlite::Error> {
    let agent_type_str: String = row.get(5)?;
    let agent_type: AgentType =
        serde_json::from_value(serde_json::Value::String(agent_type_str))
            .unwrap_or(AgentType::ClaudeCode);

    Ok(KnowledgeEntry {
        id: row.get(0)?,
        project_hash: row.get(1)?,
        category: row.get(2)?,
        title: row.get(3)?,
        content: row.get(4)?,
        source_agent: agent_type,
        source_session_id: row.get(6)?,
        source_type: row.get(7)?,
        confidence: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        access_count: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::AgentType;

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
            source_type: "auto_collect".to_string(),
            confidence: 0.8,
            created_at: chrono::Local::now().to_rfc3339(),
            updated_at: chrono::Local::now().to_rfc3339(),
            access_count: 0,
        }
    }

    #[test]
    fn test_add_and_search() {
        let db = TempDb::new();
        let e1 = make_entry("k1", "proj_a", "Rust error handling", "Use thiserror for error types in Rust");
        let e2 = make_entry("k2", "proj_a", "CSS variables", "Define CSS custom properties for theming");
        let e3 = make_entry("k3", "proj_b", "Tauri commands", "Use State for dependency injection");

        add_entry(&db.conn, &e1).unwrap();
        add_entry(&db.conn, &e2).unwrap();
        add_entry(&db.conn, &e3).unwrap();

        let results = search_entries(&db.conn, "Rust error", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "k1");
    }

    #[test]
    fn test_get_entries_for_project() {
        let db = TempDb::new();
        add_entry(&db.conn, &make_entry("k1", "proj_a", "Title 1", "Content 1")).unwrap();
        add_entry(&db.conn, &make_entry("k2", "proj_b", "Title 2", "Content 2")).unwrap();
        add_entry(&db.conn, &make_entry("k3", "proj_a", "Title 3", "Content 3")).unwrap();

        let proj_a = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(proj_a.len(), 2);
    }

    #[test]
    fn test_delete_entry() {
        let db = TempDb::new();
        add_entry(&db.conn, &make_entry("k1", "proj_a", "Title", "Content")).unwrap();
        delete_entry(&db.conn, "k1").unwrap();
        assert!(delete_entry(&db.conn, "k1").is_err());
    }

    #[test]
    fn test_dedup_same_content_skipped() {
        let db = TempDb::new();
        add_entry(&db.conn, &make_entry("k1", "proj_a", "Title", "Same content here that is long enough")).unwrap();
        // Same project + same content prefix → should be silently skipped
        add_entry(&db.conn, &make_entry("k2", "proj_a", "Title 2", "Same content here that is long enough")).unwrap();
        let entries = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(entries.len(), 1); // dedup: only first inserted
        assert_eq!(entries[0].id, "k1");
    }

    #[test]
    fn test_dedup_different_content_allowed() {
        let db = TempDb::new();
        add_entry(&db.conn, &make_entry("k1", "proj_a", "Title", "Content about Rust")).unwrap();
        add_entry(&db.conn, &make_entry("k2", "proj_a", "Title 2", "Content about Python")).unwrap();
        let entries = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_add_entry_multibyte_content_does_not_panic() {
        // Regression: the OLD `&content[..content.len().min(200)]` byte-sliced.
        // 300 CJK chars = 900 bytes, so byte index 200 lands mid-char (inside
        // '的' at bytes 198..201) → panic "byte index 200 is not a char boundary".
        // This fired on every react_kernel completion (CJK output). Char-based
        // truncation must never panic and must insert cleanly.
        let db = TempDb::new();
        let cjk = "的".repeat(300);
        add_entry(&db.conn, &make_entry("k1", "proj_a", "中文知识", &cjk)).unwrap();
        let entries = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].content.chars().count(), 300);
    }

    #[test]
    fn test_dedup_multibyte_uses_char_prefix_not_byte() {
        // Dedup must key on the first 200 CHARS (matching SQLite SUBSTR), not
        // 200 bytes. Two CJK entries that share their first 200 chars but diverge
        // afterward must dedup; two that differ within the first 200 chars must not.
        let db = TempDb::new();
        let shared = "知识".repeat(150); // 300 chars; first 200 chars identical below
        let mut same_prefix_a = shared.clone();
        same_prefix_a.push_str("尾巴甲"); // diverge AFTER the 200-char window
        let mut same_prefix_b = shared;
        same_prefix_b.push_str("尾巴乙");
        add_entry(&db.conn, &make_entry("k1", "proj_a", "T1", &same_prefix_a)).unwrap();
        add_entry(&db.conn, &make_entry("k2", "proj_a", "T2", &same_prefix_b)).unwrap();
        let entries = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(entries.len(), 1); // dedup: same first-200-char prefix
        assert_eq!(entries[0].id, "k1");
    }

    #[test]
    fn test_search_entries_for_project() {
        let db = TempDb::new();
        add_entry(&db.conn, &make_entry("k1", "proj_a", "Rust error handling", "Use thiserror for error types in Rust")).unwrap();
        add_entry(&db.conn, &make_entry("k2", "proj_a", "CSS theming", "Define CSS custom properties for dark mode")).unwrap();
        add_entry(&db.conn, &make_entry("k3", "proj_b", "Rust async", "Use tokio for async runtime in Rust")).unwrap();

        // Scoped to proj_a, search for "Rust" → should only get k1
        let results = search_entries_for_project(&db.conn, "proj_a", "Rust", 0.5, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "k1");
    }

    #[test]
    fn test_search_entries_cross_project() {
        let db = TempDb::new();
        add_entry(&db.conn, &make_entry("k1", "proj_a", "Rust error handling", "Use thiserror for error types in Rust")).unwrap();
        add_entry(&db.conn, &make_entry("k2", "proj_b", "Rust async runtime", "Use tokio for async Rust applications")).unwrap();
        add_entry(&db.conn, &make_entry("k3", "proj_b", "CSS theming", "Define CSS custom properties")).unwrap();

        // Exclude proj_a, search for "Rust" → should get k2 from proj_b only
        let results = search_entries_cross_project(&db.conn, "proj_a", "Rust", 0.5, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "k2");
    }

    #[test]
    fn test_prune_old_entries() {
        let db = TempDb::new();
        // Recent entry
        let mut recent = make_entry("k1", "proj_a", "Recent", "Fresh content here");
        recent.updated_at = chrono::Local::now().to_rfc3339();
        add_entry(&db.conn, &recent).unwrap();

        // Old entry (200 days ago)
        let mut old = make_entry("k2", "proj_a", "Old", "Stale content from long ago");
        old.updated_at = (chrono::Local::now() - chrono::Duration::days(200)).to_rfc3339();
        add_entry(&db.conn, &old).unwrap();

        let before = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(before.len(), 2);

        let pruned = prune_old_entries(&db.conn, 180).unwrap();
        assert_eq!(pruned, 1);

        let after = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, "k1");
    }

    #[test]
    fn set_entry_confidence_updates_confidence_and_bumps_updated_at() {
        // D6 improvement tracking: decaying a resolved lesson lowers confidence
        // (not deletes) and bumps updated_at so the row re-sorts as recent.
        let db = TempDb::new();
        let mut e = make_entry("k1", "proj_a", "no tests", "Forge 任务 x 评分 … no tests");
        e.confidence = 0.85;
        let stamp = "2020-01-01T00:00:00+08:00";
        e.updated_at = stamp.into();
        add_entry(&db.conn, &e).unwrap();

        set_entry_confidence(&db.conn, "k1", 0.425).unwrap();
        let got = get_entries_for_project(&db.conn, "proj_a").unwrap();
        assert_eq!(got.len(), 1);
        assert!((got[0].confidence - 0.425).abs() < 1e-6, "confidence updated: {got:?}");
        assert_ne!(got[0].updated_at, stamp, "updated_at bumped");
    }

    #[test]
    fn build_session_memory_entry_caps_content_and_tags_fields() {
        let e = build_session_memory_entry(
            "hash1", "sid1", "实现 auto-compact", &"x".repeat(2000), &AgentType::ClaudeCode,
        );
        assert_eq!(e.project_hash, "hash1");
        assert_eq!(e.category, "react_session");
        assert_eq!(e.source_type, "react_agent");
        assert_eq!(e.source_session_id.as_deref(), Some("sid1"));
        assert_eq!(e.source_agent, AgentType::ClaudeCode);
        assert!((e.confidence - 0.6).abs() < 1e-9);
        assert_eq!(e.content.chars().count(), 1000, "content capped at 1000 chars");
        assert_eq!(e.title, "实现 auto-compact");
        assert!(!e.id.is_empty());
    }

    #[test]
    fn build_session_memory_entry_takes_first_line_of_multiline_title() {
        let e = build_session_memory_entry(
            "h", "s", "第一行标题\n第二行不该出现", "内容", &AgentType::Codex,
        );
        assert_eq!(e.title, "第一行标题");
        assert!(!e.title.contains("第二行"));
        assert_eq!(e.source_agent, AgentType::Codex);
    }

    #[test]
    fn build_session_reflection_entry_tags_category_and_caps() {
        // D6 reflection builder: distinct category from react_session, same
        // confidence (0.6 → clears the memory-suffix threshold), title/content
        // capped so a noisy run can't bloat FTS.
        let e = build_session_reflection_entry(
            "hash1",
            "sid1",
            &"R".repeat(200),
            &"c".repeat(2000),
            &AgentType::ClaudeCode,
        );
        assert_eq!(e.category, "react_reflection", "distinct from react_session");
        assert_eq!(e.source_type, "react_agent");
        assert_eq!(e.source_session_id.as_deref(), Some("sid1"));
        assert!((e.confidence - 0.6).abs() < 1e-9);
        assert_eq!(e.content.chars().count(), 1000, "content capped at 1000");
        assert!(
            e.title.chars().count() <= 120,
            "title capped at 120: {}",
            e.title.chars().count()
        );
    }
}
