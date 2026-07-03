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
        let mut guard = self.0.inner.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(GET_TIMEOUT_SECS);
        loop {
            if let Some(c) = guard.idle.pop_front() {
                guard.in_use += 1;
                return Ok(PooledConn {
                    conn: Some(c),
                    pool: self.0.clone(),
                });
            }
            // Replenish: if total (idle+in_use) < POOL_SIZE, open a new conn.
            if guard.in_use + guard.idle.len() < POOL_SIZE {
                let path = guard.db_path.clone();
                // Release lock while opening (I/O).
                drop(guard);
                let conn = Self::make_conn(&path).map_err(|e| format!("replenish: {e}"))?;
                guard = self.0.inner.lock().unwrap_or_else(|e| e.into_inner());
                // Re-check the cap after re-locking: another thread may have
                // replenished while we held no lock during the open. If so,
                // close the extra connection and fall through to wait, rather
                // than bursting past POOL_SIZE. The wasted open only happens on
                // this rare race, never on the steady-state path.
                if guard.in_use + guard.idle.len() < POOL_SIZE {
                    guard.in_use += 1;
                    return Ok(PooledConn {
                        conn: Some(conn),
                        pool: self.0.clone(),
                    });
                }
                drop(conn);
            }
            // Wait for a connection to be returned.
            let g = self
                .0
                .cvar
                .wait_timeout(guard, std::time::Duration::from_secs(1))
                .unwrap_or_else(|e| e.into_inner())
                .0;
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
    blocks TEXT,
    task_ref TEXT
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
    palette TEXT NOT NULL DEFAULT 'pi',
    preferred_terminal TEXT NOT NULL DEFAULT '',
    cli_flags TEXT NOT NULL DEFAULT '{}',
    onboarding_completed INTEGER NOT NULL DEFAULT 0
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

CREATE TABLE IF NOT EXISTS slash_commands (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    template TEXT NOT NULL,
    category TEXT,
    created_at TEXT NOT NULL
);
-- Seed the four built-in slash commands. Idempotent: INSERT OR IGNORE on the
-- UNIQUE name + fixed ids means re-running SCHEMA on every launch is a no-op
-- once they exist; a user editing a built-in later is NOT overwritten.
-- Multi-line string literals store real newlines (SQLite supports them).
INSERT OR IGNORE INTO slash_commands (id, name, description, template, category, created_at) VALUES
    ('builtin-plan', 'plan', '计划模式 — 先输出计划再执行', '请先制定计划，确认后再执行。

需求：$ARGUMENTS', 'builtin', '2026-06-18T00:00:00Z'),
    ('builtin-review', 'review', '代码审查', '请审查以下代码变更，重点关注：正确性、安全性、性能、可读性。逐条给出意见，不要泛泛而谈。

审查范围：$ARGUMENTS', 'builtin', '2026-06-18T00:00:00Z'),
    ('builtin-test', 'test', '运行测试', '请运行测试套件并报告结果。如果有失败，先定位根因再修复——禁止弱化断言（t.Fatal→t.Log、放宽条件、t.Skip）。

目标：$ARGUMENTS', 'builtin', '2026-06-18T00:00:00Z'),
    ('builtin-fix', 'fix', '修复问题', '请修复以下问题。先定位根因，不要掩盖症状。

问题：$ARGUMENTS', 'builtin', '2026-06-18T00:00:00Z');

-- D2 user-configurable lifecycle hooks (claude-code command-hook analog). Each
-- row is one shell command bound to a lifecycle event; build_react_agent loads
-- the enabled rows and registers a UserCommandHook per row. `event` is one of
-- 'user_prompt_submit' | 'stop'. `shell` 1 = run via `sh -c` (default, matches
-- claude-code), 0 = exec the command directly. IF NOT EXISTS makes this safe to
-- re-apply on every launch (same idempotent pattern as slash_commands above).
CREATE TABLE IF NOT EXISTS user_hooks (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    event TEXT NOT NULL,
    command TEXT NOT NULL,
    shell INTEGER NOT NULL DEFAULT 1,
    timeout_secs INTEGER NOT NULL DEFAULT 30,
    enabled INTEGER NOT NULL DEFAULT 1,
    -- Optional tool-name matcher (claude-code `matcher`), meaningful only for
    -- pre_tool_use / post_tool_use. NULL = match all. v12→v13 column.
    matcher TEXT,
    created_at TEXT NOT NULL
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
    -- B5 transparent cost: prompt-cache token tiers (Anthropic
    -- cache_read_input_tokens / cache_creation_input_tokens). Default 0 so the
    -- column exists for pre-v17 rows and for providers that don't report
    -- cache usage. Added by migrate_v16_to_v17 on existing DBs; present in
    -- CREATE here for fresh DBs.
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
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

-- LLM call traces — one row per ChatModel HTTP request. The observability
-- layer: persists the request body, HTTP status, and (on error) the response
-- body so a failed session's root cause is always queryable. Before this,
-- a non-2xx was compressed to a bare status string and the error body
-- was discarded. session_id is the per-turn key (driver passes it through
-- build_react_agent); conversation_id is redundant for cross-turn aggregation.
CREATE TABLE IF NOT EXISTS llm_traces (
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
    -- B3 per-call timing breakdown (eino five-timing-points model → derived
    -- intervals). ttfb_ms = request-send → first response signal (model
    -- thinking time); stream_ms = first-byte → completion (output time). NULL
    -- when the phase never happened (pure network failure, or pre-B3 rows).
    -- Added by migrate_v17_to_v18 on existing DBs; present in CREATE for fresh.
    ttfb_ms INTEGER,
    stream_ms INTEGER,
    -- A1 (OTel span tree): one span per agent instance. span_id identifies the
    -- agent that issued the call; parent_span_id is the orchestrating agent's
    -- span (NULL for the root). TraceView groups calls by span_id and renders
    -- the agent-DAG nesting. NULL for pre-v22 rows and ad-hoc/test agents.
    span_id TEXT,
    parent_span_id TEXT,
    span_name TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_llm_traces_session ON llm_traces(session_id, created_at);
-- created_at single-column index speeds the retention prune (DELETE WHERE
-- created_at < cutoff); without it the prune full-scans llm_traces.
CREATE INDEX IF NOT EXISTS idx_llm_traces_created ON llm_traces(created_at);

-- Trace retention settings (single-row table, mirrors budget_settings). NULL or
-- 0 retention_days = infinite (the default — Phoenix's
-- PHOENIX_DEFAULT_RETENTION_POLICY_DAYS=0 semantics, per the 2026-06-19 trace
-- observability research); a positive N prunes traces older than N days on
-- startup. last_vacuum_at throttles VACUUM to at most weekly (SQLite does not
-- reclaim disk after DELETE without it).
CREATE TABLE IF NOT EXISTS trace_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    retention_days INTEGER,
    last_vacuum_at TEXT,
    updated_at TEXT NOT NULL
);
INSERT OR IGNORE INTO trace_settings (id, retention_days, last_vacuum_at, updated_at)
VALUES (1, NULL, NULL, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));

-- B7 trajectory-eval runs (one row per scored session). score in [0,1], grade
-- in {optimal, suboptimal, incorrect}; trajectory_json/reference_json are the
-- full snapshots for replay. Backs the regression-curve trend query.
CREATE TABLE IF NOT EXISTS eval_runs (
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
CREATE INDEX IF NOT EXISTS idx_eval_runs_created ON eval_runs(created_at);

-- L1 verdict ledger — one row per gate/circuit verdict emitted during an agent
-- run or a platform eval. `gate` ∈ {verify, honesty, forge, circuit-breaker,
-- eval}; `verdict` ∈ {PASS, FAIL, TRIPPED, RESET, SKIPPED, ...}. `attribution`
-- encodes the anti-gaming stance (反刷分三原则): a gain with no verifiable
-- causal chain lands as BRAKE (unattributed = brake), not as a win — so a
-- passing run that cannot show its work is still flagged. `report` holds the
-- gate's detail JSON (honesty findings / forge score / verify rubric verdict /
-- circuit host+thresholds). `commit_sha` ties a verdict to the platform
-- version under test (platform-eval + paired-replay). `case_id` is populated
-- only by L2 eval runs (replay against a stored case); ad-hoc gate verdicts
-- leave it NULL.
CREATE TABLE IF NOT EXISTS verdicts (
    id TEXT PRIMARY KEY,
    session_id TEXT,
    case_id TEXT,
    gate TEXT NOT NULL,
    verdict TEXT NOT NULL,
    attribution TEXT,
    report TEXT,
    commit_sha TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_verdicts_session ON verdicts(session_id);
CREATE INDEX IF NOT EXISTS idx_verdicts_gate ON verdicts(gate);
CREATE INDEX IF NOT EXISTS idx_verdicts_created ON verdicts(created_at);

-- L2 eval cases — the deterministic contract a replay (L3) or paired comparison
-- (L4) runs against. `category` ∈ {agent, platform-mechanism, platform-e2e,
-- platform-boost} (the four eval targets, P4). 反刷分三原则 #1 (客观事实代码判):
-- the expected_* fields are deterministic facts extracted from a real past run,
-- not LLM-generated; an LLM is used only to judge `expected_output`, never to
-- invent the target. `draft` = 1 marks a case auto-built straight off a
-- trajectory (expected_steps frozen from extract_trajectory) but NOT yet
-- independently reviewed — a draft cannot anchor a paired replay, so an agent
-- can't self-certify whatever-it-did as the answer. `source_session_id` ties a
-- draft back to the real run it was frozen from (traceability, anti-drift).
-- `negative_json` holds counter-examples (steps/output that must NOT happen) —
-- the anti-gaming guard against right-steps-wrong-outcome刷分.
CREATE TABLE IF NOT EXISTS eval_cases (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    category TEXT NOT NULL,
    input_prompt TEXT NOT NULL,
    expected_steps_json TEXT,
    expected_output TEXT,
    expected_observables_json TEXT,
    negative_json TEXT,
    source_session_id TEXT,
    commit_sha TEXT,
    draft INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_eval_cases_category ON eval_cases(category);
CREATE INDEX IF NOT EXISTS idx_eval_cases_draft ON eval_cases(draft);
CREATE INDEX IF NOT EXISTS idx_eval_cases_created ON eval_cases(created_at);
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

/// Check whether the v0.6→v0.7 sessions migration has been applied (version >= 7).
///
/// Reads version >= 7 from schema_version. migrate_v6_to_v7 runs *before*
/// migrate_v7_to_v8 writes version=8, so this v6 guard must key off 7, not 8 —
/// keying off 8 made v6_to_v7 short-circuit once v7_to_v8 had run, so
/// sessions.json was never imported (silently dropping v0.6 history).
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
