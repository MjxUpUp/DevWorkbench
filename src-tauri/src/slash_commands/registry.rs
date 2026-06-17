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
}
