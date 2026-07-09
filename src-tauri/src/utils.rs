//! Shared utilities — single home for helpers used across modules to avoid
//! the duplicated-strip-ansi drift that previously lived in agents/pty.rs.

/// Strip ANSI CSI escape sequences from a string.
///
/// Removes `ESC [ ... <letter>` sequences (the common SGR/color/cursor set).
/// Previously duplicated as `strip_ansi` (pty.rs), `strip_ansi_basic`
/// (pty.rs), and `strip_ansi_escapes` (collector.rs) — now unified here.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            // Consume the CSI sequence up to and including the final letter.
            chars.next(); // consume '['
            for nc in chars.by_ref() {
                if nc.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Extract `filesChanged` from a ContextSnapshot JSON string.
/// Shared by executor.rs (poll) and opaque_agent.rs (read_session_files) to
/// avoid the duplicated parse logic (shotgun-surgery risk).
pub fn files_changed_from_snapshot(snap: Option<&str>) -> Vec<String> {
    snap.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| v.get("filesChanged").cloned())
        .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_sgr_color_codes() {
        let s = "\u{1b}[31mred\u{1b}[0m text";
        assert_eq!(strip_ansi(s), "red text");
    }

    #[test]
    fn leaves_plain_text_intact() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn strips_cursor_movement() {
        assert_eq!(strip_ansi("a\u{1b}[2Ab"), "ab");
    }
}
