/// Integration tests for the v0.6→v0.7 sessions migration.
///
/// This migration had two silent bugs that dropped all v0.6 conversation
/// history:
///   1. The idempotency guard used `is_migrated` (version >= 8), which was
///      already true once migrate_v7_to_v8 ran, so this function short-circuited.
///   2. The body opened a transaction but never committed it, so even when it
///      ran the imported rows were rolled back.
///
/// Plus a recovery concern: an earlier buggy run could have already renamed
/// `sessions.json` → `sessions.json.v0.6.bak` while leaving the DB empty, so
/// recovery must read the `.bak`.
use app_lib::db;
use app_lib::migrate;
use app_lib::models::{AgentType, Session, SessionStatus};
use rusqlite::Connection;
use std::fs;
use tempfile::TempDir;

fn make_session(id: &str) -> Session {
    Session {
        id: id.to_string(),
        project_path: "E:/DevWorkbench".to_string(),
        agent_type: AgentType::ReactKernel,
        status: SessionStatus::Completed,
        prompt: "test prompt".to_string(),
        model: None,
        started_at: "2026-06-08T21:50:14+08:00".to_string(),
        finished_at: Some("2026-06-08T21:52:14+08:00".to_string()),
        exit_code: Some(0),
        output_summary: None,
        context_snapshot: None,
        linked_requirement_id: None,
        parent_session_id: None,
        conversation_id: None,
        blocks: None,
        task_ref: None,
    }
}

fn write_agents_file(data_dir: &std::path::Path, name: &str, sessions: &[Session]) {
    let agents = data_dir.join("agents");
    fs::create_dir_all(&agents).unwrap();
    let json = serde_json::to_string_pretty(sessions).unwrap();
    fs::write(agents.join(name), json).unwrap();
}

fn fresh_db(db_path: &std::path::Path) -> Connection {
    db::init_db(db_path).expect("init_db failed")
}

fn count_sessions(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn imports_from_live_sessions_json_and_commits() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let conn = fresh_db(&db_path);
    write_agents_file(
        tmp.path(),
        "sessions.json",
        &[make_session("s1"), make_session("s2")],
    );

    migrate::migrate_v6_to_v7(&conn, tmp.path()).expect("migration failed");

    // Re-open on a FRESH connection to prove the transaction committed (the
    // original bug never called tx.commit(), so rows vanished on rollback).
    let conn2 = Connection::open(&db_path).unwrap();
    assert_eq!(
        count_sessions(&conn2),
        2,
        "imported rows must survive a new connection"
    );
    assert!(
        db::is_v6_migrated(&conn2),
        "version>=7 marker must be written"
    );

    // Live file renamed to .bak after a successful commit.
    assert!(
        !tmp.path().join("agents/sessions.json").exists(),
        "live sessions.json should be renamed after commit"
    );
    assert!(
        tmp.path().join("agents/sessions.json.v0.6.bak").exists(),
        "backup file should exist"
    );
}

#[test]
fn falls_back_to_bak_when_live_already_renamed() {
    // Reproduces the post-bug state: an earlier run renamed sessions.json to
    // .bak but (never having committed) left the DB empty. Recovery must read
    // the .bak and import it WITHOUT clobbering the only surviving copy.
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let conn = fresh_db(&db_path);
    write_agents_file(
        tmp.path(),
        "sessions.json.v0.6.bak",
        &[make_session("recovered-1")],
    );

    migrate::migrate_v6_to_v7(&conn, tmp.path()).expect("migration failed");

    let conn2 = Connection::open(&db_path).unwrap();
    assert_eq!(count_sessions(&conn2), 1, "must recover rows from the .bak");
    // The .bak must still exist (not renamed onto itself).
    assert!(
        tmp.path().join("agents/sessions.json.v0.6.bak").exists(),
        "backup file must be preserved, not clobbered"
    );
}

#[test]
fn idempotent_second_run_does_not_duplicate() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let conn = fresh_db(&db_path);
    write_agents_file(tmp.path(), "sessions.json", &[make_session("once")]);

    migrate::migrate_v6_to_v7(&conn, tmp.path()).unwrap();
    // Second run: the guard (version >= 7) must short-circuit and not re-import.
    migrate::migrate_v6_to_v7(&conn, tmp.path()).unwrap();

    let conn2 = Connection::open(&db_path).unwrap();
    assert_eq!(
        count_sessions(&conn2),
        1,
        "second run must not duplicate rows"
    );
}

#[test]
fn skips_when_already_marked_v6_migrated() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let conn = fresh_db(&db_path);
    // Pre-mark v6 migration done (version = 7) to simulate an already-migrated install.
    conn.execute(
        "INSERT INTO schema_version (version, applied_at) VALUES (7, ?1)",
        [chrono::Utc::now().to_rfc3339()],
    )
    .unwrap();
    write_agents_file(
        tmp.path(),
        "sessions.json",
        &[make_session("should-not-import")],
    );

    migrate::migrate_v6_to_v7(&conn, tmp.path()).unwrap();

    assert_eq!(
        count_sessions(&conn),
        0,
        "must skip when version>=7 already set"
    );
    // Live file must be untouched (the guard returned before any rename).
    assert!(
        tmp.path().join("agents/sessions.json").exists(),
        "live file must not be renamed when migration is skipped"
    );
}

#[test]
fn no_source_files_is_noop() {
    // An install that never had v0.6 data: no sessions.json, no .bak. Migration
    // must succeed (no panic) and record the version marker without inserting rows.
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let conn = fresh_db(&db_path);

    migrate::migrate_v6_to_v7(&conn, tmp.path()).expect("migration should be a no-op success");

    assert_eq!(count_sessions(&conn), 0);
    assert!(db::is_v6_migrated(&conn), "version marker still recorded");
}
