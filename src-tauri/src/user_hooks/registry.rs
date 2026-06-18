//! User-defined hook registry — SQLite CRUD + load-enabled-by-event for the
//! D2 lifecycle-hook config layer. Mirrors the [`crate::slash_commands`]
//! registry shape (list/find/create/update/delete) so the frontend authoring
//! flow and the agent-build-time loader share one data path.

use rusqlite::Connection;

use crate::error::AppError;
use crate::models::{UserHook, UserHookEvent};

/// List every user hook (enabled + disabled), ordered by name. The settings UI
/// reads this to render the management list.
pub fn list_user_hooks(conn: &Connection) -> Result<Vec<UserHook>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, event, command, shell, timeout_secs, enabled, created_at \
         FROM user_hooks ORDER BY name",
    )?;
    let rows = stmt.query_map([], row_to_hook)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Look up a single hook by primary key (id).
pub fn find_by_id(conn: &Connection, id: &str) -> Result<Option<UserHook>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, event, command, shell, timeout_secs, enabled, created_at \
         FROM user_hooks WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map([id], row_to_hook)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// Load the enabled hooks bound to a given event, ordered by name. This is the
/// shape `build_react_agent` wants: it registers one [`UserCommandHook`] per row
/// and each hook no-ops for events it isn't bound to, but pre-filtering avoids
/// registering dead hooks and makes the set testable in isolation.
pub fn load_enabled_by_event(
    conn: &Connection,
    event: UserHookEvent,
) -> Result<Vec<UserHook>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, event, command, shell, timeout_secs, enabled, created_at \
         FROM user_hooks WHERE enabled = 1 AND event = ?1 ORDER BY name",
    )?;
    let rows = stmt.query_map([event.as_db()], row_to_hook)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Create a user hook. `name` must be unique. Returns the created row (re-read so
/// the caller gets the generated id + created_at).
pub fn create_hook(
    conn: &Connection,
    name: &str,
    event: UserHookEvent,
    command: &str,
    shell: bool,
    timeout_secs: u64,
    enabled: bool,
) -> Result<UserHook, AppError> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "INSERT INTO user_hooks (id, name, event, command, shell, timeout_secs, enabled, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            id,
            name,
            event.as_db(),
            command,
            shell as i64,
            timeout_secs as i64,
            enabled as i64,
            now,
        ],
    )
    .map_err(|e| {
        // UNIQUE(name) clash → friendly message.
        if matches!(
            e,
            rusqlite::Error::SqliteFailure(ref f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation
        ) {
            AppError::Config(format!("hook {name} 已存在"))
        } else {
            AppError::from(e)
        }
    })?;
    find_by_id(conn, &id)?
        .ok_or_else(|| AppError::Config("insert succeeded but row not found".into()))
}

/// Update a hook's editable fields by id.
pub fn update_hook(
    conn: &Connection,
    id: &str,
    name: &str,
    event: UserHookEvent,
    command: &str,
    shell: bool,
    timeout_secs: u64,
    enabled: bool,
) -> Result<(), AppError> {
    // Existence check first so an absent id errors with a clear message rather
    // than silently updating zero rows.
    find_by_id(conn, id)?
        .ok_or_else(|| AppError::Config(format!("hook {id} 不存在")))?;
    conn.execute(
        "UPDATE user_hooks SET name=?1, event=?2, command=?3, shell=?4, timeout_secs=?5, enabled=?6 \
         WHERE id=?7",
        rusqlite::params![
            name,
            event.as_db(),
            command,
            shell as i64,
            timeout_secs as i64,
            enabled as i64,
            id,
        ],
    )?;
    Ok(())
}

/// Flip just the `enabled` flag (the list-card toggle calls this without
/// re-POSTing the whole row).
pub fn set_enabled(conn: &Connection, id: &str, enabled: bool) -> Result<(), AppError> {
    find_by_id(conn, id)?
        .ok_or_else(|| AppError::Config(format!("hook {id} 不存在")))?;
    conn.execute(
        "UPDATE user_hooks SET enabled=?1 WHERE id=?2",
        rusqlite::params![enabled as i64, id],
    )?;
    Ok(())
}

/// Delete a hook by id.
pub fn delete_hook(conn: &Connection, id: &str) -> Result<(), AppError> {
    find_by_id(conn, id)?
        .ok_or_else(|| AppError::Config(format!("hook {id} 不存在")))?;
    conn.execute("DELETE FROM user_hooks WHERE id=?1", [id])?;
    Ok(())
}

fn row_to_hook(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserHook> {
    let event_str: String = row.get(2)?;
    let event = UserHookEvent::from_db(&event_str).map_err(|e| {
        // rusqlite::Error has no generic From<String>; wrap as ToSqlConversion so
        // the corrupt-row message bubbles up rather than panicking.
        rusqlite::Error::ToSqlConversionFailure(e.into())
    })?;
    Ok(UserHook {
        id: row.get(0)?,
        name: row.get(1)?,
        event,
        command: row.get(3)?,
        shell: row.get::<_, i64>(4)? != 0,
        timeout_secs: row.get::<_, i64>(5)? as u64,
        enabled: row.get::<_, i64>(6)? != 0,
        created_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Connection {
        let tmp = tempfile::TempDir::new().unwrap();
        crate::db::init_db(&tmp.path().join("h.db")).unwrap()
    }

    #[test]
    fn list_empty_init_returns_none() {
        let conn = fresh();
        assert!(list_user_hooks(&conn).unwrap().is_empty(), "no hooks seeded");
    }

    #[test]
    fn create_then_find_and_list() {
        let conn = fresh();
        let h = create_hook(
            &conn,
            "load-conventions",
            UserHookEvent::UserPromptSubmit,
            "cat .cursorrules",
            true,
            30,
            true,
        )
        .unwrap();
        assert_eq!(h.name, "load-conventions");
        assert_eq!(h.event, UserHookEvent::UserPromptSubmit);
        assert!(h.shell);
        assert_eq!(h.timeout_secs, 30);
        assert!(h.enabled);

        let by_id = find_by_id(&conn, &h.id).unwrap().expect("findable by id");
        assert_eq!(by_id.name, "load-conventions");
        let all = list_user_hooks(&conn).unwrap();
        assert!(all.iter().any(|x| x.name == "load-conventions"));
    }

    #[test]
    fn create_duplicate_name_errors() {
        let conn = fresh();
        create_hook(
            &conn,
            "dup",
            UserHookEvent::Stop,
            "echo done",
            true,
            10,
            true,
        )
        .unwrap();
        let err = create_hook(&conn, "dup", UserHookEvent::Stop, "x", true, 10, true);
        assert!(err.is_err(), "duplicate name must error");
    }

    #[test]
    fn load_enabled_by_event_filters_event_and_disabled() {
        let conn = fresh();
        let a = create_hook(
            &conn,
            "a",
            UserHookEvent::UserPromptSubmit,
            "echo a",
            true,
            10,
            true,
        )
        .unwrap();
        create_hook(
            &conn,
            "b",
            UserHookEvent::Stop,
            "echo b",
            true,
            10,
            true,
        )
        .unwrap();
        // Disabled submit hook must NOT appear in the enabled load.
        create_hook(
            &conn,
            "c",
            UserHookEvent::UserPromptSubmit,
            "echo c",
            true,
            10,
            false,
        )
        .unwrap();

        let submits = load_enabled_by_event(&conn, UserHookEvent::UserPromptSubmit).unwrap();
        assert_eq!(submits.len(), 1, "only enabled submit hooks: {submits:?}");
        assert_eq!(submits[0].id, a.id);

        let stops = load_enabled_by_event(&conn, UserHookEvent::Stop).unwrap();
        assert_eq!(stops.len(), 1);
        assert_eq!(stops[0].name, "b");
    }

    #[test]
    fn update_changes_fields() {
        let conn = fresh();
        let h = create_hook(
            &conn,
            "h",
            UserHookEvent::Stop,
            "old",
            true,
            10,
            true,
        )
        .unwrap();
        update_hook(
            &conn,
            &h.id,
            "h2",
            UserHookEvent::UserPromptSubmit,
            "new",
            false,
            60,
            false,
        )
        .unwrap();
        let after = find_by_id(&conn, &h.id).unwrap().unwrap();
        assert_eq!(after.name, "h2");
        assert_eq!(after.event, UserHookEvent::UserPromptSubmit);
        assert_eq!(after.command, "new");
        assert!(!after.shell);
        assert_eq!(after.timeout_secs, 60);
        assert!(!after.enabled);
    }

    #[test]
    fn update_absent_id_errors() {
        let conn = fresh();
        assert!(update_hook(
            &conn,
            "nope",
            "x",
            UserHookEvent::Stop,
            "c",
            true,
            10,
            true
        )
        .is_err());
    }

    #[test]
    fn set_enabled_toggles_flag() {
        let conn = fresh();
        let h = create_hook(
            &conn,
            "h",
            UserHookEvent::Stop,
            "c",
            true,
            10,
            true,
        )
        .unwrap();
        set_enabled(&conn, &h.id, false).unwrap();
        assert!(!find_by_id(&conn, &h.id).unwrap().unwrap().enabled);
        set_enabled(&conn, &h.id, true).unwrap();
        assert!(find_by_id(&conn, &h.id).unwrap().unwrap().enabled);
    }

    #[test]
    fn delete_removes_hook() {
        let conn = fresh();
        let h = create_hook(
            &conn,
            "h",
            UserHookEvent::Stop,
            "c",
            true,
            10,
            true,
        )
        .unwrap();
        delete_hook(&conn, &h.id).unwrap();
        assert!(find_by_id(&conn, &h.id).unwrap().is_none());
        // Deleting again (absent id) errors.
        assert!(delete_hook(&conn, &h.id).is_err());
    }

    #[test]
    fn event_db_round_trip() {
        for ev in [UserHookEvent::UserPromptSubmit, UserHookEvent::Stop] {
            assert_eq!(UserHookEvent::from_db(ev.as_db()).unwrap(), ev);
        }
        assert!(UserHookEvent::from_db("bogus").is_err());
    }
}
