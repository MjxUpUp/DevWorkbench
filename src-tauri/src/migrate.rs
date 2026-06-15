use crate::db;
use crate::error::AppError;
use crate::models::{AppSettings, Project, Session};
use rusqlite::Connection;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::Path;

/// Migrate v0.6 JSON data to v0.7 SQLite.
///
/// - Idempotent: checks `schema_version` first, skips if already migrated.
/// - Transactional: wraps all inserts in a transaction; on failure the JSON
///   files are left untouched.
/// - Backup: renames original files to `.v0.6.bak` on success.
pub fn migrate_v6_to_v7(conn: &Connection, data_dir: &Path) -> Result<(), AppError> {
    // v6-specific marker. The old guard used is_migrated (version >= 8), which
    // was already true once migrate_v7_to_v8 had run — so this function
    // short-circuited and sessions.json was never imported, silently losing all
    // v0.6 conversation history. Additionally the body built a transaction but
    // never committed it, so even when it did run nothing persisted. Both fixed.
    if db::is_v6_migrated(conn) {
        return Ok(());
    }

    let agents_dir = data_dir.join("agents");

    // Begin transaction — all-or-nothing
    let tx = conn.unchecked_transaction()?;

    // 1. Migrate sessions. Prefer the live sessions.json; fall back to
    //    sessions.json.v0.6.bak — an earlier buggy run of this function
    //    renamed the live file to .bak but (because it never committed) left
    //    zero rows in the DB, so the .bak is the ONLY surviving copy of the
    //    v0.6 history for installs that already hit that path.
    let sessions_live = agents_dir.join("sessions.json");
    let sessions_bak = agents_dir.join("sessions.json.v0.6.bak");
    let (sessions_file, from_backup) = if sessions_live.exists() {
        (sessions_live.clone(), false)
    } else if sessions_bak.exists() {
        (sessions_bak.clone(), true)
    } else {
        (sessions_live.clone(), false) // neither exists — reads below no-op
    };
    let mut imported = 0;
    if sessions_file.exists() {
        let content = fs::read_to_string(&sessions_file)?;
        if !content.trim().is_empty() {
            let sessions: Vec<Session> = serde_json::from_str(&content)?;
            for s in &sessions {
                insert_session(&tx, s)?;
                imported += 1;
            }
        }
    }

    // 2. Mark v6→v7 done and COMMIT (the missing commit was the second half of
    //    the history-loss bug — the transaction was discarded on drop).
    tx.execute(
        "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (7, ?1)",
        [chrono::Utc::now().to_rfc3339()],
    )?;
    tx.commit()?;

    // 3. Backup the live sessions file (post-commit, best-effort). Skip when we
    //    already read from the .bak — renaming it onto itself is a no-op at
    //    best and could clobber the only surviving copy at worst.
    if !from_backup && sessions_live.exists() {
        let _ = fs::rename(&sessions_live, &sessions_bak);
    }

    log::info!("v0.6→v0.7 migration: imported {} sessions from sessions.json", imported);
    Ok(())
}

fn insert_session(conn: &Connection, s: &Session) -> Result<(), AppError> {
    let snapshot_json = s.context_snapshot.as_ref().map(|cs| serde_json::to_string(cs).unwrap_or_default());
    conn.execute(
        "INSERT OR IGNORE INTO sessions
            (id, project_path, agent_type, status, prompt, model,
             started_at, finished_at, exit_code, output_summary,
             context_snapshot, linked_requirement_id, parent_session_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        rusqlite::params![
            s.id,
            s.project_path,
            serde_json::to_string(&s.agent_type)?.trim_matches('"'),
            s.status.as_str(),
            s.prompt,
            s.model,
            s.started_at,
            s.finished_at,
            s.exit_code,
            s.output_summary,
            snapshot_json,
            s.linked_requirement_id,
            s.parent_session_id,
        ],
    )?;
    Ok(())
}

/// Migrate v0.7 JSON projects/settings to v0.8 SQLite.
///
/// - Idempotent: checks `schema_version` for version >= 8.
/// - Reads `projects.json` and `settings.json` from the data directory.
/// - Inserts into `projects` and `settings` tables.
/// - Renames original files to `.v0.7.bak`.
pub fn migrate_v7_to_v8(conn: &Connection, data_dir: &Path) -> Result<(), AppError> {
    // Check if already migrated
    let already_migrated: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_version WHERE version >= 8",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);
    if already_migrated {
        return Ok(());
    }

    let tx = conn.unchecked_transaction()?;

    // 1. Migrate projects.json
    let projects_file = data_dir.join("projects.json");
    if projects_file.exists() {
        let content = fs::read_to_string(&projects_file)?;
        if !content.trim().is_empty() {
            let projects: Vec<Project> = serde_json::from_str(&content)?;
            for p in &projects {
                insert_project(&tx, p)?;
            }
        }
    }

    // 2. Migrate settings.json
    let settings_file = data_dir.join("settings.json");
    if settings_file.exists() {
        let content = fs::read_to_string(&settings_file)?;
        if !content.trim().is_empty() {
            let settings: AppSettings = serde_json::from_str(&content)?;
            insert_settings(&tx, &settings)?;
        }
    }

    // 3. Mark migration done
    tx.execute(
        "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (8, ?1)",
        [chrono::Utc::now().to_rfc3339()],
    )?;

    tx.commit()?;

    // 4. Backup original files (post-commit, best-effort)
    if projects_file.exists() {
        let _ = fs::rename(&projects_file, data_dir.join("projects.json.v0.7.bak"));
    }
    if settings_file.exists() {
        let _ = fs::rename(&settings_file, data_dir.join("settings.json.v0.7.bak"));
    }

    Ok(())
}

fn insert_project(conn: &Connection, p: &Project) -> Result<(), AppError> {
    conn.execute(
        "INSERT OR IGNORE INTO projects
            (id, name, description, path, tags, cover_image, open_count,
             last_opened_at, starred, created_at, last_opened_tools, workspace_tools)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            p.id,
            p.name,
            p.description,
            p.path,
            serde_json::to_string(&p.tags)?,
            p.cover_image,
            p.open_count,
            p.last_opened_at,
            p.starred as i32,
            p.created_at,
            serde_json::to_string(&p.last_opened_tools)?,
            serde_json::to_string(&p.workspace_tools)?,
        ],
    )?;
    Ok(())
}

fn insert_settings(conn: &Connection, s: &AppSettings) -> Result<(), AppError> {
    conn.execute(
        "INSERT OR REPLACE INTO settings
            (id, scan_directories, tool_paths, theme, preferred_terminal, cli_flags)
         VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            serde_json::to_string(&s.scan_directories)?,
            serde_json::to_string(&s.tool_paths)?,
            s.theme,
            s.preferred_terminal,
            serde_json::to_string(&s.cli_flags)?,
        ],
    )?;
    Ok(())
}

/// Migrate v0.8 to v1.0 schema (v9).
///
/// v1.0 adds workflows, skills, cost_records, and budget_settings tables.
/// These tables are created by CREATE TABLE IF NOT EXISTS in db.rs SCHEMA,
/// so this function only records the migration version.
pub fn migrate_v8_to_v9(conn: &Connection) -> Result<(), AppError> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if version < 9 {
        conn.execute(
            "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (9, ?1)",
            [chrono::Utc::now().to_rfc3339()],
        )?;
        log::info!("Migrated schema from v8 to v9");
    }
    Ok(())
}

/// Migrate v9 → v10: introduce the `conversations` table and backfill every
/// existing session into a conversation by collapsing `parent_session_id`
/// chains.
///
/// Each parent-chain (root + all descendants) becomes ONE conversation; an
/// isolated session (no parent, no children) becomes its own conversation. A
/// dangling parent (points at an id not in the table) is treated as a root, so
/// history is never silently dropped. Transactional — on failure nothing is
/// written, so a re-run starts clean. Idempotent via schema_version >= 10.
pub fn migrate_v9_to_v10(conn: &Connection) -> Result<(), AppError> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if version >= 10 {
        return Ok(());
    }

    let tx = conn.unchecked_transaction()?;

    // 1. conversations table + indexes (idempotent; also in SCHEMA for fresh DBs).
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS conversations (
            id TEXT PRIMARY KEY,
            project_path TEXT NOT NULL,
            title TEXT NOT NULL,
            last_agent TEXT,
            status TEXT NOT NULL DEFAULT 'active',
            started_at TEXT NOT NULL,
            last_activity_at TEXT NOT NULL,
            pinned INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_conversations_project ON conversations(project_path);
        CREATE INDEX IF NOT EXISTS idx_conversations_last_activity ON conversations(last_activity_at DESC);",
    )?;

    // 2. sessions.conversation_id column. rusqlite has no ADD COLUMN IF NOT
    //    EXISTS, so probe by preparing a statement that references the column.
    let col_exists = tx
        .prepare("SELECT conversation_id FROM sessions LIMIT 0")
        .is_ok();
    if !col_exists {
        tx.execute("ALTER TABLE sessions ADD COLUMN conversation_id TEXT", [])?;
    }
    // Index creation is OUTSIDE the `if !col_exists` branch. A fresh DB has the
    // column already (from the static SCHEMA's CREATE TABLE), so it skips the
    // ALTER — and previously skipped the index too, leaving the column
    // un-indexed forever. CREATE INDEX IF NOT EXISTS is idempotent, so running
    // it unconditionally is safe for old-DB, fresh-DB, and re-run cases alike.
    // (The index deliberately does NOT live in the static SCHEMA: see db.rs.)
    tx.execute(
        "CREATE INDEX IF NOT EXISTS idx_sessions_conversation ON sessions(conversation_id)",
        [],
    )?;

    // 3. Load the parent-chain structure of every session.
    struct Row {
        id: String,
        project_path: String,
        prompt: String,
        agent_type: String,
        started_at: String,
        parent_session_id: Option<String>,
    }
    let rows: Vec<Row> = {
        let mut stmt = tx.prepare(
            "SELECT id, project_path, prompt, agent_type, started_at, parent_session_id
             FROM sessions",
        )?;
        let mapped = stmt.query_map([], |r| Ok(Row {
            id: r.get(0)?,
            project_path: r.get(1)?,
            prompt: r.get(2)?,
            agent_type: r.get(3)?,
            started_at: r.get(4)?,
            parent_session_id: r.get(5)?,
        }))?;
        mapped.collect::<Result<Vec<_>, _>>()?
    };

    let by_id: HashMap<&str, &Row> = rows.iter().map(|r| (r.id.as_str(), r)).collect();
    // parent_id → child ids (reverse index).
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for r in &rows {
        if let Some(p) = r.parent_session_id.as_deref() {
            children.entry(p).or_default().push(r.id.as_str());
        }
    }

    // Roots: parent is None, OR points at an id absent from the table (dangling
    // — treated as a root rather than skipped, so the chain is still preserved).
    let roots: Vec<&str> = rows
        .iter()
        .filter(|r| match r.parent_session_id.as_deref() {
            None => true,
            Some(p) => !by_id.contains_key(p),
        })
        .map(|r| r.id.as_str())
        .collect();

    let now = chrono::Utc::now().to_rfc3339();
    let mut conversations_made = 0usize;
    for root_id in roots {
        // BFS the whole chain from this root.
        let mut chain: Vec<&Row> = Vec::new();
        let mut queue: VecDeque<&str> = VecDeque::new();
        queue.push_back(root_id);
        while let Some(id) = queue.pop_front() {
            if let Some(r) = by_id.get(id) {
                chain.push(r);
                if let Some(kids) = children.get(id) {
                    for k in kids {
                        queue.push_back(k);
                    }
                }
            }
        }
        if chain.is_empty() {
            continue;
        }
        // Order by started_at so title comes from the earliest turn and
        // last_agent from the latest.
        chain.sort_by(|a, b| a.started_at.cmp(&b.started_at));
        let first = chain.first().unwrap();
        let last = chain.last().unwrap();
        let title: String = first.prompt.chars().take(40).collect();
        let conv_id = uuid::Uuid::new_v4().to_string();

        tx.execute(
            "INSERT OR IGNORE INTO conversations
                (id, project_path, title, last_agent, status, started_at, last_activity_at, pinned)
             VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, 0)",
            rusqlite::params![
                conv_id,
                first.project_path,
                title,
                last.agent_type,
                first.started_at,
                last.started_at,
            ],
        )?;
        for r in &chain {
            tx.execute(
                "UPDATE sessions SET conversation_id = ?1 WHERE id = ?2",
                rusqlite::params![conv_id, r.id],
            )?;
        }
        conversations_made += 1;
    }

    tx.execute(
        "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (10, ?1)",
        [now],
    )?;
    tx.commit()?;
    log::info!(
        "v9→v10 migration: created {} conversations from {} sessions",
        conversations_made,
        rows.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::session::insert_session_db;
    use crate::models::{AgentType, Session, SessionStatus};

    struct TempDb {
        _tmp: tempfile::TempDir,
        conn: Connection,
    }
    impl TempDb {
        fn new() -> Self {
            let tmp = tempfile::TempDir::new().unwrap();
            let path = tmp.path().join("test.db");
            let conn = db::init_db(&path).unwrap();
            Self { _tmp: tmp, conn }
        }
    }

    /// Build a completed session. `started_at` is derived from the id length so
    /// a chain A→B→C has a stable increasing order for title/last_agent tests.
    fn mk(id: &str, project: &str, parent: Option<&str>) -> Session {
        Session {
            id: id.to_string(),
            project_path: project.to_string(),
            agent_type: AgentType::ClaudeCode,
            status: SessionStatus::Completed,
            prompt: format!("prompt-{}", id),
            model: None,
            started_at: format!("2026-01-01T00:00:0{}Z", id.len()),
            finished_at: None,
            exit_code: None,
            output_summary: None,
            context_snapshot: None,
            linked_requirement_id: None,
            parent_session_id: parent.map(|s| s.to_string()),
            conversation_id: None,
        }
    }

    fn conversation_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM conversations", [], |r| r.get(0))
            .unwrap()
    }

    fn distinct_conversation_ids(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT COUNT(DISTINCT conversation_id) FROM sessions WHERE conversation_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn parent_chain_collapses_into_one_conversation() {
        let g = TempDb::new();
        insert_session_db(&g.conn, &mk("A", "/p", None)).unwrap();
        insert_session_db(&g.conn, &mk("BB", "/p", Some("A"))).unwrap();
        insert_session_db(&g.conn, &mk("CCC", "/p", Some("BB"))).unwrap();

        migrate_v9_to_v10(&g.conn).unwrap();

        assert_eq!(conversation_count(&g.conn), 1);
        // All three turns share the single conversation.
        assert_eq!(distinct_conversation_ids(&g.conn), 1);
    }

    #[test]
    fn isolated_sessions_each_become_their_own_conversation() {
        let g = TempDb::new();
        insert_session_db(&g.conn, &mk("X", "/p", None)).unwrap();
        insert_session_db(&g.conn, &mk("Y", "/p", None)).unwrap();

        migrate_v9_to_v10(&g.conn).unwrap();

        assert_eq!(conversation_count(&g.conn), 2);
        assert_eq!(distinct_conversation_ids(&g.conn), 2);
    }

    #[test]
    fn dangling_parent_is_treated_as_a_root_not_dropped() {
        // B claims parent "GONE" which isn't in the table — B must still be
        // backfilled into its own conversation, never silently dropped.
        let g = TempDb::new();
        insert_session_db(&g.conn, &mk("B", "/p", Some("GONE"))).unwrap();

        migrate_v9_to_v10(&g.conn).unwrap();

        assert_eq!(conversation_count(&g.conn), 1);
        assert_eq!(distinct_conversation_ids(&g.conn), 1);
    }

    #[test]
    fn idempotent_second_run_is_a_noop() {
        let g = TempDb::new();
        insert_session_db(&g.conn, &mk("A", "/p", None)).unwrap();
        insert_session_db(&g.conn, &mk("BB", "/p", Some("A"))).unwrap();

        migrate_v9_to_v10(&g.conn).unwrap();
        migrate_v9_to_v10(&g.conn).unwrap(); // second run

        assert_eq!(conversation_count(&g.conn), 1);
    }

    /// Regression: a pre-v10 database has a `sessions` table WITHOUT the
    /// conversation_id column. The old static SCHEMA created
    /// `idx_sessions_conversation ON sessions(conversation_id)` *before* any
    /// migration ran, so opening such a DB aborted the whole SCHEMA batch with
    /// `no such column: conversation_id` and the app panicked on every launch.
    /// This builds that exact old-schema DB, re-opens it (which re-runs SCHEMA
    /// + migration the way lib.rs does), and asserts it survives with the
    /// column + index present and the existing session backfilled.
    #[test]
    fn pre_v10_db_without_conversation_id_column_opens_and_migrates() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("old.db");

        // 1. Build a pre-v10 DB by hand: schema_version at 9, a sessions table
        //    whose CREATE matches the OLD shape (no conversation_id column),
        //    and one real row. This is exactly what an upgraded install has on
        //    disk before this fix.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);
                 INSERT INTO schema_version (version, applied_at) VALUES (9, '2026-01-01T00:00:00Z');
                 CREATE TABLE sessions (
                     id TEXT PRIMARY KEY,
                     project_path TEXT NOT NULL,
                     agent_type TEXT NOT NULL,
                     status TEXT NOT NULL DEFAULT 'running',
                     prompt TEXT NOT NULL,
                     model TEXT,
                     started_at TEXT NOT NULL,
                     finished_at TEXT,
                     exit_code INTEGER,
                     output_summary TEXT,
                     context_snapshot TEXT,
                     linked_requirement_id TEXT,
                     parent_session_id TEXT
                 );
                 INSERT INTO sessions (id, project_path, agent_type, status, prompt, model,
                     started_at, finished_at, exit_code, output_summary, context_snapshot,
                     linked_requirement_id, parent_session_id)
                 VALUES ('legacy1', '/p', 'claude_code', 'completed', 'old prompt',
                     NULL, '2026-01-01T00:00:00Z', NULL, 0, NULL, NULL, NULL, NULL);",
            )
            .unwrap();
        }

        // 2. Re-open exactly like the app does: init_db runs the full SCHEMA
        //    (which previously included the offending index), then migrations
        //    run. Before the fix this line panicked.
        let conn = db::init_db(&path).expect("re-opening a pre-v10 DB must not panic");
        migrate_v9_to_v10(&conn).expect("v9→v10 migration on a pre-v10 DB must succeed");

        // 3. The column now exists, the index is in place, and the legacy
        //    session was backfilled into a conversation.
        let has_col: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='conversation_id'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(has_col, 1, "conversation_id column must be added");

        let conv_id: Option<String> = conn
            .query_row("SELECT conversation_id FROM sessions WHERE id='legacy1'", [], |r| r.get(0))
            .unwrap();
        assert!(conv_id.is_some(), "legacy session must be backfilled into a conversation");

        // The index must exist (fresh-DB path used to skip it).
        let idx_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_sessions_conversation'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(idx_count, 1, "idx_sessions_conversation must be created");
    }
}
