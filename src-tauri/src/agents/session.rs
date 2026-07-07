use crate::agents::react_chat::validate_block_pairs;
use crate::error::AppError;
use crate::models::{AgentType, ContextSnapshot, Conversation, Session, SessionStatus};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// A flat branch-tree node: one turn + its parent pointer. The frontend groups
/// these by `parent_session_id` to render the branch switcher (a turn that was
/// edited-and-regenerated forks a sibling under the SAME parent; the switcher
/// walks between siblings). Kept as a query DTO here so `models.rs` stays free
/// of presentation-shaped types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchNode {
    pub id: String,
    pub parent_session_id: Option<String>,
    pub prompt: String,
    pub status: String,
    pub started_at: String,
    pub agent_type: String,
}

// Thread-local override for database path (test isolation).
#[cfg(test)]
std::thread_local! {
    pub(crate) static TEST_DB_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Get a connection for tests (bypasses Tauri state).
#[cfg(test)]
fn test_conn() -> rusqlite::Connection {
    TEST_DB_PATH_OVERRIDE.with(|cell| {
        let path = cell.borrow().clone().expect("TEST_DB_PATH_OVERRIDE not set — use TempDb guard");
        rusqlite::Connection::open(&path).expect("failed to open test DB")
    })
}

pub fn load_sessions_from_db(conn: &rusqlite::Connection) -> Result<Vec<Session>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, project_path, agent_type, status, prompt, model,
                started_at, finished_at, exit_code, output_summary,
                context_snapshot, linked_requirement_id, parent_session_id,
                conversation_id, blocks, task_ref
         FROM sessions ORDER BY started_at DESC"
    )?;

    let sessions = stmt.query_map([], |row| {
        let agent_type_str: String = row.get(2)?;
        let agent_type: AgentType = serde_json::from_value(serde_json::Value::String(agent_type_str))
            .unwrap_or(AgentType::ClaudeCode);

        let status_str: String = row.get(3)?;
        let status = match status_str.as_str() {
            "running" => SessionStatus::Running,
            "completed" => SessionStatus::Completed,
            "failed" => SessionStatus::Failed,
            "cancelled" => SessionStatus::Cancelled,
            _ => SessionStatus::Failed,
        };

        let snapshot_str: Option<String> = row.get(10)?;
        let context_snapshot: Option<ContextSnapshot> = snapshot_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        let blocks_str: Option<String> = row.get(14)?;
        let blocks: Option<serde_json::Value> = blocks_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
        let task_ref: Option<String> = row.get(15)?;

        Ok(Session {
            id: row.get(0)?,
            project_path: row.get(1)?,
            agent_type,
            status,
            prompt: row.get(4)?,
            model: row.get(5)?,
            started_at: row.get(6)?,
            finished_at: row.get(7)?,
            exit_code: row.get(8)?,
            output_summary: row.get(9)?,
            context_snapshot,
            linked_requirement_id: row.get(11)?,
            parent_session_id: row.get(12)?,
            conversation_id: row.get(13)?,
            blocks,
            task_ref,
        })
    })?;

    let mut result = Vec::new();
    for s in sessions {
        result.push(s?);
    }

    // Reconcile stale running sessions
    let mut dirty = false;
    let now = chrono::Local::now();
    for s in &mut result {
        if s.status == SessionStatus::Running {
            if let Ok(started) = chrono::DateTime::parse_from_rfc3339(&s.started_at) {
                let started_local = started.with_timezone(&chrono::Local);
                if (now - started_local).num_minutes() > 10 {
                    s.status = SessionStatus::Failed;
                    s.finished_at = Some(now.to_rfc3339());
                    s.exit_code = Some(-1);
                    s.output_summary = Some("Session was interrupted (app restart)".to_string());
                    dirty = true;
                }
            }
        }
    }
    if dirty {
        // Update stale sessions in DB
        for s in &result {
            if s.status == SessionStatus::Failed && s.exit_code == Some(-1) {
                let _ = conn.execute(
                    "UPDATE sessions SET status = 'failed', finished_at = ?1, exit_code = -1, output_summary = 'Session was interrupted (app restart)' WHERE id = ?2",
                    params![s.finished_at, s.id],
                );
            }
        }
    }

    Ok(result)
}

pub fn insert_session_db(conn: &rusqlite::Connection, s: &Session) -> Result<(), AppError> {
    let snapshot_json = s.context_snapshot.as_ref().map(|cs| serde_json::to_string(cs).unwrap_or_default());
    conn.execute(
        "INSERT OR IGNORE INTO sessions
            (id, project_path, agent_type, status, prompt, model,
             started_at, finished_at, exit_code, output_summary,
             context_snapshot, linked_requirement_id, parent_session_id,
             conversation_id, task_ref)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
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
            s.conversation_id,
            s.task_ref,
        ],
    )?;
    Ok(())
}

/// Returns the number of rows affected. A terminal status (completed/failed/
/// cancelled) is write-once: the WHERE clause carries `AND status = 'running'`
/// so a racing second writer (e.g. the user clicking stop just as the agent
/// finishes naturally) flips 0 rows instead of clobbering the winner. Callers
/// use the row count to tell "I won the race" (>0) from "nothing to do" (0 —
/// either the id is absent or the session was already terminal) and skip the
/// duplicate `agent:completed` emit in the latter case. Err is reserved for DB
/// failures / an invalid status.
pub fn update_session_db(conn: &rusqlite::Connection, id: &str, patch: serde_json::Value) -> Result<usize, AppError> {
    // Build SET clause dynamically based on provided fields
    let mut set_clauses: Vec<String> = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut terminal_status = false;

    if let Some(status) = patch.get("status").and_then(|v| v.as_str()) {
        let validated = match status {
            "running" => "running",
            "completed" => "completed",
            "failed" => "failed",
            "cancelled" => "cancelled",
            _ => return Err(AppError::Agent(format!("无效 status: {}", status))),
        };
        terminal_status = matches!(validated, "completed" | "failed" | "cancelled");
        set_clauses.push("status = ?".to_string());
        param_values.push(Box::new(validated.to_string()));
    }
    if let Some(exit_code) = patch.get("exitCode").or_else(|| patch.get("exit_code")).and_then(|v| v.as_i64()) {
        set_clauses.push("exit_code = ?".to_string());
        param_values.push(Box::new(exit_code as i32));
    }
    if let Some(finished_at) = patch.get("finishedAt").or_else(|| patch.get("finished_at")).and_then(|v| v.as_str()) {
        set_clauses.push("finished_at = ?".to_string());
        param_values.push(Box::new(finished_at.to_string()));
    }
    if let Some(summary) = patch.get("outputSummary").or_else(|| patch.get("output_summary")).and_then(|v| v.as_str()) {
        set_clauses.push("output_summary = ?".to_string());
        param_values.push(Box::new(summary.to_string()));
    }
    if let Some(snap) = patch.get("contextSnapshot").or_else(|| patch.get("context_snapshot")) {
        let snap_json = serde_json::to_string(snap).unwrap_or_default();
        set_clauses.push("context_snapshot = ?".to_string());
        param_values.push(Box::new(snap_json));
    }
    if let Some(conv) = patch.get("conversationId").or_else(|| patch.get("conversation_id")) {
        set_clauses.push("conversation_id = ?".to_string());
        // null → unset the conversation; otherwise the conversation id string.
        let v = conv.as_str().map(|s| s.to_string());
        param_values.push(Box::new(v));
    }
    if let Some(blocks) = patch.get("blocks") {
        // Persisted chat blocks (text/tool_use/tool_result JSON array). Written
        // by finalize_session so history replays via BlocksView. null (Value::Null)
        // → SQL NULL so load returns None (raw agent / explicit clear), matching
        // the conversationId branch's null-handling — NOT the string "null".
        let v = if blocks.is_null() {
            None
        } else {
            Some(serde_json::to_string(blocks).unwrap_or_default())
        };
        set_clauses.push("blocks = ?".to_string());
        param_values.push(Box::new(v));
    }

    if set_clauses.is_empty() {
        return Ok(0);
    }

    // CAS guard: a terminal status may only transition out of 'running'. Without
    // this, stop_agent_session and finalize_session racing in the cancel window
    // would both write (non-deterministic final status) and both emit
    // agent:completed (duplicate notification). The guard makes the terminal
    // flip atomic — the loser updates 0 rows.
    let where_running = if terminal_status { " AND status = 'running'" } else { "" };
    let sql = format!("UPDATE sessions SET {} WHERE id = ?{}", set_clauses.join(", "), where_running);
    param_values.push(Box::new(id.to_string()));

    let params: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
    Ok(conn.execute(&sql, params.as_slice())?)
}

pub fn get_sessions_for_project_db(conn: &rusqlite::Connection, project_path: &str) -> Result<Vec<Session>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, project_path, agent_type, status, prompt, model,
                started_at, finished_at, exit_code, output_summary,
                context_snapshot, linked_requirement_id, parent_session_id,
                conversation_id, blocks, task_ref
         FROM sessions WHERE project_path = ?1 ORDER BY started_at DESC"
    )?;

    let sessions = stmt.query_map(params![project_path], |row| {
        let agent_type_str: String = row.get(2)?;
        let agent_type: AgentType = serde_json::from_value(serde_json::Value::String(agent_type_str))
            .unwrap_or(AgentType::ClaudeCode);

        let status_str: String = row.get(3)?;
        let status = match status_str.as_str() {
            "running" => SessionStatus::Running,
            "completed" => SessionStatus::Completed,
            "failed" => SessionStatus::Failed,
            "cancelled" => SessionStatus::Cancelled,
            _ => SessionStatus::Failed,
        };

        let snapshot_str: Option<String> = row.get(10)?;
        let context_snapshot: Option<ContextSnapshot> = snapshot_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        let blocks_str: Option<String> = row.get(14)?;
        let blocks: Option<serde_json::Value> = blocks_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
        let task_ref: Option<String> = row.get(15)?;

        Ok(Session {
            id: row.get(0)?,
            project_path: row.get(1)?,
            agent_type,
            status,
            prompt: row.get(4)?,
            model: row.get(5)?,
            started_at: row.get(6)?,
            finished_at: row.get(7)?,
            exit_code: row.get(8)?,
            output_summary: row.get(9)?,
            context_snapshot,
            linked_requirement_id: row.get(11)?,
            parent_session_id: row.get(12)?,
            conversation_id: row.get(13)?,
            blocks,
            task_ref,
        })
    })?;

    let mut result = Vec::new();
    for s in sessions {
        result.push(s?);
    }
    Ok(result)
}

// ---- Conversation CRUD ----
//
// A Conversation is the multi-turn container (= a Claude Code session). Turns
// (sessions) attach via conversation_id. These helpers cover insert / load /
// patch; the v9→v10 migration backfills conversations from existing
// parent_session_id chains.

pub fn insert_conversation_db(conn: &rusqlite::Connection, c: &Conversation) -> Result<(), AppError> {
    conn.execute(
        "INSERT OR REPLACE INTO conversations
            (id, project_path, title, last_agent, status, started_at, last_activity_at, pinned)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            c.id,
            c.project_path,
            c.title,
            c.last_agent.as_ref().map(|a| serde_json::to_string(a).unwrap_or_default().trim_matches('"').to_string()),
            c.status,
            c.started_at,
            c.last_activity_at,
            c.pinned as i32,
        ],
    )?;
    Ok(())
}

fn parse_agent_type(s: Option<String>) -> Option<AgentType> {
    s.and_then(|v| serde_json::from_value(serde_json::Value::String(v)).ok())
}

pub fn load_conversations_for_project_db(
    conn: &rusqlite::Connection,
    project_path: &str,
    include_archived: bool,
) -> Result<Vec<Conversation>, AppError> {
    // Soft-delete model: archived/deleted rows are hidden unless explicitly
    // requested, so the sidebar only shows active conversations by default.
    let sql = if include_archived {
        "SELECT id, project_path, title, last_agent, status, started_at, last_activity_at, pinned
         FROM conversations WHERE project_path = ?1
         ORDER BY pinned DESC, last_activity_at DESC"
    } else {
        "SELECT id, project_path, title, last_agent, status, started_at, last_activity_at, pinned
         FROM conversations WHERE project_path = ?1 AND status = 'active'
         ORDER BY pinned DESC, last_activity_at DESC"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![project_path], |row| {
        let last_agent_str: Option<String> = row.get(3)?;
        Ok(Conversation {
            id: row.get(0)?,
            project_path: row.get(1)?,
            title: row.get(2)?,
            last_agent: parse_agent_type(last_agent_str),
            status: row.get(4)?,
            started_at: row.get(5)?,
            last_activity_at: row.get(6)?,
            pinned: row.get::<_, i32>(7)? != 0,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Patch a conversation by id. Supports title / status / pinned / lastAgent /
/// lastActivityAt. Mirrors update_session_db's dynamic-SET style.
pub fn update_conversation_db(
    conn: &rusqlite::Connection,
    id: &str,
    patch: serde_json::Value,
) -> Result<(), AppError> {
    let mut set_clauses: Vec<String> = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(title) = patch.get("title").and_then(|v| v.as_str()) {
        set_clauses.push("title = ?".to_string());
        param_values.push(Box::new(title.to_string()));
    }
    if let Some(status) = patch.get("status").and_then(|v| v.as_str()) {
        set_clauses.push("status = ?".to_string());
        param_values.push(Box::new(status.to_string()));
    }
    if let Some(pinned) = patch.get("pinned").and_then(|v| v.as_bool()) {
        set_clauses.push("pinned = ?".to_string());
        param_values.push(Box::new(pinned as i32));
    }
    if let Some(last_agent) = patch.get("lastAgent").and_then(|v| v.as_str()) {
        set_clauses.push("last_agent = ?".to_string());
        param_values.push(Box::new(last_agent.to_string()));
    }
    if let Some(last_activity) = patch.get("lastActivityAt").or_else(|| patch.get("last_activity_at")).and_then(|v| v.as_str()) {
        set_clauses.push("last_activity_at = ?".to_string());
        param_values.push(Box::new(last_activity.to_string()));
    }

    if set_clauses.is_empty() {
        return Ok(());
    }
    let sql = format!("UPDATE conversations SET {} WHERE id = ?", set_clauses.join(", "));
    param_values.push(Box::new(id.to_string()));
    let params: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
    let rows = conn.execute(&sql, params.as_slice())?;
    if rows == 0 {
        return Err(AppError::NotFound(format!("Conversation {} 不存在", id)));
    }
    Ok(())
}

/// Set a conversation's lifecycle status (`active` | `archived` | `deleted`).
///
/// Soft-delete model: archived/deleted rows remain in the table so the action
/// is undoable (restore to `active`); `load_conversations_for_project_db` hides
/// them unless `include_archived` is set. Rejects any other status string so a
/// frontend typo can't smuggle an undocumented state into the column.
pub fn set_conversation_status_db(
    conn: &rusqlite::Connection,
    id: &str,
    status: &str,
) -> Result<(), AppError> {
    match status {
        "active" | "archived" | "deleted" => {}
        other => {
            return Err(AppError::Internal(format!(
                "非法对话状态 {other:?},允许 active|archived|deleted"
            )));
        }
    }
    let rows = conn.execute(
        "UPDATE conversations SET status = ?1 WHERE id = ?2",
        params![status, id],
    )?;
    if rows == 0 {
        return Err(AppError::NotFound(format!("Conversation {id} 不存在")));
    }
    Ok(())
}

/// Load the turns (sessions) of one conversation, oldest-first. Used by the
/// context bridge to inject prior-turn history into a follow-up turn of the
/// same conversation — especially when the follow-up switches agents, where the
/// new agent has no native way to inherit the prior agent's internal state.
pub fn load_turns_for_conversation_db(
    conn: &rusqlite::Connection,
    conversation_id: &str,
) -> Result<Vec<Session>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, project_path, agent_type, status, prompt, model,
                started_at, finished_at, exit_code, output_summary,
                context_snapshot, linked_requirement_id, parent_session_id,
                conversation_id, blocks, task_ref
         FROM sessions WHERE conversation_id = ?1 ORDER BY started_at ASC"
    )?;
    let sessions = stmt.query_map(params![conversation_id], |row| {
        let agent_type_str: String = row.get(2)?;
        let agent_type: AgentType = serde_json::from_value(serde_json::Value::String(agent_type_str))
            .unwrap_or(AgentType::ClaudeCode);
        let status_str: String = row.get(3)?;
        let status = match status_str.as_str() {
            "running" => SessionStatus::Running,
            "completed" => SessionStatus::Completed,
            "failed" => SessionStatus::Failed,
            "cancelled" => SessionStatus::Cancelled,
            _ => SessionStatus::Failed,
        };
        let snapshot_str: Option<String> = row.get(10)?;
        let context_snapshot: Option<ContextSnapshot> = snapshot_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
        let blocks_str: Option<String> = row.get(14)?;
        let blocks: Option<serde_json::Value> = blocks_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
        let task_ref: Option<String> = row.get(15)?;
        Ok(Session {
            id: row.get(0)?,
            project_path: row.get(1)?,
            agent_type,
            status,
            prompt: row.get(4)?,
            model: row.get(5)?,
            started_at: row.get(6)?,
            finished_at: row.get(7)?,
            exit_code: row.get(8)?,
            output_summary: row.get(9)?,
            context_snapshot,
            linked_requirement_id: row.get(11)?,
            parent_session_id: row.get(12)?,
            conversation_id: row.get(13)?,
            blocks,
            task_ref,
        })
    })?;
    let mut out = Vec::new();
    for s in sessions {
        out.push(s?);
    }

    // Defect ③: validate pairing integrity across all loaded sessions' blocks.
    // Sessions without blocks (raw agent, pre-G1) are skipped; only persisted
    // structured turns are checked. Orphan pairs indicate a crash/interrupt that
    // was not captured at stream end.
    for sess in &out {
        if let Some(raw_blocks) = &sess.blocks {
            if let Ok(blocks) = serde_json::from_value::<Vec<crate::agents::pty::ChatStreamEvent>>(raw_blocks.clone()) {
                let violations = validate_block_pairs(&blocks);
                for v in &violations {
                    log::warn!(
                        "[load_turns_for_conversation_db] pairing violation (session {}): {}",
                        sess.id,
                        v.detail()
                    );
                }
            }
        }
    }

    Ok(out)
}

/// Load a single session by id (None when absent). Used by edit_and_regenerate
/// to read the edited turn's lineage (parent_session_id + conversation_id)
/// before forking its regenerated sibling.
pub fn get_session_by_id_db(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<Option<Session>, AppError> {
    let row = conn
        .query_row(
            "SELECT id, project_path, agent_type, status, prompt, model,
                    started_at, finished_at, exit_code, output_summary,
                    context_snapshot, linked_requirement_id, parent_session_id,
                    conversation_id, blocks, task_ref
             FROM sessions WHERE id = ?1",
            params![id],
            |row| {
                let agent_type_str: String = row.get(2)?;
                let agent_type: AgentType =
                    serde_json::from_value(serde_json::Value::String(agent_type_str))
                        .unwrap_or(AgentType::ClaudeCode);
                let status_str: String = row.get(3)?;
                let status = match status_str.as_str() {
                    "running" => SessionStatus::Running,
                    "completed" => SessionStatus::Completed,
                    "failed" => SessionStatus::Failed,
                    _ => SessionStatus::Failed,
                };
                let snapshot_str: Option<String> = row.get(10)?;
                let context_snapshot: Option<ContextSnapshot> = snapshot_str
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok());
                let blocks_str: Option<String> = row.get(14)?;
                let blocks: Option<serde_json::Value> =
                    blocks_str.as_deref().and_then(|s| serde_json::from_str(s).ok());
                let task_ref: Option<String> = row.get(15)?;
                Ok(Session {
                    id: row.get(0)?,
                    project_path: row.get(1)?,
                    agent_type,
                    status,
                    prompt: row.get(4)?,
                    model: row.get(5)?,
                    started_at: row.get(6)?,
                    finished_at: row.get(7)?,
                    exit_code: row.get(8)?,
                    output_summary: row.get(9)?,
                    context_snapshot,
                    linked_requirement_id: row.get(11)?,
                    parent_session_id: row.get(12)?,
                    conversation_id: row.get(13)?,
                    blocks,
                    task_ref,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Walk the parent_session_id chain from `descendant_id` back to the root,
/// returning the ancestor turns oldest-first (NOT including the descendant
/// itself).
///
/// This is the branch-pure history injector for edit-and-regenerate: a fork
/// re-runs with `parent_session_id` = the edited turn's own parent, so its
/// prior context must be exactly that parent's ancestor chain — never the
/// edited turn's siblings (the other branches), which would otherwise leak
/// into the regenerated turn's history via the conversation-wide loader.
///
/// Linear conversations (no forks): equivalent to "all turns before this one".
/// Returns empty when `descendant_id` is the root (no parent) or doesn't exist.
pub fn load_turn_chain_db(
    conn: &rusqlite::Connection,
    descendant_id: &str,
) -> Result<Vec<Session>, AppError> {
    // Locate the descendant's conversation + its immediate parent. Missing row
    // or no parent ⇒ root ⇒ no ancestors to inject.
    let start: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT conversation_id, parent_session_id FROM sessions WHERE id = ?1",
            params![descendant_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()?;
    let Some((Some(conv), first_parent)) = start else {
        return Ok(Vec::new());
    };
    let Some(first_parent) = first_parent else {
        return Ok(Vec::new()); // root turn
    };

    // Load the whole conversation once, then walk the chain in memory. Cheaper
    // than N round-trips and reuses the validated row mapping of
    // load_turns_for_conversation_db (consistent Session decoding).
    let all = load_turns_for_conversation_db(conn, &conv)?;
    let by_id: HashMap<&str, &Session> = all.iter().map(|s| (s.id.as_str(), s)).collect();

    let mut chain = Vec::new();
    let mut cursor = Some(first_parent);
    let mut visited: HashSet<String> = HashSet::new();
    while let Some(pid) = cursor {
        if !visited.insert(pid.clone()) {
            break; // cycle guard (malformed chain)
        }
        match by_id.get(pid.as_str()) {
            Some(s) => {
                let session: &Session = s;
                chain.push(session.clone());
                cursor = session.parent_session_id.clone();
            }
            None => break, // dangling parent ref
        }
    }
    chain.reverse(); // collected child→parent; flip to oldest-first
    Ok(chain)
}

/// Return every turn of a conversation as flat [`BranchNode`]s (oldest-first),
/// each carrying its `parent_session_id`. The frontend groups by parent to
/// render the branch switcher. We deliberately do NOT assemble the tree server-
/// side — the parent pointers are enough for the client, and keeping it flat
/// avoids coupling branch UI shape to the backend.
pub fn load_conversation_branches_db(
    conn: &rusqlite::Connection,
    conversation_id: &str,
) -> Result<Vec<BranchNode>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, parent_session_id, prompt, status, started_at, agent_type
         FROM sessions WHERE conversation_id = ?1 ORDER BY started_at ASC",
    )?;
    let rows = stmt.query_map(params![conversation_id], |row| {
        let agent_type_str: String = row.get(5)?;
        Ok(BranchNode {
            id: row.get(0)?,
            parent_session_id: row.get(1)?,
            prompt: row.get(2)?,
            status: row.get(3)?,
            started_at: row.get(4)?,
            agent_type: agent_type_str,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

// ---- Legacy helpers (still used by pty output logging) ----

pub(crate) fn agents_dir() -> Result<PathBuf, String> {
    let home = crate::commands::projects::dirs_home();
    let dir = home.join(".dev-workbench").join("agents");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建 agents 目录失败: {}", e))?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::SessionStatus;

    /// RAII guard: creates a temp SQLite DB and sets thread-local override.
    struct TempDb {
        _tmp: tempfile::TempDir,
    }

    impl TempDb {
        fn new() -> Self {
            let tmp = tempfile::TempDir::new().unwrap();
            let db_path = tmp.path().join("test.db");
            let _conn = db::init_db(&db_path).expect("init_db failed");
            // Drop the connection so test_conn can open it
            drop(_conn);
            TEST_DB_PATH_OVERRIDE.with(|cell| *cell.borrow_mut() = Some(db_path));
            Self { _tmp: tmp }
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            TEST_DB_PATH_OVERRIDE.with(|cell| *cell.borrow_mut() = None);
        }
    }

    fn make_session(id: &str, project: &str, status: SessionStatus) -> Session {
        Session {
            id: id.to_string(),
            project_path: project.to_string(),
            agent_type: AgentType::ClaudeCode,
            status,
            prompt: "test prompt".to_string(),
            model: None,
            started_at: chrono::Local::now().to_rfc3339(),
            finished_at: None,
            exit_code: None,
            output_summary: None,
            context_snapshot: None,
            linked_requirement_id: None,
            parent_session_id: None,
            conversation_id: None,
            blocks: None,
            task_ref: None,
        }
    }

    #[test]
    fn test_add_and_load_sessions() {
        let _guard = TempDb::new();
        let conn = test_conn();

        insert_session_db(&conn, &make_session("s1", "/proj/a", SessionStatus::Running)).unwrap();
        insert_session_db(&conn, &make_session("s2", "/proj/b", SessionStatus::Completed)).unwrap();

        let loaded = load_sessions_from_db(&conn).unwrap();
        assert_eq!(loaded.len(), 2);
        let ids: Vec<&str> = loaded.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"s1"));
        assert!(ids.contains(&"s2"));
    }

    #[test]
    fn test_update_session_status() {
        let _guard = TempDb::new();
        let conn = test_conn();

        insert_session_db(&conn, &make_session("s1", "/proj/a", SessionStatus::Running)).unwrap();

        let patch = serde_json::json!({ "status": "completed", "exitCode": 0 });
        update_session_db(&conn, "s1", patch).unwrap();

        let loaded = load_sessions_from_db(&conn).unwrap();
        assert_eq!(loaded[0].status, SessionStatus::Completed);
        assert_eq!(loaded[0].exit_code, Some(0));
    }

    #[test]
    fn test_update_session_cancelled_status() {
        // Regression: stop_agent_session writes status="cancelled". Previously
        // update_session_db's validator rejected it (only running/completed/
        // failed allowed), so EVERY user stop returned Err via `?` — the
        // subprocess was killed but the DB row stayed Running (until the stale-
        // reconciler later flipped it to Failed) and agent:completed never
        // fired. Now "cancelled" is first-class: it round-trips through the DB
        // and loads back as SessionStatus::Cancelled (UI renders "已取消").
        let _guard = TempDb::new();
        let conn = test_conn();

        insert_session_db(&conn, &make_session("s1", "/proj/a", SessionStatus::Running)).unwrap();

        let patch = serde_json::json!({
            "status": "cancelled",
            "exitCode": 0,
            "outputSummary": "Session cancelled by user"
        });
        update_session_db(&conn, "s1", patch).unwrap();

        let loaded = load_sessions_from_db(&conn).unwrap();
        assert_eq!(loaded[0].status, SessionStatus::Cancelled);
        assert_eq!(loaded[0].exit_code, Some(0));
    }

    #[test]
    fn terminal_status_is_write_once_cas() {
        // Regression: stop_agent_session (status=cancelled) and finalize_session
        // (status=completed/failed) race in the cancel window. Before the CAS
        // guard both wrote unconditionally → non-deterministic final status AND a
        // double agent:completed. Now a terminal status is write-once: the second
        // writer flips 0 rows (Ok(0)) so the caller skips its duplicate emit.
        let _guard = TempDb::new();
        let conn = test_conn();
        insert_session_db(&conn, &make_session("s1", "/proj/a", SessionStatus::Running)).unwrap();

        // Natural completion wins the race first (running → completed): 1 row.
        let rows_complete = update_session_db(
            &conn,
            "s1",
            serde_json::json!({ "status": "completed", "exitCode": 0 }),
        )
        .unwrap();
        assert_eq!(rows_complete, 1, "first terminal transition from running wins");

        // The racing stop arrives second (already completed → cannot flip to
        // cancelled): 0 rows. Callers use Ok(0) to skip the duplicate emit.
        let rows_cancel = update_session_db(
            &conn,
            "s1",
            serde_json::json!({ "status": "cancelled", "exitCode": 0 }),
        )
        .unwrap();
        assert_eq!(rows_cancel, 0, "a terminal status cannot overwrite another");

        // Final status is the winner (completed), NOT cancelled.
        let loaded = load_sessions_from_db(&conn).unwrap();
        assert_eq!(loaded[0].status, SessionStatus::Completed);

        // Non-status column updates are NOT CAS-guarded: a blocks write on an
        // already-terminal session still applies (the guard only narrows terminal
        // status flips, not arbitrary column updates).
        let rows_blocks = update_session_db(
            &conn,
            "s1",
            serde_json::json!({ "blocks": [{ "kind": "text", "content": "late" }] }),
        )
        .unwrap();
        assert_eq!(rows_blocks, 1, "non-status updates are not CAS-guarded");
    }

    #[test]
    fn test_get_sessions_for_project() {
        let _guard = TempDb::new();
        let conn = test_conn();

        insert_session_db(&conn, &make_session("s1", "/proj/a", SessionStatus::Running)).unwrap();
        insert_session_db(&conn, &make_session("s2", "/proj/b", SessionStatus::Completed)).unwrap();
        insert_session_db(&conn, &make_session("s3", "/proj/a", SessionStatus::Completed)).unwrap();

        let proj_a = get_sessions_for_project_db(&conn, "/proj/a").unwrap();
        assert_eq!(proj_a.len(), 2);
        assert!(proj_a.iter().all(|s| s.project_path == "/proj/a"));
    }

    #[test]
    fn test_update_session_invalid_status() {
        let _guard = TempDb::new();
        let conn = test_conn();

        insert_session_db(&conn, &make_session("s1", "/proj/a", SessionStatus::Running)).unwrap();

        let patch = serde_json::json!({ "status": "invalid_status" });
        let result = update_session_db(&conn, "s1", patch);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("无效 status"));
    }

    // --- A3 conversation archive/delete (soft-delete model) ---

    fn make_conv(id: &str, project: &str, status: &str) -> Conversation {
        Conversation {
            id: id.to_string(),
            project_path: project.to_string(),
            title: format!("conv-{id}"),
            last_agent: None,
            status: status.to_string(),
            started_at: chrono::Local::now().to_rfc3339(),
            last_activity_at: chrono::Local::now().to_rfc3339(),
            pinned: false,
        }
    }

    #[test]
    fn archived_and_deleted_are_hidden_unless_requested() {
        let _guard = TempDb::new();
        let conn = test_conn();
        insert_conversation_db(&conn, &make_conv("c1", "/p", "active")).unwrap();
        insert_conversation_db(&conn, &make_conv("c2", "/p", "active")).unwrap();

        set_conversation_status_db(&conn, "c1", "archived").unwrap();
        set_conversation_status_db(&conn, "c2", "deleted").unwrap();

        let active = load_conversations_for_project_db(&conn, "/p", false).unwrap();
        assert_eq!(
            active.len(),
            0,
            "archived + deleted hidden from the default sidebar view"
        );

        let all = load_conversations_for_project_db(&conn, "/p", true).unwrap();
        assert_eq!(all.len(), 2, "include_archived surfaces both rows");
    }

    #[test]
    fn set_conversation_status_restore_round_trip() {
        let _guard = TempDb::new();
        let conn = test_conn();
        insert_conversation_db(&conn, &make_conv("c1", "/p", "active")).unwrap();

        set_conversation_status_db(&conn, "c1", "deleted").unwrap();
        assert_eq!(
            load_conversations_for_project_db(&conn, "/p", false)
                .unwrap()
                .len(),
            0,
            "soft-deleted hides from sidebar"
        );

        // Undo path: the frontend toast restores to 'active'.
        set_conversation_status_db(&conn, "c1", "active").unwrap();
        assert_eq!(
            load_conversations_for_project_db(&conn, "/p", false)
                .unwrap()
                .len(),
            1,
            "restore brings the conversation back to the sidebar"
        );
    }

    #[test]
    fn set_conversation_status_rejects_invalid_status() {
        let _guard = TempDb::new();
        let conn = test_conn();
        insert_conversation_db(&conn, &make_conv("c1", "/p", "active")).unwrap();

        let err = set_conversation_status_db(&conn, "c1", "frozen").unwrap_err();
        assert!(
            matches!(err, crate::error::AppError::Internal(_)),
            "undocumented status rejected: {err:?}"
        );
        // The rejected write must not have mutated the row.
        assert_eq!(
            load_conversations_for_project_db(&conn, "/p", true).unwrap()[0].status,
            "active"
        );
    }

    #[test]
    fn set_conversation_status_missing_id_is_not_found() {
        let _guard = TempDb::new();
        let conn = test_conn();

        let err = set_conversation_status_db(&conn, "ghost", "archived").unwrap_err();
        assert!(
            matches!(err, crate::error::AppError::NotFound(_)),
            "missing conversation → NotFound: {err:?}"
        );
    }

    #[test]
    fn load_turns_returns_conversation_turns_oldest_first() {
        let _guard = TempDb::new();
        let conn = test_conn();

        // Three turns of one conversation, inserted out of time-order to prove
        // the ORDER BY started_at ASC sort (not insert order) drives the result.
        let mut mid = make_session("mid", "/p", SessionStatus::Completed);
        mid.conversation_id = Some("c1".to_string());
        mid.started_at = "2026-01-02T00:00:00Z".to_string();
        let mut last = make_session("last", "/p", SessionStatus::Completed);
        last.conversation_id = Some("c1".to_string());
        last.started_at = "2026-01-03T00:00:00Z".to_string();
        let mut first = make_session("first", "/p", SessionStatus::Completed);
        first.conversation_id = Some("c1".to_string());
        first.started_at = "2026-01-01T00:00:00Z".to_string();
        // A turn of a DIFFERENT conversation must not leak in.
        let mut other = make_session("other", "/p", SessionStatus::Completed);
        other.conversation_id = Some("c2".to_string());

        for s in [&mid, &last, &first, &other] {
            insert_session_db(&conn, s).unwrap();
        }

        let turns = load_turns_for_conversation_db(&conn, "c1").unwrap();
        let ids: Vec<&str> = turns.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["first", "mid", "last"], "oldest-first within c1 only");
    }

    // --- A4 edit-and-regenerate: branch-pure turn chain ---

    /// Helper: a turn with explicit conversation + parent + ordered start time,
    /// so branch trees can be built deterministically in tests.
    fn make_turn(id: &str, conv: &str, parent: Option<&str>, started: &str) -> Session {
        let mut s = make_session(id, "/p", SessionStatus::Completed);
        s.conversation_id = Some(conv.to_string());
        s.parent_session_id = parent.map(|p| p.to_string());
        s.started_at = started.to_string();
        s
    }

    /// The core A4 guarantee: a forked/regenerated turn's history chain is its
    /// parent's ancestors ONLY — never the sibling branch being replaced. Without
    /// this, the conversation-wide loader would leak the edited-out branch into
    /// the new turn's context.
    #[test]
    fn turn_chain_walks_ancestors_excluding_sibling_branches() {
        let _guard = TempDb::new();
        let conn = test_conn();

        // root → b1 → c1   (original branch)
        //  └──→ b2         (fork: b1 edited → regenerated as sibling under root)
        let nodes = [
            make_turn("root", "c1", None, "2026-01-01T00:00:00Z"),
            make_turn("b1", "c1", Some("root"), "2026-01-02T00:00:00Z"),
            make_turn("c1", "c1", Some("b1"), "2026-01-03T00:00:00Z"),
            make_turn("b2", "c1", Some("root"), "2026-01-04T00:00:00Z"),
        ];
        for s in &nodes {
            insert_session_db(&conn, s).unwrap();
        }

        // b2 (the fork) sees ONLY [root] — not b1/c1.
        let chain = load_turn_chain_db(&conn, "b2").unwrap();
        let ids: Vec<&str> = chain.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["root"], "forked turn chain = parent's ancestors only");

        // c1 (deep in the original branch) sees its full ancestor chain.
        let chain_c1 = load_turn_chain_db(&conn, "c1").unwrap();
        let ids_c1: Vec<&str> = chain_c1.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids_c1, vec!["root", "b1"]);

        // Root (no parent) and a missing id both yield empty.
        assert!(load_turn_chain_db(&conn, "root").unwrap().is_empty());
        assert!(load_turn_chain_db(&conn, "missing").unwrap().is_empty());
    }

    #[test]
    fn conversation_branches_returns_flat_turns_with_parent_pointers() {
        let _guard = TempDb::new();
        let conn = test_conn();

        for s in [
            make_turn("root", "c1", None, "2026-01-01T00:00:00Z"),
            make_turn("b1", "c1", Some("root"), "2026-01-02T00:00:00Z"),
            make_turn("b2", "c1", Some("root"), "2026-01-04T00:00:00Z"),
            // A different conversation's turn must not leak in.
            make_turn("other", "c2", None, "2026-01-05T00:00:00Z"),
        ] {
            insert_session_db(&conn, &s).unwrap();
        }

        let branches = load_conversation_branches_db(&conn, "c1").unwrap();
        let ids: Vec<&str> = branches.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, vec!["root", "b1", "b2"], "all c1 turns oldest-first; c2 excluded");

        // b1 + b2 share parent=root → siblings → the branch switcher's group.
        let b1 = branches.iter().find(|b| b.id == "b1").unwrap();
        let b2 = branches.iter().find(|b| b.id == "b2").unwrap();
        assert_eq!(b1.parent_session_id.as_deref(), Some("root"));
        assert_eq!(b2.parent_session_id.as_deref(), Some("root"));
        assert_eq!(branches[0].parent_session_id, None, "root has no parent");
    }

    /// blocks round-trip: update_session_db writes the persisted blocks JSON,
    /// load_sessions_from_db reads it back as the same Value. This is the DB
    /// half of the G1 persistence path (the merge+cap transformation is unit-
    /// tested in pty::tests; finalize_session applies it before this write).
    #[test]
    fn blocks_round_trip_through_update_and_load() {
        let _guard = TempDb::new();
        let conn = test_conn();
        insert_session_db(&conn, &make_session("s1", "/p", SessionStatus::Running)).unwrap();

        let blocks = serde_json::json!([
            { "kind": "text", "content": "hello" },
            { "kind": "tool_use", "name": "Read", "input": { "file_path": "/x" } },
            { "kind": "tool_result", "content": "file body", "is_error": false },
        ]);
        let mut patch = serde_json::json!({});
        patch["blocks"] = blocks.clone();
        update_session_db(&conn, "s1", patch).unwrap();

        let loaded = load_sessions_from_db(&conn).unwrap();
        let s = loaded.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(
            s.blocks.as_ref().unwrap(),
            &blocks,
            "blocks must round-trip unchanged through the DB layer"
        );
    }

    /// A raw agent (no agent:event stream) writes no blocks — load must return
    /// None without error, and an explicit null patch must clear them. Guards
    /// the fallback path: AgentMessage falls through to the terminal when blocks
    /// is null/None.
    #[test]
    fn blocks_absent_is_none_and_null_clears() {
        let _guard = TempDb::new();
        let conn = test_conn();
        // No blocks written at all (raw agent / pre-G1 session).
        insert_session_db(&conn, &make_session("raw", "/p", SessionStatus::Completed)).unwrap();
        let loaded = load_sessions_from_db(&conn).unwrap();
        let s = loaded.iter().find(|s| s.id == "raw").unwrap();
        assert!(s.blocks.is_none(), "no blocks written → None on load");

        // Explicit null patch clears the column.
        let mut patch = serde_json::json!({});
        patch["blocks"] = serde_json::Value::Null;
        update_session_db(&conn, "raw", patch).unwrap();
        let loaded2 = load_sessions_from_db(&conn).unwrap();
        let s2 = loaded2.iter().find(|s| s.id == "raw").unwrap();
        assert!(s2.blocks.is_none(), "null blocks patch → None on load");
    }
}
