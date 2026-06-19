//! B6 tool-call-repair — promote plain-text tool calls back into structured
//! `ToolCall`s.
//!
//! Weak models (GLM / DeepSeek families) sometimes leak a tool call as plain
//! text instead of a structured `tool_use` block, e.g.:
//!
//! ```text
//! [grep]\n{"pattern": "foo"}\n[END_TOOL_REQUEST]
//! [tool:write_file] {"path": "a.txt", "content": "x"}
//! <|channel|>commentary to=run code {...}<|call|>
//! <function=read_file><parameter=path>src/lib.rs</parameter></function>
//! ```
//!
//! The agent loop only acts on structured `tool_calls`, so a leaked plain-text
//! call silently terminates the turn. This module detects such leaks in the
//! assembled assistant text and promotes them to real `ToolCall`s.
//!
//! Pure-Rust port of openclaw `packages/tool-call-repair` (payload.ts +
//! grammar.ts), non-streaming `parseStandalone` path only. The streaming
//! normalizer (1371 lines) is deferred — repairing the fully-assembled
//! assistant text already covers the core robustness gap for non-structured
//! models.

use kernel_core::{FunctionCall, ToolCall};

/// Legacy marker some models emit after a serialized JSON tool request.
const END_TOOL_REQUEST: &str = "[END_TOOL_REQUEST]";
/// Harmony stream marker introducing the target channel before a tool call.
const HARMONY_CHANNEL_MARKER: &str = "<|channel|>";
/// Harmony stream marker that may separate the header from the JSON payload.
const HARMONY_MESSAGE_MARKER: &str = "<|message|>";
/// Harmony stream marker that may close a serialized tool-call payload.
const HARMONY_CALL_MARKER: &str = "<|call|>";

/// Matches openclaw `isPlainTextToolNameChar`: `[A-Za-z0-9_-]`.
fn is_plain_text_tool_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Matches openclaw `isXmlishNameChar`: `[A-Za-z0-9_.:-]`.
fn is_xmlish_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-')
}

/// Skips spaces and tabs only, preserving line boundaries for grammar decisions.
fn skip_horizontal_whitespace(s: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < s.len() && matches!(s[i], b' ' | b'\t') {
        i += 1;
    }
    i
}

/// Skips ASCII whitespace when line structure is no longer meaningful.
fn skip_whitespace(s: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < s.len() && s[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Consumes either Unix or Windows line endings; returns the first offset after
/// them, or `None` if there is no line break at `start`.
fn consume_line_break(s: &[u8], start: usize) -> Option<usize> {
    match s.get(start) {
        Some(b'\r') => Some(if s.get(start + 1) == Some(&b'\n') {
            start + 2
        } else {
            start + 1
        }),
        Some(b'\n') => Some(start + 1),
        _ => None,
    }
}

/// ASCII case-insensitive prefix compare at `cursor`. Openclaw tags (`<function=`,
/// `</parameter>`, `</function>`) are ASCII, so byte-level `eq_ignore_ascii_case`
/// matches the TS `/i` regex semantics without locale rules.
fn starts_with_ascii_ignore_case(s: &[u8], cursor: usize, marker: &[u8]) -> bool {
    s.get(cursor..cursor + marker.len())
        .map(|rest| rest.eq_ignore_ascii_case(marker))
        .unwrap_or(false)
}

/// Finds the exclusive end offset of a balanced JSON object starting at `start`.
///
/// Hand-written brace balancing (not `serde_json` streaming) so a truncated or
/// oversized object is rejected as `None` rather than erroring. Mirrors openclaw
/// `findJsonObjectEnd`. Byte-level scan is safe here: every control char we
/// branch on (`{` `}` `"` `\`) is a single-byte ASCII codepoint that never
/// appears inside a UTF-8 continuation byte (0x80–0xBF).
fn find_json_object_end(text: &[u8], start: usize, max_payload_bytes: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut index = start;
    while index < text.len() {
        // openclaw guard: `index + 1 - start > maxPayloadBytes`.
        if index + 1 > start + max_payload_bytes {
            return None;
        }
        let c = text[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// Parsed tool-call opening: name + where the payload begins + closing shape.
struct Opening {
    name: String,
    end: usize,
    /// `[tool:NAME]` may be followed by `</function>` (optional) when used with
    /// XML-ish parameter blocks; other forms require a strict closing.
    allows_optional_xmlish_close: bool,
    /// `[NAME]\n` requires `[END_TOOL_REQUEST]` / `[/NAME]`; `[tool:NAME]` and
    /// harmony / xmlish openers do not.
    requires_closing: bool,
}

/// Parses `[NAME]\n`, `[tool:NAME] ` openings. Mirrors openclaw
/// `parseBracketOpening`.
fn parse_bracket_opening(s: &str, start: usize) -> Option<Opening> {
    let bytes = s.as_bytes();
    if bytes.get(start) != Some(&b'[') {
        return None;
    }
    let mut cursor = start + 1;
    // `[tool:NAME]` — inline, no line break, payload directly follows.
    if bytes[cursor..].starts_with(b"tool:") {
        cursor += b"tool:".len();
        let name_start = cursor;
        while cursor < bytes.len() && is_plain_text_tool_name_char(bytes[cursor]) {
            cursor += 1;
        }
        if cursor == name_start || bytes.get(cursor) != Some(&b']') {
            return None;
        }
        return Some(Opening {
            name: s[name_start..cursor].to_string(),
            end: cursor + 1,
            allows_optional_xmlish_close: true,
            requires_closing: false,
        });
    }
    // `[NAME]\n` — multi-line form, payload on the next line.
    let name_start = cursor;
    while cursor < bytes.len() && is_plain_text_tool_name_char(bytes[cursor]) {
        cursor += 1;
    }
    if cursor == name_start || bytes.get(cursor) != Some(&b']') {
        return None;
    }
    let name = s[name_start..cursor].to_string();
    cursor += 1;
    cursor = skip_horizontal_whitespace(bytes, cursor);
    let after_line_break = consume_line_break(bytes, cursor)?;
    Some(Opening {
        name,
        end: after_line_break,
        allows_optional_xmlish_close: false,
        requires_closing: true,
    })
}

/// Parses Harmony `commentary to=NAME code {...}` openings (with optional
/// `<|channel|>` prefix and `<|message|>` separator). Mirrors openclaw
/// `parseHarmonyOpening`.
fn parse_harmony_opening(s: &str, start: usize) -> Option<Opening> {
    let bytes = s.as_bytes();
    let mut cursor = start;
    if bytes[cursor..].starts_with(HARMONY_CHANNEL_MARKER.as_bytes()) {
        cursor += HARMONY_CHANNEL_MARKER.len();
    }
    let channel_start = cursor;
    while cursor < bytes.len() && matches!(bytes[cursor], b'A'..=b'Z' | b'a'..=b'z' | b'_') {
        cursor += 1;
    }
    let channel = &bytes[channel_start..cursor];
    if channel != b"commentary" && channel != b"analysis" && channel != b"final" {
        return None;
    }
    cursor = skip_horizontal_whitespace(bytes, cursor);
    if !bytes[cursor..].starts_with(b"to=") {
        return None;
    }
    cursor += b"to=".len();
    let name_start = cursor;
    while cursor < bytes.len() && is_plain_text_tool_name_char(bytes[cursor]) {
        cursor += 1;
    }
    if cursor == name_start {
        return None;
    }
    let name = s[name_start..cursor].to_string();
    cursor = skip_horizontal_whitespace(bytes, cursor);
    if !bytes[cursor..].starts_with(b"code") {
        return None;
    }
    cursor += b"code".len();
    cursor = skip_whitespace(bytes, cursor);
    if bytes[cursor..].starts_with(HARMONY_MESSAGE_MARKER.as_bytes()) {
        cursor = skip_whitespace(bytes, cursor + HARMONY_MESSAGE_MARKER.len());
    }
    Some(Opening {
        name,
        end: cursor,
        allows_optional_xmlish_close: false,
        requires_closing: false,
    })
}

/// Parses `<function=NAME>` openings. Mirrors openclaw
/// `parseXmlishFunctionOpening` (`/^<function=([A-Za-z0-9_.:-]{1,120})>\s*/i`).
fn parse_xmlish_function_opening(s: &str, start: usize) -> Option<Opening> {
    let bytes = s.as_bytes();
    let rest = bytes.get(start..)?;
    let prefix = b"<function=";
    if rest.len() < prefix.len() || !rest[..prefix.len()].eq_ignore_ascii_case(prefix) {
        return None;
    }
    let mut cursor = start + prefix.len();
    let name_start = cursor;
    while cursor < bytes.len() && is_xmlish_name_char(bytes[cursor]) {
        cursor += 1;
    }
    let name_end = cursor; // ASCII boundary — name chars are all single-byte.
    let name_len = name_end - name_start;
    if name_len == 0 || name_len > 120 || bytes.get(cursor) != Some(&b'>') {
        return None;
    }
    cursor += 1;
    cursor = skip_whitespace(bytes, cursor);
    Some(Opening {
        name: s[name_start..name_end].to_string(),
        end: cursor,
        allows_optional_xmlish_close: false,
        requires_closing: false,
    })
}

/// Bracket-or-Harmony opening (JSON payload path).
fn parse_opening(s: &str, start: usize) -> Option<Opening> {
    parse_bracket_opening(s, start).or_else(|| parse_harmony_opening(s, start))
}

/// Bracket-or-XML-function opening (parameter-block payload path).
fn parse_xmlish_opening(s: &str, start: usize) -> Option<Opening> {
    parse_bracket_opening(s, start).or_else(|| parse_xmlish_function_opening(s, start))
}

/// Consumes a balanced JSON object starting at/after `start`; returns its
/// exclusive end offset and the parsed object. Mirrors openclaw
/// `consumeJsonObject` — rejects non-object (array / scalar) payloads.
fn consume_json_object(
    s: &str,
    start: usize,
    max_payload_bytes: usize,
) -> Option<(usize, serde_json::Value)> {
    let bytes = s.as_bytes();
    let cursor = skip_whitespace(bytes, start);
    if bytes.get(cursor) != Some(&b'{') {
        return None;
    }
    let end = find_json_object_end(bytes, cursor, max_payload_bytes)?;
    let raw = s.get(cursor..end)?;
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    if !parsed.is_object() {
        return None;
    }
    Some((end, parsed))
}

/// Path-A closing: `[END_TOOL_REQUEST]` or `[/NAME]` when required, else an
/// optional `<|call|>` Harmony marker.
fn parse_closing(s: &str, start: usize, name: &str, requires_closing: bool) -> Option<usize> {
    let bytes = s.as_bytes();
    if requires_closing {
        let cursor = skip_whitespace(bytes, start);
        if bytes[cursor..].starts_with(END_TOOL_REQUEST.as_bytes()) {
            return Some(cursor + END_TOOL_REQUEST.len());
        }
        let named = format!("[/{name}]");
        if bytes[cursor..].starts_with(named.as_bytes()) {
            return Some(cursor + named.len());
        }
        None
    } else {
        // parseOptionalHarmonyClosing: returns `start` (not cursor) when no marker.
        let cursor = skip_whitespace(bytes, start);
        if bytes[cursor..].starts_with(HARMONY_CALL_MARKER.as_bytes()) {
            Some(cursor + HARMONY_CALL_MARKER.len())
        } else {
            Some(start)
        }
    }
}

/// A parsed plain-text tool call: name + structured arguments + source span.
#[derive(Debug, Clone, PartialEq)]
pub struct PlainTextToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Bounds of a single `<parameter=NAME>...</parameter>` block.
struct ParamBounds {
    payload_start: usize,
    close_start: usize,
    end: usize,
    name: String,
}

/// Finds a `<parameter=NAME>` … `</parameter>` block starting at/after `start`.
/// Mirrors openclaw `findXmlishParameterBlock` (case-insensitive tags).
fn find_xmlish_parameter_block(s: &str, start: usize) -> Option<ParamBounds> {
    let bytes = s.as_bytes();
    let cursor = skip_whitespace(bytes, start);
    let open_marker = b"<parameter=";
    let rest = bytes.get(cursor..)?;
    if rest.len() < open_marker.len() || !rest[..open_marker.len()].eq_ignore_ascii_case(open_marker)
    {
        return None;
    }
    let mut name_cursor = cursor + open_marker.len();
    let name_start = name_cursor;
    while name_cursor < bytes.len() && is_xmlish_name_char(bytes[name_cursor]) {
        name_cursor += 1;
    }
    let name_len = name_cursor - name_start;
    if name_len == 0 || name_len > 120 || bytes.get(name_cursor) != Some(&b'>') {
        return None;
    }
    let payload_start = name_cursor + 1;

    // First case-insensitive `</parameter>` at/after payload_start.
    let close_marker = b"</parameter>";
    let mut search = payload_start;
    let close_start = loop {
        if search + close_marker.len() > bytes.len() {
            return None;
        }
        if bytes[search..search + close_marker.len()].eq_ignore_ascii_case(close_marker) {
            break search;
        }
        search += 1;
    };
    let close_end = close_start + close_marker.len();
    Some(ParamBounds {
        payload_start,
        close_start,
        end: close_end,
        name: s[name_start..name_cursor].to_string(),
    })
}

/// Trims a leading line break after the opening tag and a trailing newline/CR
/// before the close tag. Mirrors openclaw `extractXmlishParameterValue`.
fn extract_xmlish_parameter_value(s: &str, bounds: &ParamBounds) -> String {
    let bytes = s.as_bytes();
    let mut payload_start = bounds.payload_start;
    let mut payload_end = bounds.close_start;
    if let Some(after) = consume_line_break(bytes, payload_start) {
        payload_start = after;
        if payload_end > payload_start && bytes.get(payload_end - 1) == Some(&b'\n') {
            payload_end -= 1;
            if payload_end > payload_start && bytes.get(payload_end - 1) == Some(&b'\r') {
                payload_end -= 1;
            }
        } else if payload_end > payload_start && bytes.get(payload_end - 1) == Some(&b'\r') {
            payload_end -= 1;
        }
    }
    s[payload_start..payload_end].to_string()
}

/// Consumes `</function>` (strict) at/after `start`. Mirrors openclaw
/// `consumeXmlishFunctionClose` (case-insensitive).
fn consume_xmlish_function_close(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let cursor = skip_whitespace(bytes, start);
    if starts_with_ascii_ignore_case(bytes, cursor, b"</function>") {
        Some(cursor + b"</function>".len())
    } else {
        None
    }
}

/// Parses one plain-text tool call at `start` (JSON payload path: bracket or
/// Harmony opening). Mirrors openclaw `parsePlainTextToolCallBlockAt`.
fn parse_block_at(
    s: &str,
    start: usize,
    max_payload_bytes: usize,
    allowlist: Option<&[String]>,
) -> Option<PlainTextToolCall> {
    let opening = parse_opening(s, start)?;
    if !name_allowed(&opening.name, allowlist) {
        return None;
    }
    let (payload_end, value) = consume_json_object(s, opening.end, max_payload_bytes)?;
    let closing_end = parse_closing(s, payload_end, &opening.name, opening.requires_closing)?;
    let _ = closing_end; // span end not needed for the promoted call
    Some(PlainTextToolCall {
        name: opening.name,
        arguments: value,
    })
}

/// Parses one plain-text tool call at `start` (XML parameter-block path).
/// Mirrors openclaw `parseXmlishPlainTextToolCallBlockAt`.
fn parse_xmlish_block_at(
    s: &str,
    start: usize,
    max_payload_bytes: usize,
    allowlist: Option<&[String]>,
) -> Option<PlainTextToolCall> {
    let opening = parse_xmlish_opening(s, start)?;
    if !name_allowed(&opening.name, allowlist) {
        return None;
    }
    let mut args = serde_json::Map::new();
    let mut cursor = opening.end;
    let mut parameter_count = 0usize;
    while let Some(bounds) = find_xmlish_parameter_block(s, cursor) {
        if bounds.end - opening.end > max_payload_bytes {
            return None;
        }
        let value = extract_xmlish_parameter_value(s, &bounds);
        args.insert(bounds.name, serde_json::Value::String(value));
        parameter_count += 1;
        cursor = bounds.end;
    }
    if parameter_count == 0 {
        return None;
    }
    let end = if opening.allows_optional_xmlish_close {
        consume_xmlish_function_close(s, cursor).unwrap_or(cursor)
    } else {
        consume_xmlish_function_close(s, cursor)?
    };
    let _ = end;
    Some(PlainTextToolCall {
        name: opening.name,
        arguments: serde_json::Value::Object(args),
    })
}

fn name_allowed(name: &str, allowlist: Option<&[String]>) -> bool {
    match allowlist {
        Some(names) => names.iter().any(|n| n == name),
        None => true,
    }
}

/// Parses all plain-text tool-call blocks in `text` (all-or-nothing: if any
/// trailing text cannot be parsed as a block, returns `None`).
///
/// Mirrors openclaw `parseStandalonePlainTextToolCallBlocks`. The top-level
/// scanner skips leading whitespace then demands every remaining byte belong to
/// a tool-call block — this prevents promoting a fragment of ordinary prose
/// that merely starts with `[name]`.
pub fn parse_standalone(text: &str) -> Option<Vec<PlainTextToolCall>> {
    parse_standalone_with(text, None)
}

/// Same as [`parse_standalone`] but restricts accepted tool names to
/// `allowlist` (defends against prompt-injected tool calls invoking tools the
/// agent never advertised).
pub fn parse_standalone_with(text: &str, allowlist: Option<&[String]>) -> Option<Vec<PlainTextToolCall>> {
    let bytes = text.as_bytes();
    let mut cursor = skip_whitespace(bytes, 0);
    let mut blocks = Vec::new();
    while cursor < bytes.len() {
        let block = parse_block_at(text, cursor, DEFAULT_MAX_PAYLOAD_BYTES, allowlist)
            .or_else(|| parse_xmlish_block_at(text, cursor, DEFAULT_MAX_PAYLOAD_BYTES, allowlist))?;
        blocks.push(block);
        cursor = skip_whitespace(bytes, parse_block_span_end(text, cursor));
    }
    if blocks.is_empty() {
        None
    } else {
        Some(blocks)
    }
}

/// Default per-payload size cap. Matches openclaw
/// `DEFAULT_MAX_PLAIN_TEXT_TOOL_PAYLOAD_BYTES`.
const DEFAULT_MAX_PAYLOAD_BYTES: usize = 256_000;

/// To advance the top-level scanner we need the block's end offset; re-parse the
/// span bounds (cheap) instead of threading them through the promoted call.
fn parse_block_span_end(s: &str, start: usize) -> usize {
    // Try the JSON path.
    if let Some(opening) = parse_opening(s, start) {
        if let Some((payload_end, _)) = consume_json_object(s, opening.end, DEFAULT_MAX_PAYLOAD_BYTES)
        {
            if let Some(close) = parse_closing(s, payload_end, &opening.name, opening.requires_closing)
            {
                return close;
            }
        }
    }
    // Try the XML-ish path.
    if let Some(opening) = parse_xmlish_opening(s, start) {
        let mut cursor = opening.end;
        let mut last = cursor;
        while let Some(bounds) = find_xmlish_parameter_block(s, cursor) {
            cursor = bounds.end;
            last = cursor;
        }
        if last > opening.end {
            if opening.allows_optional_xmlish_close {
                return consume_xmlish_function_close(s, last).unwrap_or(last);
            }
            if let Some(close) = consume_xmlish_function_close(s, last) {
                return close;
            }
        }
    }
    start
}

/// Promotes leaked plain-text tool calls in `text` to structured `ToolCall`s.
///
/// Returns `None` when `text` carries no repairable tool call (the common case
/// — an ordinary assistant reply). Each repaired call gets a synthetic id
/// (`repair_<i>`) so the downstream tool-result correlation works the same as a
/// native `tool_use` block.
pub fn repair_plain_text_tool_calls(
    text: &str,
    allowlist: Option<&[String]>,
) -> Option<Vec<ToolCall>> {
    let blocks = parse_standalone_with(text, allowlist)?;
    let repaired: Vec<ToolCall> = blocks
        .into_iter()
        .enumerate()
        .map(|(i, b)| ToolCall {
            id: format!("repair_{i}"),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: b.name,
                arguments: serde_json::to_string(&b.arguments).unwrap_or_else(|_| "{}".into()),
            },
        })
        .collect();
    if repaired.is_empty() {
        None
    } else {
        Some(repaired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bracket_multiline_with_end_marker() {
        let text = "[grep]\n{\"pattern\": \"foo\"}\n[END_TOOL_REQUEST]";
        let repaired = repair_plain_text_tool_calls(text, None).unwrap();
        assert_eq!(repaired.len(), 1);
        assert_eq!(repaired[0].function.name, "grep");
        let v: serde_json::Value = serde_json::from_str(&repaired[0].function.arguments).unwrap();
        assert_eq!(v["pattern"], "foo");
        assert_eq!(repaired[0].id, "repair_0");
        assert_eq!(repaired[0].call_type, "function");
    }

    #[test]
    fn parses_bracket_multiline_with_named_close() {
        let text = "[write_file]\n{\"path\": \"a.txt\", \"content\": \"x\"}\n[/write_file]";
        let repaired = repair_plain_text_tool_calls(text, None).unwrap();
        assert_eq!(repaired[0].function.name, "write_file");
        let v: serde_json::Value = serde_json::from_str(&repaired[0].function.arguments).unwrap();
        assert_eq!(v["path"], "a.txt");
        assert_eq!(v["content"], "x");
    }

    #[test]
    fn parses_tool_prefix_inline() {
        let text = "[tool:edit_file] {\"path\": \"b.rs\", \"old\": \"a\", \"new\": \"b\"}";
        let repaired = repair_plain_text_tool_calls(text, None).unwrap();
        assert_eq!(repaired[0].function.name, "edit_file");
        let v: serde_json::Value = serde_json::from_str(&repaired[0].function.arguments).unwrap();
        assert_eq!(v["old"], "a");
    }

    #[test]
    fn parses_harmony_with_channel_and_call_markers() {
        let text =
            "<|channel|>commentary to=run code {\"cmd\": \"ls\"}<|call|>";
        let repaired = repair_plain_text_tool_calls(text, None).unwrap();
        assert_eq!(repaired[0].function.name, "run");
        let v: serde_json::Value = serde_json::from_str(&repaired[0].function.arguments).unwrap();
        assert_eq!(v["cmd"], "ls");
    }

    #[test]
    fn parses_harmony_without_channel_marker() {
        // analysis channel, no <|channel|> prefix, no <|call|> suffix.
        let text = "analysis to=search code {\"q\": \"r\"}";
        let repaired = repair_plain_text_tool_calls(text, None).unwrap();
        assert_eq!(repaired[0].function.name, "search");
    }

    #[test]
    fn parses_xmlish_function_with_parameters() {
        let text = "<function=read_file><parameter=path>src/lib.rs</parameter></function>";
        let repaired = repair_plain_text_tool_calls(text, None).unwrap();
        assert_eq!(repaired[0].function.name, "read_file");
        let v: serde_json::Value = serde_json::from_str(&repaired[0].function.arguments).unwrap();
        assert_eq!(v["path"], "src/lib.rs");
    }

    #[test]
    fn parses_xmlish_multiline_parameter_trims_line_breaks() {
        let text = "<function=write>\n<parameter=content>\nline1\nline2\n</parameter>\n</function>";
        let repaired = repair_plain_text_tool_calls(text, None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&repaired[0].function.arguments).unwrap();
        assert_eq!(v["content"], "line1\nline2");
    }

    #[test]
    fn parses_multiple_blocks_all_or_nothing() {
        let text = "[a]\n{\"x\": 1}\n[END_TOOL_REQUEST]\n[b]\n{\"y\": 2}\n[END_TOOL_REQUEST]";
        let repaired = repair_plain_text_tool_calls(text, None).unwrap();
        assert_eq!(repaired.len(), 2);
        assert_eq!(repaired[0].function.name, "a");
        assert_eq!(repaired[1].function.name, "b");
        assert_eq!(repaired[0].id, "repair_0");
        assert_eq!(repaired[1].id, "repair_1");
    }

    #[test]
    fn returns_none_for_ordinary_prose() {
        // Starts with `[done]`-ish but has trailing prose → all-or-nothing fails.
        let text = "Here is my answer: the file is fine.";
        assert!(repair_plain_text_tool_calls(text, None).is_none());
    }

    #[test]
    fn returns_none_when_trailing_text_is_prose() {
        // A valid block followed by prose must NOT be partially promoted.
        let text = "[grep]\n{\"pattern\": \"foo\"}\n[END_TOOL_REQUEST]\nExplanation: done.";
        assert!(repair_plain_text_tool_calls(text, None).is_none());
    }

    #[test]
    fn allowlist_filters_unknown_tool() {
        let text = "[evil]\n{\"x\": 1}\n[END_TOOL_REQUEST]";
        let allow = vec!["grep".to_string(), "read_file".to_string()];
        assert!(repair_plain_text_tool_calls(text, Some(&allow)).is_none());
    }

    #[test]
    fn allowlist_admits_known_tool() {
        let text = "[grep]\n{\"pattern\": \"foo\"}\n[END_TOOL_REQUEST]";
        let allow = vec!["grep".to_string()];
        let repaired = repair_plain_text_tool_calls(text, Some(&allow)).unwrap();
        assert_eq!(repaired[0].function.name, "grep");
    }

    #[test]
    fn json_string_with_braces_does_not_break_balancing() {
        // A `}` inside a JSON string literal must not fool brace balancing.
        let text = "[bash]\n{\"cmd\": \"echo {not-the-end}\"}\n[END_TOOL_REQUEST]";
        let repaired = repair_plain_text_tool_calls(text, None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&repaired[0].function.arguments).unwrap();
        assert_eq!(v["cmd"], "echo {not-the-end}");
    }

    #[test]
    fn nested_json_object_balances() {
        let text = "[t]\n{\"opts\": {\"a\": 1, \"b\": {\"c\": 2}}}\n[END_TOOL_REQUEST]";
        let repaired = repair_plain_text_tool_calls(text, None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&repaired[0].function.arguments).unwrap();
        assert_eq!(v["opts"]["b"]["c"], 2);
    }

    #[test]
    fn find_json_object_end_rejects_truncated() {
        let bytes = b"{\"a\": 1"; // no closing brace
        assert!(find_json_object_end(bytes, 0, 256_000).is_none());
    }

    #[test]
    fn find_json_object_end_respects_max_bytes() {
        let bytes = b"{\"a\": 1}";
        // max=2 means we stop scanning before the closing brace.
        assert!(find_json_object_end(bytes, 0, 2).is_none());
    }

    #[test]
    fn xmlish_function_case_insensitive_tags() {
        let text = "<FUNCTION=read_file><PARAMETER=path>x.rs</PARAMETER></FUNCTION>";
        let repaired = repair_plain_text_tool_calls(text, None).unwrap();
        assert_eq!(repaired[0].function.name, "read_file");
        let v: serde_json::Value = serde_json::from_str(&repaired[0].function.arguments).unwrap();
        assert_eq!(v["path"], "x.rs");
    }

    #[test]
    fn harmony_message_marker_before_payload() {
        let text = "commentary to=run code<|message|>{\"cmd\": \"pwd\"}";
        let repaired = repair_plain_text_tool_calls(text, None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&repaired[0].function.arguments).unwrap();
        assert_eq!(v["cmd"], "pwd");
    }

    #[test]
    fn rejects_array_payload() {
        // A JSON array is not a tool-call argument object.
        let text = "[t]\n[1, 2, 3]\n[END_TOOL_REQUEST]";
        assert!(repair_plain_text_tool_calls(text, None).is_none());
    }

    #[test]
    fn bracket_requires_closing_marker() {
        // `[NAME]\n{...}` without `[END_TOOL_REQUEST]`/`[/NAME]` → not closed.
        let text = "[grep]\n{\"pattern\": \"foo\"}";
        assert!(repair_plain_text_tool_calls(text, None).is_none());
    }

    #[test]
    fn unicode_in_arguments_preserved() {
        let text = "[write]\n{\"msg\": \"你好世界 🦀\"}\n[END_TOOL_REQUEST]";
        let repaired = repair_plain_text_tool_calls(text, None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&repaired[0].function.arguments).unwrap();
        assert_eq!(v["msg"], "你好世界 🦀");
    }
}
