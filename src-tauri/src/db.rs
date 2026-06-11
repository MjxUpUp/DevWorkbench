use crate::error::AppError;
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Managed state wrapping the SQLite connection.
/// Uses Arc so it can be cloned and passed to background threads (e.g. pty wait thread).
pub struct DbState(pub Arc<Mutex<Connection>>);

impl Clone for DbState {
    fn clone(&self) -> Self {
        DbState(self.0.clone())
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
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
CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_path);
CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);

CREATE TABLE IF NOT EXISTS requirements (
    id TEXT PRIMARY KEY,
    project_path TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'todo',
    priority TEXT,
    linked_session_id TEXT,
    artifacts TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_requirements_project ON requirements(project_path);

CREATE TABLE IF NOT EXISTS knowledge_entries (
    id TEXT PRIMARY KEY,
    project_hash TEXT NOT NULL,
    category TEXT NOT NULL,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    source_agent TEXT NOT NULL,
    source_session_id TEXT,
    source_type TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 0.5,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    access_count INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_knowledge_project ON knowledge_entries(project_hash);
CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_fts USING fts5(
    title, content,
    tokenize='unicode61'
);

CREATE TABLE IF NOT EXISTS activity_events (
    id TEXT PRIMARY KEY,
    project_hash TEXT NOT NULL,
    agent_type TEXT NOT NULL,
    event_type TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT,
    files_changed TEXT,
    session_id TEXT,
    timestamp TEXT NOT NULL,
    metadata TEXT
);
CREATE INDEX IF NOT EXISTS idx_activity_project ON activity_events(project_hash);
CREATE INDEX IF NOT EXISTS idx_activity_timestamp ON activity_events(timestamp);

CREATE TABLE IF NOT EXISTS quality_reports (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    checks TEXT NOT NULL,
    overall_status TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_quality_session ON quality_reports(session_id);

CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    path TEXT NOT NULL,
    tags TEXT NOT NULL DEFAULT '[]',
    cover_image TEXT,
    open_count INTEGER NOT NULL DEFAULT 0,
    last_opened_at TEXT,
    starred INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    last_opened_tools TEXT NOT NULL DEFAULT '[]',
    workspace_tools TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX IF NOT EXISTS idx_projects_path ON projects(path);

CREATE TABLE IF NOT EXISTS settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    scan_directories TEXT NOT NULL DEFAULT '[]',
    tool_paths TEXT NOT NULL DEFAULT '{}',
    theme TEXT NOT NULL DEFAULT 'obsidian',
    preferred_terminal TEXT NOT NULL DEFAULT '',
    cli_flags TEXT NOT NULL DEFAULT '{}'
);
";

/// Open (or create) the SQLite database at `db_path`, create all tables.
pub fn init_db(db_path: &Path) -> Result<Connection, AppError> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db_path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// Check whether data migration has already been applied (version >= 8).
pub fn is_migrated(conn: &Connection) -> bool {
    match conn.query_row(
        "SELECT COUNT(*) FROM schema_version WHERE version >= 8",
        [],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(count) => count > 0,
        Err(_) => false,
    }
}
