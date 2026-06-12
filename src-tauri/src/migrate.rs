use crate::db;
use crate::error::AppError;
use crate::models::{AppSettings, Project, Requirement, Session};
use rusqlite::Connection;
use std::fs;
use std::path::Path;

/// Migrate v0.6 JSON data to v0.7 SQLite.
///
/// - Idempotent: checks `schema_version` first, skips if already migrated.
/// - Transactional: wraps all inserts in a transaction; on failure the JSON
///   files are left untouched.
/// - Backup: renames original files to `.v0.6.bak` on success.
pub fn migrate_v6_to_v7(conn: &Connection, data_dir: &Path) -> Result<(), AppError> {
    if db::is_migrated(conn) {
        return Ok(());
    }

    let agents_dir = data_dir.join("agents");

    // Begin transaction — all-or-nothing
    let tx = conn.unchecked_transaction()?;

    // 1. Migrate sessions
    let sessions_file = agents_dir.join("sessions.json");
    if sessions_file.exists() {
        let content = fs::read_to_string(&sessions_file)?;
        if !content.trim().is_empty() {
            let sessions: Vec<Session> = serde_json::from_str(&content)?;
            for s in &sessions {
                insert_session(&tx, s)?;
            }
        }
    }

    // 2. Migrate requirements
    let reqs_file = agents_dir.join("requirements.json");
    if reqs_file.exists() {
        let content = fs::read_to_string(&reqs_file)?;
        if !content.trim().is_empty() {
            let reqs: Vec<Requirement> = serde_json::from_str(&content)?;
            for r in &reqs {
                insert_requirement(&tx, r)?;
            }
        }
    }

    // 3. Mark migration done
    tx.execute(
        "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (7, ?1)",
        [chrono::Utc::now().to_rfc3339()],
    )?;

    tx.commit()?;

    // 4. Backup original files (post-commit, best-effort)
    if sessions_file.exists() {
        let _ = fs::rename(&sessions_file, agents_dir.join("sessions.json.v0.6.bak"));
    }
    if reqs_file.exists() {
        let _ = fs::rename(&reqs_file, agents_dir.join("requirements.json.v0.6.bak"));
    }

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

fn insert_requirement(conn: &Connection, r: &Requirement) -> Result<(), AppError> {
    let artifacts_json = serde_json::to_string(&r.artifacts).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        "INSERT OR IGNORE INTO requirements
            (id, project_path, title, description, status, priority,
             linked_session_id, artifacts, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            r.id,
            r.project_path,
            r.title,
            r.description,
            r.status.as_str(),
            r.priority,
            r.linked_session_id,
            artifacts_json,
            r.created_at,
            r.updated_at,
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
