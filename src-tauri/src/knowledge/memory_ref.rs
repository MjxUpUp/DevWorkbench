//! `@memory:<title>` explicit memory reference (D3).
//!
//! Lets the user name a SPECIFIC memory to inject, as opposed to the implicit
//! FTS retrieval in [`super::retrieval::retrieve_relevant`]. Two forms:
//! - `@memory:word` — unquoted, title runs until the next whitespace (space-free
//!   titles; typical for CJK like `@memory:错误处理模式`).
//! - `@memory:"Title with spaces"` — quoted, title runs until the closing quote
//!   (required for English titles that contain spaces).
//!
//! Looks up the entry by title (case-insensitive, `status='active'`) via
//! [`super::store::get_entry_by_title_for_project`], and inlines its content
//! wrapped in a memory fence. Unknown titles leave a visible `[memory 'X' not
//! found]` marker so the user sees the ref was honored but unresolved — silent
//! dropping would hide typos.

use crate::knowledge::store::get_entry_by_title_for_project;

/// Marker prefix the user types to reference a memory by title.
const MEMORY_REF_PREFIX: &str = "@memory:";

/// Resolve all `@memory:<title>` references in `prompt` against the project's
/// active knowledge entries. Returns the prompt with each ref replaced by a
/// fenced content block (or a not-found marker). A DB error leaves the ref
/// untouched (best-effort — never blocks the prompt on a transient DB failure).
///
/// `@memory:` must be preceded by start-of-string or whitespace (same
/// anti-email rule as `@file` refs in `inject_file_references`), so an address
/// like `user@memory.com` is not matched.
pub fn resolve_memory_refs(
    prompt: &str,
    conn: &rusqlite::Connection,
    project_hash: &str,
) -> String {
    let chars: Vec<char> = prompt.chars().collect();
    let mut out = String::with_capacity(prompt.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '@' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // anti-email: @ must follow whitespace or be at start
        let at_start = i == 0 || chars[i - 1].is_whitespace();
        // try to match "memory:" literally right after @
        let tail_len = MEMORY_REF_PREFIX.len() - 1; // minus the leading @
        let rest: String = chars[i + 1..].iter().take(tail_len).collect();
        if at_start && rest == "memory:" {
            let title_start = i + MEMORY_REF_PREFIX.len();
            // Quoted form @memory:"..." vs unquoted @memory:word (until whitespace).
            let (title, title_end) = if chars.get(title_start) == Some(&'"') {
                let inner = title_start + 1;
                let mut e = inner;
                while e < chars.len() && chars[e] != '"' {
                    e += 1;
                }
                let t: String = chars[inner..e].iter().collect();
                // consume the closing quote if present
                let end = if e < chars.len() { e + 1 } else { e };
                (t, end)
            } else {
                let mut e = title_start;
                while e < chars.len() && !chars[e].is_whitespace() {
                    e += 1;
                }
                let t: String = chars[title_start..e].iter().collect();
                (t, e)
            };

            if title.is_empty() {
                // @memory: with no title — leave as-is (don't eat the bare token)
                let full: String = chars[i..title_end].iter().collect();
                out.push_str(&full);
                i = title_end;
                continue;
            }
            let replacement = match get_entry_by_title_for_project(conn, project_hash, &title) {
                Ok(Some(e)) => format!(
                    "--- BEGIN MEMORY: {} ---\n{}\n--- END MEMORY: {} ---",
                    e.title, e.content, e.title
                ),
                Ok(None) => format!("[memory '{}' not found]", title),
                Err(_) => {
                    // DB error — leave the ref untouched rather than dropping it
                    let full: String = chars[i..title_end].iter().collect();
                    out.push_str(&full);
                    i = title_end;
                    continue;
                }
            };
            out.push_str(&replacement);
            i = title_end;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
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

    fn mk_entry(id: &str, hash: &str, title: &str, content: &str, status: &str) -> KnowledgeEntry {
        KnowledgeEntry {
            id: id.to_string(),
            project_hash: hash.to_string(),
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
            status: status.to_string(),
            effectiveness: 0.0,
        }
    }

    #[test]
    fn test_resolve_known_memory_quoted() {
        let db = TempDb::new();
        crate::knowledge::store::add_entry(
            &db.conn,
            &mk_entry("k1", "proj", "Error handling", "Use thiserror for errors", "active"),
        )
        .unwrap();

        let out = resolve_memory_refs(
            "See @memory:\"Error handling\" for details",
            &db.conn,
            "proj",
        );
        assert!(out.contains("BEGIN MEMORY: Error handling"), "out = {out}");
        assert!(out.contains("Use thiserror for errors"), "out = {out}");
        assert!(!out.contains("@memory:"), "ref must be replaced: {out}");
    }

    #[test]
    fn test_resolve_known_memory_unquoted_cjk() {
        // Space-free CJK title works unquoted.
        let db = TempDb::new();
        crate::knowledge::store::add_entry(
            &db.conn,
            &mk_entry("k1", "proj", "错误处理模式", "用 thiserror 处理错误", "active"),
        )
        .unwrap();

        let out = resolve_memory_refs("参考 @memory:错误处理模式 详见", &db.conn, "proj");
        assert!(out.contains("用 thiserror 处理错误"), "out = {out}");
    }

    #[test]
    fn test_resolve_case_insensitive() {
        let db = TempDb::new();
        crate::knowledge::store::add_entry(
            &db.conn,
            &mk_entry("k1", "proj", "Build Tips", "use cargo nextest", "active"),
        )
        .unwrap();

        let out = resolve_memory_refs("@memory:\"build tips\" here", &db.conn, "proj");
        assert!(out.contains("use cargo nextest"), "case-insensitive match: {out}");
    }

    #[test]
    fn test_resolve_unknown_title_not_found_marker() {
        let db = TempDb::new();
        // unquoted → title stops at first whitespace → "nonexistent"
        let out = resolve_memory_refs("@memory:nonexistent stuff", &db.conn, "proj");
        assert!(out.contains("[memory 'nonexistent' not found]"), "out = {out}");
    }

    #[test]
    fn test_resolve_skips_superseded() {
        // I4: only status='active' is referenceable — superseded is not.
        let db = TempDb::new();
        crate::knowledge::store::add_entry(
            &db.conn,
            &mk_entry("k1", "proj", "Old tip", "stale content", "superseded"),
        )
        .unwrap();

        let out = resolve_memory_refs("@memory:\"Old tip\" please", &db.conn, "proj");
        assert!(
            out.contains("[memory 'Old tip' not found]"),
            "superseded must not resolve: {out}"
        );
    }

    #[test]
    fn test_resolve_anti_email() {
        // user@memory.com must NOT be treated as a memory ref.
        let db = TempDb::new();
        let out = resolve_memory_refs("contact user@memory.com please", &db.conn, "proj");
        assert!(out.contains("user@memory.com"), "email must be preserved: {out}");
        assert!(!out.contains("BEGIN MEMORY"), "email must not resolve: {out}");
    }

    #[test]
    fn test_resolve_no_refs_untouched() {
        let db = TempDb::new();
        let prompt = "Just a normal prompt with no refs";
        let out = resolve_memory_refs(prompt, &db.conn, "proj");
        assert_eq!(out, prompt);
    }

    #[test]
    fn test_resolve_multiple_refs() {
        let db = TempDb::new();
        crate::knowledge::store::add_entry(
            &db.conn,
            &mk_entry("k1", "proj", "Alpha", "first lesson", "active"),
        )
        .unwrap();
        crate::knowledge::store::add_entry(
            &db.conn,
            &mk_entry("k2", "proj", "Beta", "second lesson", "active"),
        )
        .unwrap();

        let out = resolve_memory_refs("@memory:Alpha and @memory:Beta", &db.conn, "proj");
        assert!(out.contains("first lesson"), "out = {out}");
        assert!(out.contains("second lesson"), "out = {out}");
    }
}
