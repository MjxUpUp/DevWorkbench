use crate::activity::hash_project_path;
use crate::error::AppError;
use crate::models::{AgentType, KnowledgeEntry};
use std::path::Path;

/// Parse Claude Code JSONL files from ~/.claude/projects/{hash}/
/// Extracts user messages and assistant summaries as knowledge entries.
pub fn parse_claude_jsonl(
    project_path: &str,
    jsonl_path: &Path,
    session_id: Option<&str>,
) -> Result<Vec<KnowledgeEntry>, AppError> {
    let project_hash = hash_project_path(project_path);
    parse_claude_jsonl_inner(&project_hash, jsonl_path, session_id)
}

/// Parse Claude Code JSONL when only the project hash directory name is known
/// (used by the file watcher which can't reverse the hash).
pub fn parse_claude_jsonl_by_hash(
    project_hash: &str,
    jsonl_path: &Path,
) -> Result<Vec<KnowledgeEntry>, AppError> {
    parse_claude_jsonl_inner(project_hash, jsonl_path, None)
}

/// Maximum JSONL file size to process (5 MB). Larger files are skipped to
/// avoid memory pressure and UI freezes during post-session knowledge collection.
pub(crate) const MAX_JSONL_FILE_SIZE: u64 = 5 * 1024 * 1024;

fn parse_claude_jsonl_inner(
    project_hash: &str,
    jsonl_path: &Path,
    session_id: Option<&str>,
) -> Result<Vec<KnowledgeEntry>, AppError> {
    // Skip oversized JSONL files — reading 13MB+ transcripts into memory
    // blocks the DB lock and causes system-wide slowdown.
    let file_size = std::fs::metadata(jsonl_path)
        .map(|m| m.len())
        .unwrap_or(0);
    if file_size > MAX_JSONL_FILE_SIZE {
        log::info!(
            "Knowledge collector: 跳过 {} ({} MB, 超过 {} MB 限制)",
            jsonl_path.display(),
            file_size / (1024 * 1024),
            MAX_JSONL_FILE_SIZE / (1024 * 1024),
        );
        return Ok(vec![]);
    }

    let content = std::fs::read_to_string(jsonl_path)?;
    let mut entries = Vec::new();

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        // Extract assistant message blocks with tool results
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
        if role != "assistant" {
            continue;
        }

        let Some(content_arr) = msg.get("content").and_then(|c| c.as_array()) else {
            continue;
        };

        for block in content_arr {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");

            match block_type {
                "text" => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        let text = text.trim();
                        if text.len() < 20 || text.len() > 5000 {
                            continue;
                        }
                        if is_noise_content(text) {
                            continue;
                        }
                        let title = truncate_title(text, 80);
                        entries.push(KnowledgeEntry {
                            id: uuid::Uuid::new_v4().to_string(),
                            project_hash: project_hash.to_string(),
                            category: "agent_output".to_string(),
                            title,
                            content: text.to_string(),
                            source_agent: AgentType::ClaudeCode,
                            source_session_id: session_id.map(|s| s.to_string()),
                            source_type: "claude_jsonl".to_string(),
                            confidence: 0.7,
                            created_at: chrono::Local::now().to_rfc3339(),
                            updated_at: chrono::Local::now().to_rfc3339(),
                            access_count: 0,
                        });
                    }
                }
                "tool_result" => {
                    if let Some(content) = block.get("content").and_then(|c| c.as_str()) {
                        let content = content.trim();
                        if content.len() < 20 || content.len() > 5000 {
                            continue;
                        }
                        if is_noise_content(content) {
                            continue;
                        }
                        let tool_name = block
                            .get("tool_use_id")
                            .and_then(|t| t.as_str())
                            .unwrap_or("unknown");
                        let title = format!("Tool result: {}", truncate_title(content, 60));
                        entries.push(KnowledgeEntry {
                            id: uuid::Uuid::new_v4().to_string(),
                            project_hash: project_hash.to_string(),
                            category: "tool_output".to_string(),
                            title,
                            content: content.to_string(),
                            source_agent: AgentType::ClaudeCode,
                            source_session_id: session_id.map(|s| s.to_string()),
                            source_type: format!("claude_jsonl_tool_{}", tool_name),
                            confidence: 0.4,
                            created_at: chrono::Local::now().to_rfc3339(),
                            updated_at: chrono::Local::now().to_rfc3339(),
                            access_count: 0,
                        });
                    }
                }
                _ => {}
            }

            // Limit to 50 entries per file to avoid overwhelming
            if entries.len() >= 50 {
                break;
            }
        }
    }

    Ok(entries)
}

/// Parse DevWorkbench agent output logs.
pub fn parse_devworkbench_log(
    project_path: &str,
    log_path: &Path,
    session_id: &str,
    agent_type: &AgentType,
) -> Result<Vec<KnowledgeEntry>, AppError> {
    let project_hash = hash_project_path(project_path);
    let bytes = std::fs::read(log_path)?;
    let text = String::from_utf8_lossy(&bytes);
    let text = crate::utils::strip_ansi(&text);

    if text.trim().is_empty() || text.len() < 20 {
        return Ok(vec![]);
    }

    // Split into chunks of ~2000 chars at sentence boundaries
    let mut entries = Vec::new();
    let chunks = split_into_chunks(&text, 2000);

    for (i, chunk) in chunks.into_iter().enumerate() {
        let title = if i == 0 {
            format!("Session output: {}", truncate_title(&chunk, 60))
        } else {
            format!("Session output (part {}): {}", i + 1, truncate_title(&chunk, 40))
        };

        entries.push(KnowledgeEntry {
            id: uuid::Uuid::new_v4().to_string(),
            project_hash: project_hash.clone(),
            category: "session_output".to_string(),
            title,
            content: chunk,
            source_agent: agent_type.clone(),
            source_session_id: Some(session_id.to_string()),
            source_type: "devworkbench_log".to_string(),
            confidence: 0.5,
            created_at: chrono::Local::now().to_rfc3339(),
            updated_at: chrono::Local::now().to_rfc3339(),
            access_count: 0,
        });
    }

    Ok(entries)
}

/// Collect knowledge from a completed session's output log.
pub fn collect_from_session(
    conn: &rusqlite::Connection,
    project_path: &str,
    session_id: &str,
    agent_type: &AgentType,
) -> Result<usize, AppError> {
    let agents_dir = crate::agents::session::agents_dir()
        .map_err(|e| AppError::KnowledgeCollection {
            agent: agent_type.display_name().to_string(),
            reason: e,
        })?;

    let mut total = 0;

    // 1. Collect from DevWorkbench session output log
    let log_path = agents_dir.join("outputs").join(format!("{}.log", session_id));
    if log_path.exists() {
        let entries = parse_devworkbench_log(project_path, &log_path, session_id, agent_type)?;
        for entry in &entries {
            super::store::add_entry(conn, entry)?;
        }
        total += entries.len();
    }

    // 2. Collect from Claude Code JSONL files for this project (best-effort)
    if let Some(claude_dir) = super::watchers::claude_project_dir(project_path) {
        if let Ok(rd) = std::fs::read_dir(&claude_dir) {
            for file in rd.flatten() {
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    if let Ok(entries) = parse_claude_jsonl(project_path, &path, Some(session_id)) {
                        for entry in &entries {
                            let _ = super::store::add_entry(conn, entry);
                        }
                        total += entries.len();
                    }
                }
            }
        }
    }

    // 3. Collect from Codex SQLite databases (best-effort)
    {
        let home = crate::commands::projects::dirs_home();
        let codex_dir = home.join(".codex");
        if codex_dir.exists() {
            match parse_codex_sqlite(project_path, &codex_dir, Some(session_id)) {
                Ok(entries) => {
                    for entry in &entries {
                        let _ = super::store::add_entry(conn, entry);
                    }
                    total += entries.len();
                }
                Err(_) => {
                    // Best-effort: Codex DB may be locked or corrupted
                }
            }
        }
    }

    Ok(total)
}

/// Parse Codex SQLite databases to extract memories as knowledge entries.
/// Opens `memories_1.sqlite` read-only and optionally `state_5.sqlite` read-only
/// to filter by project working directory.
pub fn parse_codex_sqlite(
    project_path: &str,
    codex_dir: &Path,
    session_id: Option<&str>,
) -> Result<Vec<KnowledgeEntry>, AppError> {
    let project_hash = hash_project_path(project_path);

    let memories_db = codex_dir.join("memories_1.sqlite");
    if !memories_db.exists() {
        return Ok(vec![]);
    }

    // Open read-only to avoid WAL conflicts with a running Codex instance
    let conn = rusqlite::Connection::open_with_flags(
        &memories_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ).map_err(|e| AppError::KnowledgeCollection {
        agent: "Codex".to_string(),
        reason: format!("无法打开 memories_1.sqlite: {}", e),
    })?;

    // Determine which thread_ids are relevant to this project
    let thread_ids = resolve_codex_threads(codex_dir, project_path, session_id)?;

    if thread_ids.is_empty() {
        return Ok(vec![]);
    }

    // Build parameterized query with IN clause
    let placeholders: Vec<String> = thread_ids.iter().enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect();
    let sql = format!(
        "SELECT thread_id, raw_memory, rollout_summary, rollout_slug
         FROM stage1_outputs WHERE thread_id IN ({})
         ORDER BY source_updated_at DESC",
        placeholders.join(", ")
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| AppError::KnowledgeCollection {
        agent: "Codex".to_string(),
        reason: format!("查询 stage1_outputs 失败: {}", e),
    })?;

    let params: Vec<Box<dyn rusqlite::types::ToSql>> = thread_ids
        .iter()
        .map(|id| Box::new(id.clone()) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    }).map_err(|e| AppError::KnowledgeCollection {
        agent: "Codex".to_string(),
        reason: format!("读取 stage1_outputs 行失败: {}", e),
    })?;

    let mut entries = Vec::new();

    for row in rows {
        let (thread_id, raw_memory, rollout_summary, rollout_slug) = row.map_err(|e| AppError::KnowledgeCollection {
            agent: "Codex".to_string(),
            reason: format!("解析 stage1_outputs 行失败: {}", e),
        })?;

        let title = rollout_summary
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| truncate_title(s, 80))
            .unwrap_or_else(|| truncate_title(&raw_memory, 80));

        let mut content = raw_memory;
        if let Some(slug) = rollout_slug.filter(|s| !s.is_empty()) {
            content = format!("[{}]\n{}", slug, content);
        }

        if content.trim().is_empty() || content.len() < 20 {
            continue;
        }
        content.truncate(5000);

        entries.push(KnowledgeEntry {
            id: uuid::Uuid::new_v4().to_string(),
            project_hash: project_hash.clone(),
            category: "agent_output".to_string(),
            title,
            content,
            source_agent: AgentType::Codex,
            source_session_id: session_id.map(|s| s.to_string()).or(Some(thread_id)),
            source_type: "codex_sqlite_memory".to_string(),
            confidence: 0.7,
            created_at: chrono::Local::now().to_rfc3339(),
            updated_at: chrono::Local::now().to_rfc3339(),
            access_count: 0,
        });

        if entries.len() >= 50 {
            break;
        }
    }

    Ok(entries)
}

/// Resolve Codex thread IDs relevant to a given project path.
/// Opens `state_5.sqlite` read-only and matches threads by working directory.
fn resolve_codex_threads(
    codex_dir: &Path,
    project_path: &str,
    session_id: Option<&str>,
) -> Result<Vec<String>, AppError> {
    let state_db = codex_dir.join("state_5.sqlite");
    if !state_db.exists() {
        // If no state DB, we can't filter — return empty rather than all
        return Ok(vec![]);
    }

    let conn = rusqlite::Connection::open_with_flags(
        &state_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ).map_err(|e| AppError::KnowledgeCollection {
        agent: "Codex".to_string(),
        reason: format!("无法打开 state_5.sqlite: {}", e),
    })?;

    let project_basename = std::path::Path::new(project_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    let mut thread_ids = Vec::new();

    // If session_id is provided, try exact thread match first
    if let Some(sid) = session_id {
        let result: Result<String, _> = conn.query_row(
            "SELECT id FROM threads WHERE id = ?1",
            rusqlite::params![sid],
            |row| row.get(0),
        );
        if let Ok(id) = result {
            thread_ids.push(id);
        }
    }

    // Also match by project path (exact or basename-based)
    let mut stmt = conn.prepare(
        "SELECT id FROM threads WHERE cwd = ?1 OR cwd LIKE ?2"
    ).map_err(|e| AppError::KnowledgeCollection {
        agent: "Codex".to_string(),
        reason: format!("查询 threads 失败: {}", e),
    })?;

    let exact = project_path.to_string();
    let basename_pattern = if cfg!(target_os = "windows") {
        format!("%\\{}%", project_basename)
    } else {
        format!("%/{}%", project_basename)
    };

    let rows = stmt.query_map(
        rusqlite::params![exact, basename_pattern],
        |row| row.get::<_, String>(0),
    ).map_err(|e| AppError::KnowledgeCollection {
        agent: "Codex".to_string(),
        reason: format!("读取 threads 行失败: {}", e),
    })?;

    for row in rows {
        if let Ok(id) = row {
            if !thread_ids.contains(&id) {
                thread_ids.push(id);
            }
        }
        if thread_ids.len() >= 100 {
            break;
        }
    }

    Ok(thread_ids)
}

/// Check whether text looks like CLI help output, error messages, or diff noise
/// rather than genuine knowledge content. Returns true if the text should be skipped.
fn is_noise_content(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return true;
    }

    // Quick reject: text that starts with typical CLI noise patterns
    let trimmed = text.trim();
    if trimmed.starts_with("Usage:")
        || trimmed.starts_with("Options:")
        || trimmed.starts_with("Flags:")
        || trimmed.starts_with("COMMANDS:")
        || trimmed.starts_with("Commands:")
    {
        return true;
    }

    let mut noise_lines = 0usize;
    let total_lines = lines.len();

    for line in &lines {
        let lt = line.trim();
        if lt.is_empty() {
            continue;
        }
        // CLI flag lines: `  --force     skip checks`
        if lt.starts_with("--") || lt.starts_with('-') && lt.len() > 1 && lt.chars().nth(1).map(|c| c.is_ascii_alphabetic()).unwrap_or(false) {
            noise_lines += 1;
            continue;
        }
        // Diff lines: `+ added` / `- removed` / `@@ hunk @@`
        if (lt.starts_with('+') || lt.starts_with('-')) && lt.len() > 1 && lt.chars().nth(1).map(|c| c != '+' && c != '-').unwrap_or(true) {
            noise_lines += 1;
            continue;
        }
        if lt.starts_with("@@") && lt.contains("@@") {
            noise_lines += 1;
            continue;
        }
        // File:line references: `src/main.rs:42:10`
        if regex_line_ref(lt) {
            noise_lines += 1;
            continue;
        }
        // Section headers common in CLI help
        if lt == "Usage:" || lt == "Flags:" || lt == "Options:" || lt == "Commands:" || lt == "Arguments:" || lt == "Examples:" {
            noise_lines += 1;
        }
    }

    // If more than 40% of lines are noise, skip the whole text
    let ratio = noise_lines as f64 / total_lines as f64;
    ratio > 0.4
}

/// Check if a line looks like a file:line:col reference
fn regex_line_ref(line: &str) -> bool {
    // Simple check: contains `:\d+:\d+` pattern
    let bytes = line.as_bytes();
    let mut colon_count = 0;
    let mut digit_after_colon = false;
    for i in 0..bytes.len() {
        if bytes[i] == b':' {
            colon_count += 1;
            if i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
                digit_after_colon = true;
            }
        }
    }
    colon_count >= 2 && digit_after_colon
}

fn truncate_title(text: &str, max_len: usize) -> String {
    let first_line = text.lines().next().unwrap_or(text);
    if first_line.chars().count() <= max_len {
        first_line.to_string()
    } else {
        format!("{}...", first_line.chars().take(max_len).collect::<String>())
    }
}

fn split_into_chunks(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for line in text.lines() {
        if current.len() + line.len() + 1 > max_chars && !current.is_empty() {
            chunks.push(current.trim().to_string());
            current.clear();
        }
        if !line.trim().is_empty() {
            current.push_str(line);
            current.push('\n');
        }
    }

    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }

    // Cap at 10 chunks
    chunks.truncate(10);
    chunks
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_devworkbench_log() {
        let tmp = tempfile::TempDir::new().unwrap();
        let log_path = tmp.path().join("test.log");
        std::fs::write(&log_path, "Line 1: some output from agent\nLine 2: more output here\n").unwrap();

        let entries = parse_devworkbench_log(
            "/proj/a",
            &log_path,
            "s1",
            &AgentType::ClaudeCode,
        ).unwrap();

        assert!(!entries.is_empty());
        assert_eq!(entries[0].source_session_id, Some("s1".to_string()));
    }

    #[test]
    fn test_truncate_title() {
        assert_eq!(truncate_title("short", 10), "short");
        let result = truncate_title("a very long title that exceeds limit", 10);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 13); // max_len + "..."
    }

    #[test]
    fn test_split_into_chunks() {
        let text = "Line 1\nLine 2\nLine 3\nLine 4\n";
        let chunks = split_into_chunks(text, 15);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_parse_codex_sqlite_empty_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let codex_dir = tmp.path().join(".codex");
        std::fs::create_dir_all(&codex_dir).unwrap();

        let entries = parse_codex_sqlite("/proj/a", &codex_dir, None).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_codex_sqlite_with_data() {
        let tmp = tempfile::TempDir::new().unwrap();
        let codex_dir = tmp.path().join(".codex");
        std::fs::create_dir_all(&codex_dir).unwrap();

        // Create a mock memories_1.sqlite
        let mem_path = codex_dir.join("memories_1.sqlite");
        {
            let conn = rusqlite::Connection::open(&mem_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE stage1_outputs (
                    thread_id TEXT PRIMARY KEY,
                    source_updated_at INTEGER,
                    raw_memory TEXT NOT NULL,
                    rollout_summary TEXT,
                    rollout_slug TEXT,
                    generated_at INTEGER,
                    usage_count INTEGER,
                    last_usage INTEGER
                );
                INSERT INTO stage1_outputs (thread_id, source_updated_at, raw_memory, rollout_summary, rollout_slug, generated_at, usage_count, last_usage)
                VALUES ('t1', 1700000000, 'This is a detailed memory about implementing error handling in Rust with thiserror and anyhow crates for robust error propagation', 'Rust error handling', 'rust-errors', 1700000000, 1, 1700000000);"
            ).unwrap();
        }

        // Create a mock state_5.sqlite with matching thread
        let state_path = codex_dir.join("state_5.sqlite");
        {
            let conn = rusqlite::Connection::open(&state_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    rollout_path TEXT NOT NULL DEFAULT '',
                    created_at INTEGER,
                    updated_at INTEGER,
                    source TEXT NOT NULL DEFAULT '',
                    model_provider TEXT NOT NULL DEFAULT '',
                    cwd TEXT NOT NULL,
                    title TEXT,
                    sandbox_policy TEXT NOT NULL DEFAULT '',
                    approval_mode TEXT NOT NULL DEFAULT '',
                    tokens_used INTEGER NOT NULL DEFAULT 0,
                    has_user_event INTEGER NOT NULL DEFAULT 0
                );
                INSERT INTO threads (id, cwd, title, created_at, updated_at)
                VALUES ('t1', '/proj/a', 'Test thread', 1700000000, 1700000000);"
            ).unwrap();
        }

        let entries = parse_codex_sqlite("/proj/a", &codex_dir, None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source_agent, AgentType::Codex);
        assert_eq!(entries[0].source_type, "codex_sqlite_memory");
        assert_eq!(entries[0].category, "agent_output");
        assert!((entries[0].confidence - 0.7).abs() < 0.01);
        assert_eq!(entries[0].title, "Rust error handling");
    }

    #[test]
    fn test_noise_content_cli_help() {
        let cli_help = r#"Usage:
  forge gate <gate-id> [flags]

Flags:
      --current   Run current gate
      --force     Skip prerequisites
  -h, --help      Help for gate
      --retry     Retry last failed gate
      --silent    Status code only"#;
        assert!(is_noise_content(cli_help));
    }

    #[test]
    fn test_noise_content_error_with_help() {
        let forge_error = r#"Error: not in a forge project (no .forge/ directory found)
Usage:
  forge gate <gate-id> [--force] [--retry] [--silent] [--current] [flags]

Flags:
      --current   Run current active gate
      --force     Skip prerequisite checks
  -h, --help      help for gate
      --retry     Re-execute last failed gate
      --silent    Output status code only"#;
        assert!(is_noise_content(forge_error));
    }

    #[test]
    fn test_noise_content_normal_text() {
        let normal = "The project uses a React frontend with TypeScript and a Rust backend via Tauri. \
            State management is handled by Zustand stores, and the terminal emulation uses xterm.js \
            for real-time PTY output streaming from spawned agent processes.";
        assert!(!is_noise_content(normal));
    }

    #[test]
    fn test_noise_content_diff() {
        let diff = r#"@@ -10,6 +10,8 @@
 fn main() {
-    println!("old");
+    println!("new");
+    println!("added");
 }"#;
        assert!(is_noise_content(diff));
    }
}
