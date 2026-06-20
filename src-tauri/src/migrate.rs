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
            match serde_json::from_str::<Vec<Session>>(&content) {
                Ok(sessions) => {
                    for s in &sessions {
                        insert_session(&tx, s)?;
                        imported += 1;
                    }
                }
                Err(e) => {
                    // A corrupt/truncated sessions.json would otherwise propagate
                    // up through lib.rs's `.expect("Failed to run data migration")`
                    // and crash the app on EVERY startup — bricking the install
                    // until the file is hand-edited. Quarantine the file aside
                    // (.corrupt) and proceed with zero imported sessions so the
                    // app can still start; the v0.6 history in that one file is
                    // lost, but the app remains usable.
                    let mut corrupt_path = sessions_file.clone();
                    let mut name = corrupt_path
                        .file_name()
                        .map(|n| n.to_os_string())
                        .unwrap_or_default();
                    name.push(".corrupt");
                    corrupt_path.set_file_name(name);
                    log::error!(
                        "v0.6 sessions.json is corrupt ({}); moving it to {} and \
                         skipping import so startup can proceed",
                        e,
                        corrupt_path.display()
                    );
                    let _ = fs::rename(&sessions_file, &corrupt_path);
                }
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

    log::info!(
        "v0.6→v0.7 migration: imported {} sessions from sessions.json",
        imported
    );
    Ok(())
}

fn insert_session(conn: &Connection, s: &Session) -> Result<(), AppError> {
    let snapshot_json = s
        .context_snapshot
        .as_ref()
        .map(|cs| serde_json::to_string(cs).unwrap_or_default());
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
        let mapped = stmt.query_map([], |r| {
            Ok(Row {
                id: r.get(0)?,
                project_path: r.get(1)?,
                prompt: r.get(2)?,
                agent_type: r.get(3)?,
                started_at: r.get(4)?,
                parent_session_id: r.get(5)?,
            })
        })?;
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

/// Migrate v10 → v11: add the `blocks` column to `sessions`.
///
/// `blocks` stores the persisted chat blocks (a JSON array of
/// text/tool_use/tool_result events) written at finalize, so a historical
/// session replays via BlocksView instead of falling back to the raw terminal
/// log. For fresh DBs the column is already in the static SCHEMA; this only
/// ALTERs existing v10 databases. Idempotent via schema_version >= 11, with a
/// column-presence probe (same idiom as v9→v10's conversation_id probe) so a
/// fresh DB that already has the column skips the ALTER.
pub fn migrate_v10_to_v11(conn: &Connection) -> Result<(), AppError> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if version >= 11 {
        return Ok(());
    }

    // rusqlite has no ADD COLUMN IF NOT EXISTS; probe by preparing a statement
    // that references the column (same idiom as v9→v10's conversation_id probe).
    // A fresh DB already has `blocks` from the static SCHEMA and skips the ALTER.
    let col_exists = conn.prepare("SELECT blocks FROM sessions LIMIT 0").is_ok();
    if !col_exists {
        conn.execute("ALTER TABLE sessions ADD COLUMN blocks TEXT", [])?;
        log::info!("Migrated schema v10→v11: added sessions.blocks column");
    }

    conn.execute(
        "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (11, ?1)",
        [chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// v11→v12: add `sessions.task_ref TEXT` so a session can be bound to the Forge
/// task it ran under (drives TaskGuardHook's working_dir boundary check). Same
/// idempotent shape as v10→v11 — probe the column (fresh DBs already have it
/// from the static SCHEMA), ALTER only if missing, then bump the version.
pub fn migrate_v11_to_v12(conn: &Connection) -> Result<(), AppError> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if version >= 12 {
        return Ok(());
    }

    // rusqlite has no ADD COLUMN IF NOT EXISTS; probe by preparing a statement
    // that references the column (same idiom as v10→v11's blocks probe). A
    // fresh DB already has `task_ref` from the static SCHEMA and skips the ALTER.
    let col_exists = conn
        .prepare("SELECT task_ref FROM sessions LIMIT 0")
        .is_ok();
    if !col_exists {
        conn.execute("ALTER TABLE sessions ADD COLUMN task_ref TEXT", [])?;
        log::info!("Migrated schema v11→v12: added sessions.task_ref column");
    }

    conn.execute(
        "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (12, ?1)",
        [chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// v12→v13: add `user_hooks.matcher TEXT` so a Pre/PostToolUse hook can scope to
/// specific tools (claude-code `matcher` field — literal / pipe / regex). Same
/// idempotent shape as v10→v11 / v11→v12: probe the column (fresh DBs already
/// have it from the static SCHEMA), ALTER only if missing, then bump the
/// version. NULL default = match all, so pre-existing rows behave unchanged.
pub fn migrate_v12_to_v13(conn: &Connection) -> Result<(), AppError> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if version >= 13 {
        return Ok(());
    }

    // rusqlite has no ADD COLUMN IF NOT EXISTS; probe by preparing a statement
    // that references the column (same idiom as v10→v11's blocks probe). A fresh
    // DB already has `matcher` from the static SCHEMA and skips the ALTER.
    let col_exists = conn
        .prepare("SELECT matcher FROM user_hooks LIMIT 0")
        .is_ok();
    if !col_exists {
        conn.execute("ALTER TABLE user_hooks ADD COLUMN matcher TEXT", [])?;
        log::info!("Migrated schema v12→v13: added user_hooks.matcher column");
    }

    conn.execute(
        "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (13, ?1)",
        [chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// v13→v14: add the `llm_traces` table (one row per LLM HTTP call — request
/// body, status, error body, latency, tokens). Unlike v10→v13 which ALTER
/// existing tables, this creates a brand-new table, so it's a plain
/// `CREATE TABLE IF NOT EXISTS` (idempotent on its own). A fresh DB already has
/// the table from the static SCHEMA — the CREATE is a no-op there; on a pre-v14
/// DB it materializes the table. Then bump the version.
pub fn migrate_v13_to_v14(conn: &Connection) -> Result<(), AppError> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if version >= 14 {
        return Ok(());
    }

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS llm_traces (
            id TEXT PRIMARY KEY,
            session_id TEXT,
            conversation_id TEXT,
            model TEXT NOT NULL,
            base_url TEXT NOT NULL,
            status_code INTEGER,
            error_kind TEXT,
            req_body TEXT,
            resp_body TEXT,
            latency_ms INTEGER,
            input_tokens INTEGER,
            output_tokens INTEGER,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_llm_traces_session ON llm_traces(session_id, created_at);",
    )?;
    log::info!("Migrated schema v13→v14: created llm_traces table");

    conn.execute(
        "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (14, ?1)",
        [chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// v14→v15: add `trace_settings` (single-row retention config, mirrors
/// `budget_settings`) and the `idx_llm_traces_created` index that speeds the
/// retention prune. Like v13→v14 this is plain `CREATE TABLE IF NOT EXISTS` /
/// `CREATE INDEX IF NOT EXISTS` (idempotent); a fresh DB already has both from
/// the static SCHEMA, so this is a no-op there and only materializes them on a
/// pre-v15 DB. Then bump the version.
pub fn migrate_v14_to_v15(conn: &Connection) -> Result<(), AppError> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if version >= 15 {
        return Ok(());
    }

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS trace_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            retention_days INTEGER,
            last_vacuum_at TEXT,
            updated_at TEXT NOT NULL
        );
        INSERT OR IGNORE INTO trace_settings (id, retention_days, last_vacuum_at, updated_at)
        VALUES (1, NULL, NULL, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));
        CREATE INDEX IF NOT EXISTS idx_llm_traces_created ON llm_traces(created_at);",
    )?;
    log::info!("Migrated schema v14→v15: created trace_settings + idx_llm_traces_created");

    conn.execute(
        "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (15, ?1)",
        [chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// v15→v16: create the `eval_runs` table for B7 trajectory evaluation (one row
/// per scored session, supporting the daily regression-curve trend query).
/// Idempotent — `CREATE TABLE IF NOT EXISTS`.
pub fn migrate_v15_to_v16(conn: &Connection) -> Result<(), AppError> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if version >= 16 {
        return Ok(());
    }

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS eval_runs (
            id TEXT PRIMARY KEY,
            session_id TEXT,
            conversation_id TEXT,
            matcher TEXT NOT NULL,
            score REAL NOT NULL,
            grade TEXT NOT NULL,
            steps INTEGER NOT NULL,
            trajectory_json TEXT,
            reference_json TEXT,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_eval_runs_session ON eval_runs(session_id);
        CREATE INDEX IF NOT EXISTS idx_eval_runs_created ON eval_runs(created_at);",
    )?;
    log::info!("Migrated schema v15→v16: created eval_runs (B7 trajectory eval)");

    conn.execute(
        "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (16, ?1)",
        [chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// v16→v17: add `cache_read_tokens` + `cache_write_tokens` columns to
/// `cost_records` so the B5 transparent-cost dashboard can break spend down by
/// input / output / prompt-cache tiers. Idempotent — same probe-then-ALTER
/// shape as v10→v11/v11→v12: a fresh DB already has both columns from the
/// static SCHEMA and skips the ALTERs; a pre-v17 DB gets them added.
pub fn migrate_v16_to_v17(conn: &Connection) -> Result<(), AppError> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if version >= 17 {
        return Ok(());
    }

    // rusqlite has no ADD COLUMN IF NOT EXISTS; probe each column by preparing
    // a statement that references it (same idiom as v10→v11's blocks probe).
    let read_exists = conn
        .prepare("SELECT cache_read_tokens FROM cost_records LIMIT 0")
        .is_ok();
    if !read_exists {
        conn.execute(
            "ALTER TABLE cost_records ADD COLUMN cache_read_tokens INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    let write_exists = conn
        .prepare("SELECT cache_write_tokens FROM cost_records LIMIT 0")
        .is_ok();
    if !write_exists {
        conn.execute(
            "ALTER TABLE cost_records ADD COLUMN cache_write_tokens INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !read_exists || !write_exists {
        log::info!(
            "Migrated schema v16→v17: added cost_records cache token columns (B5 transparent cost)"
        );
    }

    conn.execute(
        "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (17, ?1)",
        [chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// v17→v18: add `ttfb_ms` + `stream_ms` columns to `llm_traces` so the B3
/// per-call timing breakdown (eino five-timing-points → derived intervals) can
/// be persisted and surfaced in TraceView. Same idempotent probe-then-ALTER
/// shape as v16→v17 / v10→v11: a fresh DB already has both columns from the
/// static SCHEMA and skips the ALTERs; a pre-v18 DB gets them added (NULL
/// default — legacy rows have no per-phase timing, which is honest).
pub fn migrate_v17_to_v18(conn: &Connection) -> Result<(), AppError> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if version >= 18 {
        return Ok(());
    }

    let ttfb_exists = conn.prepare("SELECT ttfb_ms FROM llm_traces LIMIT 0").is_ok();
    if !ttfb_exists {
        conn.execute("ALTER TABLE llm_traces ADD COLUMN ttfb_ms INTEGER", [])?;
    }
    let stream_exists = conn.prepare("SELECT stream_ms FROM llm_traces LIMIT 0").is_ok();
    if !stream_exists {
        conn.execute("ALTER TABLE llm_traces ADD COLUMN stream_ms INTEGER", [])?;
    }
    if !ttfb_exists || !stream_exists {
        log::info!(
            "Migrated schema v17→v18: added llm_traces timing columns (B3 ttfb/stream)"
        );
    }

    conn.execute(
        "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (18, ?1)",
        [chrono::Utc::now().to_rfc3339()],
    )?;
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
            blocks: None,
            task_ref: None,
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
    fn corrupt_v6_sessions_json_is_quarantined_not_fatal() {
        // A corrupt/truncated sessions.json must NOT bubble up from
        // migrate_v6_to_v7 — lib.rs calls it via .expect, so an error bricks
        // the app on every startup. The file is moved aside and the migration
        // completes with zero imports so the app can still start.
        let g = TempDb::new();
        let data_dir = g._tmp.path();
        let agents_dir = data_dir.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        let sessions = agents_dir.join("sessions.json");
        std::fs::write(&sessions, "{ this is not valid json").unwrap();

        // Must NOT return Err (which .expect would turn into a startup panic).
        migrate_v6_to_v7(&g.conn, data_dir).unwrap();

        // Corrupt file quarantined, not left in place to crash the next launch.
        assert!(!sessions.exists(), "corrupt sessions.json must be moved aside");
        assert!(
            agents_dir.join("sessions.json.corrupt").exists(),
            "corrupt file should be quarantined as .corrupt"
        );
        // Migration still marked done so it doesn't re-run / re-fail next start.
        assert!(db::is_v6_migrated(&g.conn));
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
    /// This builds that exact old-schema DB, re-opens it (which re-runs
    /// SCHEMA + migration the way lib.rs does), and asserts it survives with
    /// the column + index present and the existing session backfilled.
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
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='conversation_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_col, 1, "conversation_id column must be added");

        let conv_id: Option<String> = conn
            .query_row(
                "SELECT conversation_id FROM sessions WHERE id='legacy1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            conv_id.is_some(),
            "legacy session must be backfilled into a conversation"
        );

        // The index must exist (fresh-DB path used to skip it).
        let idx_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_sessions_conversation'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(idx_count, 1, "idx_sessions_conversation must be created");
    }

    /// A pre-v11 database has a `sessions` table WITHOUT the `blocks` column.
    /// This builds that exact old-schema DB (schema_version at 10), runs the
    /// v10→v11 migration, and asserts the column is added, existing data
    /// survives, and the version bumps to 11.
    #[test]
    fn migrate_v10_to_v11_adds_blocks_column_to_pre_v11_db() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("v10.db");

        // 1. Build a pre-v11 DB by hand: schema_version at 10, a sessions table
        //    WITHOUT the blocks column, and one real row.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);
                 INSERT INTO schema_version (version, applied_at) VALUES (10, '2026-01-01T00:00:00Z');
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
                     parent_session_id TEXT,
                     conversation_id TEXT
                 );
                 INSERT INTO sessions (id, project_path, agent_type, status, prompt, model,
                     started_at, finished_at, exit_code, output_summary, context_snapshot,
                     linked_requirement_id, parent_session_id, conversation_id)
                 VALUES ('legacy1', '/p', 'claude_code', 'completed', 'old prompt',
                     NULL, '2026-01-01T00:00:00Z', NULL, 0, NULL, NULL, NULL, NULL, 'conv-1');",
            )
            .unwrap();
        }

        // 2. Run v10→v11.
        let conn = Connection::open(&path).unwrap();
        migrate_v10_to_v11(&conn).expect("v10→v11 must succeed on a pre-v11 DB");

        // 3. blocks column added.
        let has_blocks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='blocks'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_blocks, 1, "blocks column must be added");

        // Existing data survives.
        let prompt: String = conn
            .query_row("SELECT prompt FROM sessions WHERE id='legacy1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            prompt, "old prompt",
            "existing data must survive the migration"
        );

        // Legacy session has no blocks (not backfilled — only newly finalized sessions write blocks).
        let blocks: Option<String> = conn
            .query_row("SELECT blocks FROM sessions WHERE id='legacy1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(blocks.is_none(), "legacy session blocks must be NULL");

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version),0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 11, "schema_version must be 11");
    }

    /// Idempotent: running twice must not double-ALTER (which would abort) and
    /// must leave exactly one `blocks` column.
    #[test]
    fn migrate_v10_to_v11_is_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("idem.db");
        let conn = db::init_db(&path).unwrap();
        migrate_v8_to_v9(&conn).unwrap();
        migrate_v9_to_v10(&conn).unwrap();
        migrate_v10_to_v11(&conn).expect("first run must succeed");
        // Second run short-circuits on version>=11 — no double ALTER, no error.
        migrate_v10_to_v11(&conn).expect("second run (idempotent) must succeed");
        let has_blocks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='blocks'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            has_blocks, 1,
            "column must exist exactly once after two runs"
        );
    }

    /// A fresh DB has `blocks` from the static SCHEMA already. The probe must
    /// skip the ALTER (ALTERing an existing column would abort) but still bump
    /// the version to 11.
    #[test]
    fn migrate_v10_to_v11_on_fresh_db_skips_alter_but_records_version() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("fresh.db");
        let conn = db::init_db(&path).unwrap();
        migrate_v8_to_v9(&conn).unwrap();
        migrate_v9_to_v10(&conn).unwrap();
        migrate_v10_to_v11(&conn).expect("fresh DB v11 migration must succeed");

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version),0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 11, "version must be 11 on fresh DB");
    }

    /// A pre-v12 database has a `sessions` table WITH the `blocks` column (added
    /// by v11) but WITHOUT the `task_ref` column. Build that exact old-schema DB
    /// (schema_version at 11), run v11→v12, assert the column is added, existing
    /// data survives, and the version bumps to 12.
    #[test]
    fn migrate_v11_to_v12_adds_task_ref_column_to_pre_v12_db() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("v11.db");

        // 1. Build a pre-v12 DB by hand: schema_version at 11, a sessions table
        //    WITH blocks (from v11) but WITHOUT task_ref, and one real row.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);
                 INSERT INTO schema_version (version, applied_at) VALUES (11, '2026-01-01T00:00:00Z');
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
                     parent_session_id TEXT,
                     conversation_id TEXT,
                     blocks TEXT
                 );
                 INSERT INTO sessions (id, project_path, agent_type, status, prompt, model,
                     started_at, finished_at, exit_code, output_summary, context_snapshot,
                     linked_requirement_id, parent_session_id, conversation_id, blocks)
                 VALUES ('legacy2', '/p', 'claude_code', 'completed', 'old prompt',
                     NULL, '2026-01-01T00:00:00Z', NULL, 0, NULL, NULL, NULL, NULL, 'conv-2', NULL);",
            )
            .unwrap();
        }

        // 2. Run v11→v12.
        let conn = Connection::open(&path).unwrap();
        migrate_v11_to_v12(&conn).expect("v11→v12 must succeed on a pre-v12 DB");

        // 3. task_ref column added.
        let has_task_ref: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='task_ref'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_task_ref, 1, "task_ref column must be added");

        // Existing data survives.
        let prompt: String = conn
            .query_row("SELECT prompt FROM sessions WHERE id='legacy2'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            prompt, "old prompt",
            "existing data must survive the migration"
        );

        // Legacy session has no task_ref (not backfilled — only bound sessions write it).
        let task_ref: Option<String> = conn
            .query_row(
                "SELECT task_ref FROM sessions WHERE id='legacy2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(task_ref.is_none(), "legacy session task_ref must be NULL");

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version),0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 12, "schema_version must be 12");
    }

    /// Idempotent: running twice must not double-ALTER (which would abort) and
    /// must leave exactly one `task_ref` column.
    #[test]
    fn migrate_v11_to_v12_is_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("idem12.db");
        let conn = db::init_db(&path).unwrap();
        migrate_v8_to_v9(&conn).unwrap();
        migrate_v9_to_v10(&conn).unwrap();
        migrate_v10_to_v11(&conn).expect("v11 must succeed");
        migrate_v11_to_v12(&conn).expect("first run must succeed");
        // Second run short-circuits on version>=12 — no double ALTER, no error.
        migrate_v11_to_v12(&conn).expect("second run (idempotent) must succeed");
        let has_task_ref: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name='task_ref'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            has_task_ref, 1,
            "column must exist exactly once after two runs"
        );
    }

    /// A fresh DB has `task_ref` from the static SCHEMA already. The probe must
    /// skip the ALTER (ALTERing an existing column would abort) but still bump
    /// the version to 12.
    #[test]
    fn migrate_v11_to_v12_on_fresh_db_skips_alter_but_records_version() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("fresh12.db");
        let conn = db::init_db(&path).unwrap();
        migrate_v8_to_v9(&conn).unwrap();
        migrate_v9_to_v10(&conn).unwrap();
        migrate_v10_to_v11(&conn).expect("v11 must succeed");
        migrate_v11_to_v12(&conn).expect("fresh DB v12 migration must succeed");

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version),0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 12, "version must be 12 on fresh DB");
    }

    /// v13→v14 creates the llm_traces table. On a fresh DB the table already
    /// exists (static SCHEMA) — the migration's CREATE is a no-op but it must
    /// still record version 14.
    #[test]
    fn migrate_v13_to_v14_on_fresh_db_creates_table_and_bumps_version() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("fresh14.db");
        let conn = db::init_db(&path).unwrap();
        migrate_v13_to_v14(&conn).expect("fresh DB v14 migration must succeed");

        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='llm_traces'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table_exists, 1, "llm_traces table must exist");

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version),0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 14, "version must be 14 on fresh DB");
    }

    /// Idempotent: running twice short-circuits on version>=14 (no error).
    #[test]
    fn migrate_v13_to_v14_is_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("idem14.db");
        let conn = db::init_db(&path).unwrap();
        migrate_v13_to_v14(&conn).expect("first run must succeed");
        migrate_v13_to_v14(&conn).expect("second run (idempotent) must succeed");
        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version),0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 14);
    }

    /// A pre-v14 DB (no llm_traces table, version pinned to 13) must get the
    /// table materialized by the migration. Simulated by dropping the table the
    /// static SCHEMA created and seeding version=13.
    #[test]
    fn migrate_v13_to_v14_creates_table_on_pre_v14_db() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("legacy14.db");
        let conn = db::init_db(&path).unwrap();
        conn.execute("DROP TABLE llm_traces", []).unwrap();
        conn.execute(
            "INSERT INTO schema_version (version, applied_at) VALUES (13, ?1)",
            [chrono::Utc::now().to_rfc3339()],
        )
        .unwrap();

        migrate_v13_to_v14(&conn).expect("pre-v14 migration must materialize the table");

        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='llm_traces'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table_exists, 1, "llm_traces must be created on pre-v14 DB");

        // Bring the table to the CURRENT shape before inserting: production
        // always runs the full v14→v18 chain, and v17→v18 is what ALTERs in the
        // timing columns insert_llm_trace now writes. v13→v14 alone stops at the
        // pre-timing shape, so the insert path is only meaningful against the
        // fully-migrated table that real DBs reach.
        migrate_v14_to_v15(&conn).unwrap();
        migrate_v15_to_v16(&conn).unwrap();
        migrate_v16_to_v17(&conn).unwrap();
        migrate_v17_to_v18(&conn).unwrap();

        // The insert path works end-to-end against the fully-migrated table.
        crate::trace::db::insert_llm_trace(
            &conn,
            &crate::trace::db::LlmTraceRow {
                id: "t1".into(),
                session_id: Some("s1".into()),
                conversation_id: None,
                model: "glm-4.6".into(),
                base_url: "https://x".into(),
                status_code: Some(400),
                error_kind: Some("non_2xx".into()),
                req_body: "{}".into(),
                resp_body: Some("boom".into()),
                latency_ms: Some(12),
                input_tokens: None,
                output_tokens: None,
                ttfb_ms: None,
                stream_ms: None,
                created_at: chrono::Utc::now().to_rfc3339(),
            },
        )
        .expect("insert must work on migrated table");
    }

    /// v14→v15 creates the trace_settings table + idx_llm_traces_created. On a
    /// fresh DB both already exist (static SCHEMA) — the migration's CREATE is a
    /// no-op but it must still record version 15 and leave a usable default row.
    #[test]
    fn migrate_v14_to_v15_on_fresh_db_creates_table_and_bumps_version() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("fresh15.db");
        let conn = db::init_db(&path).unwrap();
        migrate_v14_to_v15(&conn).expect("fresh DB v15 migration must succeed");

        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='trace_settings'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table_exists, 1, "trace_settings table must exist");

        // The default single row is present: id=1, NULL retention = infinite.
        let retention: Option<i64> = conn
            .query_row(
                "SELECT retention_days FROM trace_settings WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(retention, None, "default retention_days is NULL = infinite");

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version),0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 15, "version must be 15 on fresh DB");
    }

    /// Idempotent: running twice short-circuits on version>=15 (no error).
    #[test]
    fn migrate_v14_to_v15_is_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("idem15.db");
        let conn = db::init_db(&path).unwrap();
        migrate_v14_to_v15(&conn).expect("first run must succeed");
        migrate_v14_to_v15(&conn).expect("second run (idempotent) must succeed");
        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version),0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 15);
    }

    /// A pre-v15 DB (no trace_settings table, no idx_llm_traces_created, version
    /// pinned to 14) must get both materialized by the migration. Simulated by
    /// dropping the table+index the static SCHEMA created and seeding version=14.
    #[test]
    fn migrate_v14_to_v15_creates_table_and_index_on_pre_v15_db() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("legacy15.db");
        let conn = db::init_db(&path).unwrap();
        conn.execute("DROP TABLE trace_settings", []).unwrap();
        conn.execute("DROP INDEX IF EXISTS idx_llm_traces_created", [])
            .unwrap();
        conn.execute(
            "INSERT INTO schema_version (version, applied_at) VALUES (14, ?1)",
            [chrono::Utc::now().to_rfc3339()],
        )
        .unwrap();

        migrate_v14_to_v15(&conn).expect("pre-v15 migration must materialize table+index");

        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='trace_settings'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            table_exists, 1,
            "trace_settings must be created on pre-v15 DB"
        );

        let index_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_llm_traces_created'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            index_exists, 1,
            "idx_llm_traces_created must be created on pre-v15 DB"
        );

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version),0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 15, "version bumped to 15");
    }

    /// Build an in-memory pre-v17 DB: schema_version table pinned at 16, and a
    /// cost_records table WITHOUT the cache columns (the legacy shape). This is
    /// the state a real v16 DB would be in before B5 ran.
    fn legacy_v17_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);
             INSERT INTO schema_version (version, applied_at) VALUES (16, '2026-06-01T00:00:00Z');
             CREATE TABLE cost_records (
                 id TEXT PRIMARY KEY,
                 session_id TEXT,
                 agent_type TEXT NOT NULL,
                 model TEXT NOT NULL,
                 input_tokens INTEGER NOT NULL DEFAULT 0,
                 output_tokens INTEGER NOT NULL DEFAULT 0,
                 cost_usd REAL NOT NULL DEFAULT 0,
                 recorded_at TEXT NOT NULL
             );",
        )
        .unwrap();
        conn
    }

    fn col_count(conn: &Connection, table: &str, col: &str) -> i64 {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name='{col}'"),
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn migrate_v16_to_v17_adds_cache_columns_to_pre_v17_db() {
        let conn = legacy_v17_conn();
        // Pre-condition: legacy table has no cache columns.
        assert_eq!(col_count(&conn, "cost_records", "cache_read_tokens"), 0);
        assert_eq!(col_count(&conn, "cost_records", "cache_write_tokens"), 0);

        migrate_v16_to_v17(&conn).expect("pre-v17 migration must add cache columns");

        assert_eq!(col_count(&conn, "cost_records", "cache_read_tokens"), 1);
        assert_eq!(col_count(&conn, "cost_records", "cache_write_tokens"), 1);
        let version: i64 = conn
            .query_row("SELECT COALESCE(MAX(version),0) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 17, "version bumped to 17");
    }

    #[test]
    fn migrate_v16_to_v17_is_idempotent() {
        let conn = legacy_v17_conn();
        migrate_v16_to_v17(&conn).expect("first run");
        // Second run must not error on the now-present columns (version-gated
        // early-return makes it a no-op).
        migrate_v16_to_v17(&conn).expect("second run (idempotent)");
        assert_eq!(col_count(&conn, "cost_records", "cache_read_tokens"), 1);
        assert_eq!(col_count(&conn, "cost_records", "cache_write_tokens"), 1);
    }

    #[test]
    fn migrate_v16_to_v17_is_a_noop_on_fresh_db() {
        // A fresh DB (init_db already ran v17) has schema_version=17, so the
        // migration must skip without touching the table.
        let g = TempDb::new();
        // Fresh init already at v17 — re-running must be a clean no-op.
        migrate_v16_to_v17(&g.conn).expect("fresh DB v17 migration must be a no-op");
        assert_eq!(col_count(&g.conn, "cost_records", "cache_read_tokens"), 1);
        assert_eq!(col_count(&g.conn, "cost_records", "cache_write_tokens"), 1);
    }

    #[test]
    fn migrate_v16_to_v17_added_columns_default_zero_so_old_rows_aggregate() {
        // An ALTER ADD COLUMN with DEFAULT 0 must let a pre-existing row
        // aggregate cleanly (cache tiers read as 0). This pins the DEFAULT 0
        // contract the aggregate query relies on.
        let conn = legacy_v17_conn();
        conn.execute(
            "INSERT INTO cost_records (id, agent_type, model, input_tokens, output_tokens, cost_usd, recorded_at)
             VALUES ('r1', 'react_kernel', 'glm-4.6', 1000, 500, 0.0026, '2026-06-01T00:00:00Z')",
            [],
        )
        .unwrap();
        migrate_v16_to_v17(&conn).unwrap();
        let (cr, cw): (i64, i64) = conn
            .query_row(
                "SELECT cache_read_tokens, cache_write_tokens FROM cost_records WHERE id='r1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(cr, 0, "legacy row defaults to 0 cache_read");
        assert_eq!(cw, 0, "legacy row defaults to 0 cache_write");
    }
}
