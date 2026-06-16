use crate::error::AppError;
use rusqlite::Connection;
use std::collections::VecDeque;
use std::ops::Deref;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};

/// Pool capacity. SQLite WAL allows concurrent readers + one writer; this pool
/// removes the global-lock contention that previously serialized all DB access.
const POOL_SIZE: usize = 8;
/// How long `get()` blocks waiting for a connection before erroring.
const GET_TIMEOUT_SECS: u64 = 30;

/// Internal pool state: the idle connections + how many are checked out (for
/// replenishment accounting). Shared behind a Mutex + Condvar pair so `get()`
/// can block until a connection is returned.
struct PoolInner {
    idle: VecDeque<Connection>,
    /// Total connections ever handed out (idle + in-flight). Capped at POOL_SIZE.
    in_use: usize,
    db_path: std::path::PathBuf,
}

pub struct Pool {
    inner: Mutex<PoolInner>,
    cvar: Condvar,
}

/// A pooled SQLite connection. Returns to the pool on drop; `Deref`s to
/// `rusqlite::Connection` so existing call sites (`conn.execute(...)`,
/// `conn.query_*`) work unchanged.
pub struct PooledConn {
    conn: Option<Connection>,
    pool: Arc<Pool>,
}

impl Deref for PooledConn {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn.as_ref().expect("pooled conn used after return")
    }
}

impl Drop for PooledConn {
    fn drop(&mut self) {
        if let Some(c) = self.conn.take() {
            // Poison-tolerant: force-recover the lock if a thread panicked.
            let mut guard = self.pool.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.idle.push_back(c);
            if guard.in_use > 0 {
                guard.in_use -= 1;
            }
            drop(guard);
            // Wake one waiter.
            self.pool.cvar.notify_one();
        }
    }
}

type PoolError = String;

/// Managed state wrapping a SQLite connection pool with blocking `get()`.
///
/// Previously held a single `Arc<Mutex<Connection>>` (serialized all access) or
/// a bare `VecDeque` (exhausted -> immediate error). Now uses a Condvar so
/// `get()` blocks up to GET_TIMEOUT_SECS for a free connection, and replenishes
/// a connection if the pool was drained (capped at POOL_SIZE).
pub struct DbState(pub Arc<Pool>);

impl Clone for DbState {
    fn clone(&self) -> Self {
        DbState(self.0.clone())
    }
}

impl DbState {
    /// Open a pool of `POOL_SIZE` connections, running schema + pragmas on each.
    pub fn open(db_path: &Path) -> Result<Self, AppError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut idle = VecDeque::with_capacity(POOL_SIZE);
        for _ in 0..POOL_SIZE {
            let conn = Self::make_conn(db_path)?;
            idle.push_back(conn);
        }
        Ok(DbState(Arc::new(Pool {
            inner: Mutex::new(PoolInner {
                idle,
                in_use: 0,
                db_path: db_path.to_path_buf(),
            }),
            cvar: Condvar::new(),
        })))
    }

    fn make_conn(db_path: &Path) -> Result<Connection, AppError> {
        let conn = Connection::open(db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(conn)
    }

    /// Take a connection. Blocks up to GET_TIMEOUT_SECS for a free one; if the
    /// pool is below capacity, opens a fresh connection (replenishment). Errors
    /// only if the timeout elapses with nothing available.
    pub fn get(&self) -> Result<PooledConn, PoolError> {
        let mut guard = self
            .0
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(GET_TIMEOUT_SECS);
        loop {
            if let Some(c) = guard.idle.pop_front() {
                guard.in_use += 1;
                return Ok(PooledConn { conn: Some(c), pool: self.0.clone() });
            }
            // Replenish: if total (idle+in_use) < POOL_SIZE, open a new conn.
            if guard.in_use + guard.idle.len() < POOL_SIZE {
                let path = guard.db_path.clone();
                // Release lock while opening (I/O).
                drop(guard);
                let conn = Self::make_conn(&path).map_err(|e| format!("replenish: {e}"))?;
                guard = self.0.inner.lock().unwrap_or_else(|e| e.into_inner());
                guard.in_use += 1;
                return Ok(PooledConn { conn: Some(conn), pool: self.0.clone() });
            }
            // Wait for a connection to be returned.
            let g = self.0.cvar
                .wait_timeout(guard, std::time::Duration::from_secs(1))
                .unwrap_or_else(|e| e.into_inner()).0;
            guard = g;
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "DB pool exhausted after {}s ({} in use)",
                    GET_TIMEOUT_SECS, guard.in_use
                ));
            }
        }
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
    parent_session_id TEXT,
    conversation_id TEXT,
    blocks TEXT
);
CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_path);
CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);
-- NOTE: idx_sessions_conversation is NOT in the static SCHEMA. The
-- conversation_id column is added by migrate_v9_to_v10 via ALTER TABLE; on a
-- pre-v10 database the sessions table exists WITHOUT that column, so a
-- CREATE INDEX ... ON sessions(conversation_id) here would run BEFORE the
-- migration (make_conn runs SCHEMA first, migrations second) and abort the
-- whole execute_batch with `no such column: conversation_id` -- crashing the
-- app on every launch of an upgraded install. The index is created in the
-- migration (and guarded there for fresh DBs too).
-- Conversation = multi-turn dialogue container (= a Claude Code session). Holds
-- N sessions (turns), possibly by different agents. Built by migrate_v9_to_v10
-- from existing parent_session_id chains; new turns attach via spawn.
CREATE TABLE IF NOT EXISTS conversations (
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
CREATE INDEX IF NOT EXISTS idx_conversations_last_activity ON conversations(last_activity_at DESC);

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
    theme TEXT NOT NULL DEFAULT 'auto',
    preferred_terminal TEXT NOT NULL DEFAULT '',
    cli_flags TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS workflows (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    yaml_content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Note: workflow_runs / workflow_steps tables removed — never written to.
-- Execution state is stream-based (GraphEvent via kernel-compose).

CREATE TABLE IF NOT EXISTS skills (
    id TEXT PRIMARY KEY,
    org TEXT NOT NULL,
    name TEXT NOT NULL,
    version TEXT,
    installed_at TEXT,
    path TEXT,
    quality_score REAL,
    metadata TEXT
);

CREATE TABLE IF NOT EXISTS skill_reports (
    id TEXT PRIMARY KEY,
    skill_id TEXT NOT NULL,
    scan_result TEXT NOT NULL,
    scanned_at TEXT NOT NULL,
    FOREIGN KEY (skill_id) REFERENCES skills(id)
);
CREATE INDEX IF NOT EXISTS idx_skill_reports_skill ON skill_reports(skill_id);

CREATE TABLE IF NOT EXISTS cost_records (
    id TEXT PRIMARY KEY,
    session_id TEXT,
    agent_type TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL NOT NULL DEFAULT 0,
    recorded_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cost_records_agent ON cost_records(agent_type);
CREATE INDEX IF NOT EXISTS idx_cost_records_recorded ON cost_records(recorded_at);

CREATE TABLE IF NOT EXISTS budget_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    monthly_budget_usd REAL,
    alert_threshold REAL DEFAULT 0.8,
    updated_at TEXT NOT NULL
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

/// Check whether the v0.6→v0.7 sessions migration has been applied (version >= 7).
///
/// This is separate from `is_migrated` (>= 8) because migrate_v6_to_v7 runs
/// *before* migrate_v7_to_v8 writes version=8. Using is_migrated as the v6
/// guard made v6_to_v7 short-circuit once v7_to_v8 had run, so sessions.json
/// was never imported — silently dropping all v0.6 conversation history.
pub fn is_v6_migrated(conn: &Connection) -> bool {
    match conn.query_row(
        "SELECT COUNT(*) FROM schema_version WHERE version >= 7",
        [],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(count) => count > 0,
        Err(_) => false,
    }
}
