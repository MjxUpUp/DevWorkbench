//! Slash command registry — SQLite list/lookup + template rendering.

use rusqlite::Connection;

use crate::error::AppError;
use crate::models::SlashCommand;

/// List every slash command (built-in + user-defined), ordered by name.
pub fn list_slash_commands(conn: &Connection) -> Result<Vec<SlashCommand>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, template, category, created_at \
         FROM slash_commands ORDER BY name",
    )?;
    let rows = stmt.query_map([], row_to_command)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Look up a single command by name (WITHOUT the leading slash).
pub fn find_by_name(conn: &Connection, name: &str) -> Result<Option<SlashCommand>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, template, category, created_at \
         FROM slash_commands WHERE name = ?1",
    )?;
    let mut rows = stmt.query_map([name], row_to_command)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// Look up by primary key (id). Mirrors [`find_by_name`]'s shape.
pub fn find_by_id(conn: &Connection, id: &str) -> Result<Option<SlashCommand>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, template, category, created_at \
         FROM slash_commands WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map([id], row_to_command)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// Whether a command is a seeded builtin (protected from edit/delete). init_db
/// re-seeds builtins every launch via `INSERT OR IGNORE`, so user edits to them
/// would be silently reverted anyway — better to refuse up front.
fn is_builtin(cmd: &SlashCommand) -> bool {
    cmd.category.as_deref() == Some("builtin")
}

/// Create a user-defined slash command. `name` must be unique and carry no
/// leading slash. Closes the dive_02 gap: previously you could only CONSUME
/// builtins, never author one. Returns the created row (re-read so the caller
/// gets the generated id + created_at).
pub fn create_command(
    conn: &Connection,
    name: &str,
    description: Option<&str>,
    template: &str,
    category: Option<&str>,
) -> Result<SlashCommand, AppError> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "INSERT INTO slash_commands (id, name, description, template, category, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![id, name, description, template, category, now],
    )
    .map_err(|e| {
        // UNIQUE(name) clash (incl. shadowing a builtin) → friendly message.
        if matches!(
            e,
            rusqlite::Error::SqliteFailure(ref f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation
        ) {
            AppError::Config(format!("slash command /{name} 已存在"))
        } else {
            AppError::from(e)
        }
    })?;
    find_by_name(conn, name)?
        .ok_or_else(|| AppError::Config("insert succeeded but row not found".into()))
}

/// Update a command's editable fields by id. Built-ins are protected. Errors if
/// the id is absent or names a builtin.
pub fn update_command(
    conn: &Connection,
    id: &str,
    name: &str,
    description: Option<&str>,
    template: &str,
    category: Option<&str>,
) -> Result<(), AppError> {
    let existing = find_by_id(conn, id)?
        .ok_or_else(|| AppError::Config(format!("slash command {id} 不存在")))?;
    if is_builtin(&existing) {
        return Err(AppError::Config("内置命令不可编辑（category=builtin）".into()));
    }
    conn.execute(
        "UPDATE slash_commands SET name=?1, description=?2, template=?3, category=?4 WHERE id=?5",
        rusqlite::params![name, description, template, category, id],
    )?;
    Ok(())
}

/// Delete a command by id. Built-ins are protected (same reason as update).
/// Errors if absent or builtin.
pub fn delete_command(conn: &Connection, id: &str) -> Result<(), AppError> {
    let existing = find_by_id(conn, id)?
        .ok_or_else(|| AppError::Config(format!("slash command {id} 不存在")))?;
    if is_builtin(&existing) {
        return Err(AppError::Config("内置命令不可删除（category=builtin）".into()));
    }
    conn.execute("DELETE FROM slash_commands WHERE id=?1", [id])?;
    Ok(())
}

fn row_to_command(row: &rusqlite::Row<'_>) -> rusqlite::Result<SlashCommand> {
    Ok(SlashCommand {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        template: row.get(3)?,
        category: row.get(4)?,
        created_at: row.get(5)?,
    })
}

/// Render a template, substituting `$ARGUMENTS`/`$0` with the full argument
/// string and `$1`..`$n` with whitespace-split tokens (claude-code
/// argumentSubstitution). Unknown `$x` tokens are left intact. Replace
/// `$ARGUMENTS`/`$0` BEFORE positional tokens so a template like `$ARGUMENTS
/// $1` isn't double-substituted.
pub fn render_template(template: &str, args: &str) -> String {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    let mut out = template.replace("$ARGUMENTS", args);
    out = out.replace("$0", args);
    for (i, tok) in tokens.iter().enumerate() {
        out = out.replace(&format!("${}", i + 1), tok);
    }
    out
}

/// Parse `/name rest of args` from the start of a prompt into `(name, args)`.
/// Returns `None` if the prompt doesn't start with `/` or has no name. Trims
/// only leading whitespace so `/cmd` mid-prompt (after other text) is NOT
/// treated as a command.
pub fn parse_command(prompt: &str) -> Option<(String, String)> {
    let rest = prompt.trim_start().strip_prefix('/')?;
    let (name, args) = match rest.find(char::is_whitespace) {
        Some(i) => (rest[..i].to_string(), rest[i..].trim().to_string()),
        None => (rest.to_string(), String::new()),
    };
    if name.is_empty() {
        return None;
    }
    Some((name, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_substitutes_arguments_and_zero() {
        assert_eq!(
            render_template("需求：$ARGUMENTS (= $0)", "fix bug"),
            "需求：fix bug (= fix bug)"
        );
    }

    #[test]
    fn render_substitutes_positional_tokens() {
        assert_eq!(
            render_template("first=$1 second=$2 all=$ARGUMENTS", "a b c"),
            "first=a second=b all=a b c"
        );
    }

    #[test]
    fn render_leaves_unknown_placeholders_intact() {
        // $X is not a known placeholder; $9 has no 9th token → both intact.
        assert_eq!(render_template("$X and $9", "a"), "$X and $9");
    }

    #[test]
    fn render_empty_args_blanks_placeholders() {
        assert_eq!(render_template("do: $ARGUMENTS", ""), "do: ");
    }

    #[test]
    fn parse_splits_name_and_args() {
        assert_eq!(
            parse_command("/plan fix the bug"),
            Some(("plan".into(), "fix the bug".into()))
        );
    }

    #[test]
    fn parse_command_with_no_args() {
        assert_eq!(parse_command("/review"), Some(("review".into(), "".into())));
    }

    #[test]
    fn parse_non_command_returns_none() {
        assert_eq!(parse_command("hello world"), None);
        assert_eq!(parse_command(""), None);
        assert_eq!(parse_command("  /not-at-start"), Some(("not-at-start".into(), "".into())));
    }

    /// End-to-end through the DB: init_db runs SCHEMA which seeds the four
    /// builtins; find_by_name + render_template mirror what spawn_agent_session
    /// does at submit time.
    #[test]
    fn builtin_commands_seeded_and_findable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::init_db(&tmp.path().join("seed.db")).unwrap();
        let all = list_slash_commands(&conn).unwrap();
        let names: Vec<&str> = all.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(all.len(), 4, "exactly the 4 builtins seeded, got {names:?}");
        for expected in ["plan", "review", "test", "fix"] {
            assert!(names.contains(&expected), "{expected} must be seeded, got {names:?}");
        }
        let plan = find_by_name(&conn, "plan").unwrap().expect("plan findable");
        assert!(plan.template.contains("$ARGUMENTS"), "template keeps the placeholder");
        let rendered = render_template(&plan.template, "do X");
        assert!(rendered.contains("do X"), "args substituted in");
        assert!(!rendered.contains("$ARGUMENTS"), "placeholder must be substituted away");
        assert!(find_by_name(&conn, "nope").unwrap().is_none(), "unknown name → None");
    }

    // ---- D2 CRUD: user can author/edit/delete commands; builtins protected ----

    #[test]
    fn create_then_find_and_list() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::init_db(&tmp.path().join("c.db")).unwrap();
        let cmd =
            create_command(&conn, "myreview", Some("my review"), "Review: $ARGUMENTS", Some("user"))
                .unwrap();
        assert_eq!(cmd.name, "myreview");
        assert_eq!(cmd.category.as_deref(), Some("user"));
        // Read back by name AND by id — both must resolve to the same row.
        let by_name = find_by_name(&conn, "myreview").unwrap().expect("findable by name");
        assert_eq!(by_name.id, cmd.id);
        assert_eq!(by_name.template, "Review: $ARGUMENTS");
        let by_id = find_by_id(&conn, &cmd.id).unwrap().expect("findable by id");
        assert_eq!(by_id.name, "myreview");
        let all = list_slash_commands(&conn).unwrap();
        assert!(
            all.iter().any(|c| c.name == "myreview"),
            "user command appears in list alongside builtins"
        );
    }

    #[test]
    fn create_duplicate_name_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::init_db(&tmp.path().join("c.db")).unwrap();
        create_command(&conn, "dup", None, "t", None).unwrap();
        let err = create_command(&conn, "dup", None, "t", None);
        assert!(err.is_err(), "duplicate user name must error");
    }

    #[test]
    fn create_shadowing_builtin_name_errors() {
        // 'plan' is a seeded builtin → the UNIQUE(name) constraint must refuse
        // a user command trying to shadow it (no silent override of /plan).
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::init_db(&tmp.path().join("c.db")).unwrap();
        let err = create_command(&conn, "plan", None, "evil", None);
        assert!(err.is_err(), "must not shadow builtin /plan");
    }

    #[test]
    fn update_changes_fields_and_blocks_builtin() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::init_db(&tmp.path().join("c.db")).unwrap();
        let cmd = create_command(&conn, "u", Some("d"), "old", Some("user")).unwrap();
        update_command(&conn, &cmd.id, "u2", Some("d2"), "new", Some("user")).unwrap();
        let after = find_by_id(&conn, &cmd.id).unwrap().unwrap();
        assert_eq!(after.name, "u2");
        assert_eq!(after.template, "new");
        // Built-in is protected from edit.
        let builtin = find_by_name(&conn, "plan").unwrap().unwrap();
        assert!(
            update_command(&conn, &builtin.id, "plan", None, "x", Some("builtin")).is_err(),
            "builtin must not be editable"
        );
    }

    #[test]
    fn delete_removes_user_and_blocks_builtin() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = crate::db::init_db(&tmp.path().join("c.db")).unwrap();
        let cmd = create_command(&conn, "del", None, "t", Some("user")).unwrap();
        delete_command(&conn, &cmd.id).unwrap();
        assert!(
            find_by_id(&conn, &cmd.id).unwrap().is_none(),
            "user command gone after delete"
        );
        let builtin = find_by_name(&conn, "plan").unwrap().unwrap();
        assert!(
            delete_command(&conn, &builtin.id).is_err(),
            "builtin must not be deletable"
        );
        // plan still present after the refused delete.
        assert!(find_by_name(&conn, "plan").unwrap().is_some());
    }
}
