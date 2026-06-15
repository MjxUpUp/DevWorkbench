use crate::models::{AgentType, ContextSnapshot, FileDiff, Session, SessionStatus};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::sync::LazyLock;
use tauri::Emitter;

/// Max chars kept as output summary (truncated from tail of output)
const OUTPUT_SUMMARY_MAX_CHARS: usize = 2000;

/// Maximum time a pipe session may run before being force-killed.
/// Kill a session that produces NO output for this many seconds. A healthy long
/// task streams continuously (stream-json events / tool output) and never idles
/// out; only a truly hung process (zero stdout+stderr) trips this. Override via
/// the DEVWORKBENCH_SESSION_IDLE_TIMEOUT_SECS env var; 0 disables the timeout
/// entirely (the agent runs until it exits on its own).
const DEFAULT_SESSION_IDLE_TIMEOUT_SECS: u64 = 300;

/// Resolve the idle timeout from the env override, falling back to the default.
fn session_idle_timeout_secs() -> u64 {
    std::env::var("DEVWORKBENCH_SESSION_IDLE_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SESSION_IDLE_TIMEOUT_SECS)
}

/// Wall-clock millis for the idle tracker. SystemTime (not Instant) because the
/// value is shared across threads via AtomicU64; a non-monotonic clock jump is
/// harmless for an "is it still producing output" heuristic.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Cache resolved agent exe paths to avoid repeated PATH scanning.
static EXE_CACHE: LazyLock<Mutex<HashMap<String, PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ---------------------------------------------------------------------------
// Tracked process types
// ---------------------------------------------------------------------------

/// A tracked process. PTY support has been removed — all target CLIs run in
/// non-interactive pipe mode, so every session holds the spawned child PID.
enum TrackedProcess {
    Pipe(u32),
}

/// Active agent processes tracked by session ID.
pub struct AgentProcesses {
    processes: Mutex<HashMap<String, TrackedProcess>>,
}

impl AgentProcesses {
    pub fn new() -> Self {
        Self {
            processes: Mutex::new(HashMap::new()),
        }
    }
}

// ---------------------------------------------------------------------------
// Agent executable resolution (platform-specific)
// ---------------------------------------------------------------------------

/// On Windows, resolve the real .exe for npm-installed CLI tools.
#[cfg(target_os = "windows")]
fn resolve_agent_exe(agent_type: &AgentType) -> Result<PathBuf, String> {
    let path = crate::commands::tools::which_expanded(agent_type.command_name())
        .ok_or_else(|| format!("{} 未安装", agent_type.display_name()))?;

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext.eq_ignore_ascii_case("exe") {
        return Ok(path);
    }

    if let Some(parent) = path.parent() {
        let nm_dir = parent.join("node_modules");
        if nm_dir.exists() {
            let mappings: &[(&str, &str, &str)] = &[
                ("claude", "@anthropic-ai/claude-code", "claude.exe"),
            ];
            for &(name, pkg, exe_name) in mappings {
                if name == agent_type.command_name() {
                    let candidate = nm_dir.join(pkg).join("bin").join(exe_name);
                    if candidate.exists() {
                        return Ok(candidate);
                    }
                }
            }
        }
    }

    let with_exe = path.with_extension("exe");
    if with_exe.exists() {
        return Ok(with_exe);
    }

    Ok(path)
}

#[cfg(not(target_os = "windows"))]
fn resolve_agent_exe(agent_type: &AgentType) -> Result<PathBuf, String> {
    crate::commands::tools::which_expanded(agent_type.command_name())
        .ok_or_else(|| format!("{} 未安装", agent_type.display_name()))
}

// ---------------------------------------------------------------------------
// Command building
// ---------------------------------------------------------------------------

/// How the reader thread should interpret the agent's stdout.
#[derive(Clone, Copy, PartialEq, Debug)]
enum OutputMode {
    /// Raw byte stream — emitted verbatim to the terminal. Used by every CLI
    /// whose non-interactive mode already streams human-readable text
    /// (codex/gemini/pi/etc.).
    Raw,
    /// `claude --output-format stream-json`: each stdout line is one JSON event.
    /// The reader parses each event and renders human-readable text (assistant
    /// text, tool calls, results) so the user watches progress live. claude's
    /// DEFAULT text mode buffers all output until process exit when stdout is a
    /// non-TTY pipe — so without stream-json the terminal shows only
    /// "Agent 运行中，等待输出..." for the entire run.
    ClaudeStreamJson,
}

/// Resolved spawn parameters for an agent.
struct SpawnConfig {
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    /// If set, the prompt is delivered via stdin instead of as a CLI arg.
    stdin_prompt: Option<String>,
    output_mode: OutputMode,
}

/// Cached version of `resolve_agent_exe`. Avoids PATH scanning on every spawn.
fn resolve_agent_exe_cached(agent_type: &AgentType) -> Result<PathBuf, String> {
    let key = agent_type.command_name().to_string();
    {
        let cache = EXE_CACHE.lock().map_err(|e| format!("EXE 缓存锁失败: {}", e))?;
        if let Some(path) = cache.get(&key) {
            return Ok(path.clone());
        }
    }
    let path = resolve_agent_exe(agent_type)?;
    let mut cache = EXE_CACHE.lock().map_err(|e| format!("EXE 缓存锁失败: {}", e))?;
    cache.insert(key, path.clone());
    Ok(path)
}

/// Build the program + args for an agent invocation (shared by PTY and pipe paths).
fn build_spawn_config(
    agent_type: &AgentType,
    project_path: &str,
    prompt: &str,
    model: Option<&str>,
) -> Result<SpawnConfig, String> {
    let exe = resolve_agent_exe_cached(agent_type)?;
    let mut args: Vec<String> = Vec::new();

    // Prompt delivery: `claude --print` reads the prompt from stdin. Passing it
    // as a CLI arg instead makes every line starting with "-" in injected file
    // content parse as an unknown option (user-visible "unknown option" error
    // when attaching @file references). So ClaudeCode always goes through stdin;
    // the other CLIs fall back to argv but switch to stdin when the prompt is
    // large (Windows CLI ~32K limit, and injected file content easily exceeds it).
    const STDIN_PROMPT_THRESHOLD: usize = 4096;
    let use_stdin = matches!(agent_type, AgentType::ClaudeCode | AgentType::Pi)
        || prompt.len() > STDIN_PROMPT_THRESHOLD;
    let stdin_prompt = if use_stdin { Some(prompt.to_string()) } else { None };

    match agent_type {
        AgentType::ClaudeCode => {
            args.push("--print".to_string());
            // stream-json: emit one JSON event per line in REALTIME (system init,
            // assistant text, tool_use, tool_result, final result) instead of
            // buffering the entire run and dumping it at exit — which is what
            // claude's default text mode does when stdout is a non-TTY pipe. The
            // reader thread parses each line and renders human-readable text, so
            // the user watches the agent work instead of staring at
            // "Agent 运行中，等待输出..." for minutes. --verbose is required by
            // stream-json (surfaces tool calls + intermediate user/tool turns).
            args.push("--output-format".to_string());
            args.push("stream-json".to_string());
            args.push("--verbose".to_string());
            // No `--continue`/`--resume`: claude's bare `--continue` resumes the
            // "most recent conversation in [cwd]" — a claude-internal notion of
            // "recent" that does NOT correspond to this DevWorkbench conversation.
            // With users switching across conversations/projects, `--continue`
            // resumes an unrelated session and the agent answers off-topic (the
            // "答非所问" symptom). Continuity is provided instead by
            // inject_conversation_context (a summary of THIS conversation's prior
            // turns), which is consistent for same-agent AND cross-agent turns.
            // To restore claude's native full context later: store claude's session
            // id (parse --output-format json) and use `--resume <id>`.
            // Prompt is delivered via stdin (see use_stdin above) — never as argv.
            if let Some(m) = model {
                args.push("--model".to_string());
                args.push(m.to_string());
            }
        }
        AgentType::Codex => {
            args.push("exec".to_string());
            args.push(prompt.to_string());
            if let Some(m) = model {
                args.push("--model".to_string());
                args.push(m.to_string());
            }
        }
        AgentType::GeminiCli => {
            args.push("--prompt".to_string());
            args.push(prompt.to_string());
        }
        AgentType::CursorAgent => {
            args.push("agent".to_string());
            args.push("--prompt".to_string());
            args.push(prompt.to_string());
        }
        AgentType::Copilot => {
            args.push("copilot".to_string());
            args.push("suggest".to_string());
            args.push("--prompt".to_string());
            args.push(prompt.to_string());
        }
        AgentType::QwenCode => {
            args.push("--prompt".to_string());
            args.push(prompt.to_string());
        }
        AgentType::Pi => {
            // pi CLI: `pi --print` (-p) is non-interactive and reads the prompt
            // from STDIN (verified against pi 0.79.3 — `echo x | pi -p` replies;
            // `pi --prompt x` dies "Unknown option: --prompt"). The prompt is
            // delivered via stdin (see use_stdin above), NEVER as a --prompt flag
            // or positional argv. This mirrors how ClaudeCode is invoked.
            args.push("--print".to_string());
            if let Some(m) = model {
                args.push("--model".to_string());
                args.push(m.to_string());
            }
        }
        // Defensive: a kernel agent is never spawned as a CLI subprocess —
        // spawn_agent_session routes kernel=true to react_chat_driver before
        // build_spawn_config is reached, and resolve_agent_exe_cached would
        // already error on command_name="". This arm only keeps the match
        // exhaustive; reaching it is a routing bug.
        AgentType::ReactKernel => {
            return Err("ReactKernel agent must not reach the pty spawn path".into());
        }
    }

    Ok(SpawnConfig {
        program: exe,
        args,
        cwd: PathBuf::from(project_path),
        stdin_prompt,
        output_mode: if matches!(agent_type, AgentType::ClaudeCode) {
            OutputMode::ClaudeStreamJson
        } else {
            OutputMode::Raw
        },
    })
}

// ---------------------------------------------------------------------------
// claude stream-json parsing
// ---------------------------------------------------------------------------
//
// claude `--output-format stream-json` emits one JSON event per line in
// realtime (vs its default text mode, which buffers everything until exit when
// stdout is a non-TTY pipe). The old monolithic renderer is split into two
// passes so the structured blocks can ALSO feed a future `agent:event`
// channel for chat-style block rendering — not just the ANSI terminal text:
//
//   parse_claude_line(line) -> Vec<ClaudeBlock>   (structured, lossless)
//   render_blocks(&blocks)  -> Option<String>     (ANSI text, byte-identical
//                                                  to the old single-pass render)
//
// Parsed with `serde_json::Value` rather than a full struct schema: claude's
// event shapes vary across versions and we need only a couple of fields per
// type, so a tolerant parse + `.get()` chain degrades gracefully (unknown or
// missing fields ⇒ skip, never panic) instead of failing the whole stream on
// one odd line.

/// One structured piece of a `claude --output-format stream-json` event line.
#[derive(Debug, Clone, PartialEq)]
pub enum ClaudeBlock {
    /// assistant text content (non-empty).
    Text { content: String },
    /// assistant tool_use: tool name + its raw input object. `None` when claude
    /// omits `input`, so render stays byte-identical to the legacy preview.
    ToolUse {
        name: String,
        input: Option<serde_json::Value>,
    },
    /// a user tool_result turn. `content` is the raw result value; `is_error`
    /// is best-effort (claude sometimes omits the flag).
    ToolResult {
        tool_use_id: Option<String>,
        content: Option<serde_json::Value>,
        is_error: Option<bool>,
    },
    /// terminal result event. `is_error` is the synthesized verdict
    /// (`is_error || subtype != "success"`), matching the legacy render branch.
    Result { is_error: bool, secs: u64 },
}

/// Parse one `claude --output-format stream-json` line into zero or more
/// structured blocks. Returns an empty Vec for events with no user-facing
/// content (system / api_retry) or malformed lines — never panics, so one odd
/// line never breaks the stream.
pub fn parse_claude_line(line: &str) -> Vec<ClaudeBlock> {
    let v: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let ty = match v.get("type").and_then(|s| s.as_str()) {
        Some(t) => t,
        None => return Vec::new(),
    };
    match ty {
        "assistant" => {
            let content = match v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                Some(c) => c,
                None => return Vec::new(),
            };
            let mut blocks = Vec::new();
            for block in content {
                match block.get("type").and_then(|s| s.as_str()).unwrap_or("") {
                    "text" => {
                        if let Some(t) = block.get("text").and_then(|s| s.as_str()) {
                            if !t.is_empty() {
                                blocks.push(ClaudeBlock::Text { content: t.to_string() });
                            }
                        }
                    }
                    "tool_use" => {
                        let name = block
                            .get("name")
                            .and_then(|s| s.as_str())
                            .unwrap_or("tool")
                            .to_string();
                        let input = block.get("input").cloned();
                        blocks.push(ClaudeBlock::ToolUse { name, input });
                    }
                    _ => {}
                }
            }
            blocks
        }
        "user" => {
            // tool_result turns — a short result preview proves the tool
            // returned and the agent is making progress.
            let content = match v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                Some(c) => c,
                None => return Vec::new(),
            };
            let mut blocks = Vec::new();
            for block in content {
                if block.get("type").and_then(|s| s.as_str()) == Some("tool_result") {
                    let tool_use_id = block
                        .get("tool_use_id")
                        .and_then(|s| s.as_str())
                        .map(|s| s.to_string());
                    let content = block.get("content").cloned();
                    let is_error = block.get("is_error").and_then(|b| b.as_bool());
                    blocks.push(ClaudeBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    });
                }
            }
            blocks
        }
        "result" => {
            let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
            let dur_ms = v.get("duration_ms").and_then(|d| d.as_u64()).unwrap_or(0);
            let is_error = v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false);
            let secs = dur_ms / 1000;
            vec![ClaudeBlock::Result {
                is_error: is_error || subtype != "success",
                secs,
            }]
        }
        // "system" (init / api_retry) carries no user-facing progress — skip.
        _ => Vec::new(),
    }
}

/// Render parsed blocks into ANSI-styled text for the terminal. Returns None
/// when there is nothing to show (zero blocks). Output is byte-identical to
/// the old single-pass renderer — the render contract the terminal replay and
/// `{sid}.log` depend on is preserved exactly (see the golden test).
pub fn render_blocks(blocks: &[ClaudeBlock]) -> Option<String> {
    let mut out = String::new();
    for block in blocks {
        match block {
            ClaudeBlock::Text { content } => {
                out.push_str(content);
                out.push('\n');
            }
            ClaudeBlock::ToolUse { name, input } => {
                let preview = json_preview(input.as_ref(), 80);
                out.push_str(&format!("\x1b[36m🔧 {} \x1b[90m{}\x1b[0m\n", name, preview));
            }
            ClaudeBlock::ToolResult { content, .. } => {
                let preview = json_preview(content.as_ref(), 120);
                out.push_str(&format!("\x1b[90m  ↳ {}\x1b[0m\n", preview));
            }
            ClaudeBlock::Result { is_error, secs } => {
                if *is_error {
                    out.push_str(&format!("\x1b[31m✗ 失败 ({}s)\x1b[0m\n", secs));
                } else {
                    out.push_str(&format!("\x1b[32m✓ 完成 ({}s)\x1b[0m\n", secs));
                }
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Wire-level structured event for the `agent:event` channel — what the chat
/// frontend renders into block cards. Decoupled from kernel-core's `AgentEvent`
/// (which has no serde derives) so this schema can evolve with the UI without
/// touching the kernel trait layer. Serialized with `kind` as the discriminator
/// tag so the TS union narrows on it.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(tag = "kind")]
pub enum ChatStreamEvent {
    #[serde(rename = "text")]
    Text { content: String },
    #[serde(rename = "tool_use")]
    ToolUse { name: String, input: serde_json::Value },
    #[serde(rename = "tool_result")]
    ToolResult { content: String, is_error: bool },
    #[serde(rename = "result")]
    Result { is_error: bool, secs: u64 },
}

impl ClaudeBlock {
    /// Map a parsed block to its wire event form for the `agent:event` channel.
    pub fn to_event(&self) -> ChatStreamEvent {
        match self {
            ClaudeBlock::Text { content } => ChatStreamEvent::Text { content: content.clone() },
            ClaudeBlock::ToolUse { name, input } => ChatStreamEvent::ToolUse {
                name: name.clone(),
                input: input.clone().unwrap_or(serde_json::Value::Null),
            },
            // ToolResult: collapse the raw content value into a readable string.
            // Longer than the 120-char terminal preview — the chat card folds it,
            // so give it more room to stay useful.
            ClaudeBlock::ToolResult { content, is_error, .. } => ChatStreamEvent::ToolResult {
                content: json_preview(content.as_ref(), 500),
                is_error: is_error.unwrap_or(false),
            },
            ClaudeBlock::Result { is_error, secs } => ChatStreamEvent::Result {
                is_error: *is_error,
                secs: *secs,
            },
        }
    }
}

/// Parse a claude stream-json line into the wire events the chat frontend
/// consumes. Pure + testable (no Tauri handle). The reader thread drives the
/// same parse once and emits BOTH this structured channel and the rendered
/// `pty:output` text from the same `blocks` (no double-parse).
pub fn claude_line_to_events(line: &str) -> Vec<ChatStreamEvent> {
    parse_claude_line(line)
        .iter()
        .map(ClaudeBlock::to_event)
        .collect()
}

/// One-line preview of a JSON value: serialized, newlines collapsed to spaces,
/// truncated to `max` chars so it fits on a single terminal row.
fn json_preview(value: Option<&serde_json::Value>, max: usize) -> String {
    let s = match value {
        Some(v) => serde_json::to_string(v).unwrap_or_default(),
        None => String::new(),
    };
    let collapsed: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    collapsed.chars().take(max).collect()
}

// ---------------------------------------------------------------------------
// Main spawn entry point
// ---------------------------------------------------------------------------

/// Spawn an agent process. Tries real PTY first, falls back to pipe I/O.
pub fn spawn_pty_agent(
    app: &tauri::AppHandle,
    processes: Arc<AgentProcesses>,
    db_conn: crate::db::DbState,
    project_path: &str,
    agent_type: AgentType,
    prompt: &str,
    model: Option<&str>,
    linked_requirement_id: Option<&str>,
    parent_session_id: Option<&str>,
    conversation_id: Option<&str>,
) -> Result<Session, String> {
    // Context bridge: when this turn continues an existing conversation, inject a
    // summary of the prior turns so the new agent inherits the thread of work.
    // This injected summary is the SOLE continuity mechanism — we deliberately do
    // NOT pass claude `--continue` (it resumes claude's cwd-recent session, which
    // is unrelated to this conversation; see build_spawn_config). Same-agent and
    // cross-agent turns are therefore continuous the same way.
    let prior_turns = match conversation_id {
        Some(cid) => load_prior_turns(&db_conn, cid),
        None => Vec::new(),
    };

    // Resolve parent_session_id for the DB record (DevWorkbench parent/child turn
    // relationship — NOT for claude CLI resume). P2's continueConversation passes
    // conversation_id but not parent_session_id, so derive it: link to the last
    // prior turn of the SAME agent. A different agent has no meaningful parent, so
    // we don't fabricate a cross-agent parent link.
    let parent_for_resume = if parent_session_id.is_some() {
        parent_session_id
    } else {
        prior_turns.last().filter(|t| t.agent_type == agent_type).map(|t| t.id.as_str())
    };

    let injected_prompt = inject_conversation_context(prompt, &prior_turns, &agent_type);
    // Inject project knowledge context into the prompt — run in background thread
    // with a 2-second timeout to avoid blocking the UI on slow DB queries.
    let injected_prompt = inject_knowledge_with_timeout(
        &db_conn, &agent_type, project_path, &injected_prompt,
    );
    // Inject @file references with actual file content
    let injected_prompt = inject_file_references(project_path, &injected_prompt);
    let config = build_spawn_config(&agent_type, project_path, &injected_prompt, model)?;

    // Unified pipe mode for all platforms. PTY path removed from runtime:
    // all target CLIs support non-interactive --print/exec pipe mode.
    spawn_pipe_fallback(&app, processes, db_conn, &config, &agent_type, project_path, linked_requirement_id, parent_for_resume, conversation_id, prompt, model)
}

// ---------------------------------------------------------------------------
// Conversation context bridge (cross-agent history injection)
// ---------------------------------------------------------------------------

/// Load the completed prior turns of a conversation (oldest-first), best-effort.
/// The currently-spawning turn isn't in the DB yet, so it's naturally excluded.
/// A DB failure degrades to "no prior history" rather than blocking the spawn.
fn load_prior_turns(db_conn: &crate::db::DbState, conversation_id: &str) -> Vec<crate::models::Session> {
    let Ok(conn) = db_conn.get() else {
        return Vec::new();
    };
    crate::agents::session::load_turns_for_conversation_db(&conn, conversation_id)
        .unwrap_or_default()
}

/// Max chars of prior-turn output to include per turn in the injected summary.
/// Keeps the bridge bounded — a long conversation won't blow the prompt budget.
const CONTEXT_BRIDGE_OUTPUT_MAX_CHARS: usize = 1200;
/// Hard cap on total injected history across all prior turns.
const CONTEXT_BRIDGE_TOTAL_MAX_CHARS: usize = 8000;

/// Build the injected conversation-history prefix for a follow-up turn.
///
/// Each prior turn contributes its agent + user prompt + a tail-truncated slice
/// of its output summary. The whole block is capped at
/// [`CONTEXT_BRIDGE_TOTAL_MAX_CHARS`]; if it exceeds that, only the most recent
/// turns that fit are kept (oldest dropped first) so the immediate thread of
/// work is always preserved. Returns the original prompt unchanged when there
/// is no prior history (first turn of a conversation).
fn inject_conversation_context(
    prompt: &str,
    prior_turns: &[crate::models::Session],
    _current_agent: &AgentType,
) -> String {
    if prior_turns.is_empty() {
        return prompt.to_string();
    }

    // Render each turn into a compact block, then trim from the front until the
    // total fits the cap (keep the newest — the active thread of work).
    let blocks: Vec<String> = prior_turns
        .iter()
        .map(|t| {
            let output = t
                .output_summary
                .as_deref()
                .map(|s| tail(s, CONTEXT_BRIDGE_OUTPUT_MAX_CHARS))
                .unwrap_or_default();
            format!(
                "[Turn — agent: {}]\nUser: {}\nAssistant:\n{}",
                t.agent_type.display_name(),
                t.prompt.trim(),
                output.trim(),
            )
        })
        .collect();

    let mut selected: Vec<String> = blocks.iter().cloned().collect();
    while selected.iter().map(|b| b.len()).sum::<usize>() > CONTEXT_BRIDGE_TOTAL_MAX_CHARS && selected.len() > 1 {
        selected.remove(0);
    }

    let history = selected.join("\n\n");
    format!(
        "You are continuing an existing conversation. Prior turns (for context — do not repeat their work):\n\n{history}\n\n--- Current request ---\n{prompt}",
    )
}

/// Keep the tail of `s` up to `max` chars, snapped to a UTF-8 boundary and
/// `...`-prefixed. Mirrors truncate_tail but operates on an already-decoded str.
fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    format!("...{}", &s[start..])
}

// ---------------------------------------------------------------------------
// PTY path
// ---------------------------------------------------------------------------

#[cfg(any())] // PTY path compiled out — pipe-only
fn try_spawn_pty(
    app: &tauri::AppHandle,
    processes: &Arc<AgentProcesses>,
    db_conn: &crate::db::DbState,
    config: &SpawnConfig,
    agent_type: &AgentType,
    project_path: &str,
    linked_requirement_id: Option<&str>,
    parent_session_id: Option<&str>,
    prompt: &str,
    model: Option<&str>,
) -> Result<Session, String> {
    log::info!(
        "[PTY spawn] program={}, args={:?}, cwd={}",
        config.program.display(),
        config.args,
        config.cwd.display()
    );
    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("PTY openpty 失败: {}", e))?;

    let mut cmd = portable_pty::CommandBuilder::new(&config.program);
    cmd.args(config.args.iter().map(|s| s.as_str()));
    cmd.cwd(&config.cwd);

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("PTY spawn 失败: {}", e))?;

    let _pid = child.process_id().unwrap_or(0);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("PTY clone reader 失败: {}", e))?;

    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("PTY take writer 失败: {}", e))?;

    let killer = child.clone_killer();

    let session_id = uuid::Uuid::new_v4().to_string();
    processes
        .processes
        .lock().unwrap_or_else(|e| e.into_inner())
        .insert(
            session_id.clone(),
            TrackedProcess::Pty(PtyHandles {
                master: pair.master,
                writer,
                killer,
            }),
        );

    // Capture pre-diff in background — agent won't modify files in the first
    // few hundred milliseconds, so racing this thread against the agent is safe.
    {
        let bg_sid = session_id.clone();
        let bg_pp = project_path.to_string();
        std::thread::Builder::new()
            .name("pre-diff-capture".into())
            .spawn(move || {
                let pre_diff = capture_git_diff_names(&bg_pp);
                if let Ok(agents_dir) = crate::agents::session::agents_dir() {
                    let dir = agents_dir.join("outputs");
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(dir.join(format!("{}.pre-diff", bg_sid)), pre_diff.join("\n"));
                }
            })
            .ok();
    }

    let session = Session {
        id: session_id.clone(),
        project_path: project_path.to_string(),
        agent_type: agent_type.clone(),
        status: SessionStatus::Running,
        prompt: prompt.to_string(),
        model: model.map(|m| m.to_string()),
        started_at: chrono::Local::now().to_rfc3339(),
        finished_at: None,
        exit_code: None,
        output_summary: None,
        context_snapshot: None,
        linked_requirement_id: linked_requirement_id.map(|s| s.to_string()),
        parent_session_id: parent_session_id.map(|s| s.to_string()),
    };

    {
        let conn = db_conn.get().map_err(|e| e.to_string())?;
        crate::agents::session::insert_session_db(&conn, &session)
            .map_err(|e| e.to_string())?;
        let _ = crate::activity::record_event(&conn, &crate::activity::make_activity_event(
            &session_id,
            project_path,
            agent_type,
            "session_started",
            &format!("{} session started", agent_type.display_name()),
            None,
            None,
        ));
    }
    let _ = app.emit("agent:started", &session);

    let child_exited = Arc::new(AtomicBool::new(false));

    // Reader thread
    let app_reader = app.clone();
    let sid_reader = session_id.clone();
    let processes_reader = processes.clone();
    let reader_exit_signal = child_exited.clone();
    std::thread::spawn(move || {
        let mut output_log: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        let mut reader = reader;
        let mut total_bytes: usize = 0;
        let mut emit_count: usize = 0;

        log::info!("[PTY reader] Started for session {}", sid_reader);

        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    log::info!("[PTY reader] EOF for session {}, total_bytes={}, emit_count={}", sid_reader, total_bytes, emit_count);
                    break;
                }
                Ok(n) => {
                    total_bytes += n;
                    let data = &buf[..n];
                    output_log.extend_from_slice(data);
                    let bytes_vec: Vec<u8> = data.to_vec();
                    emit_count += 1;
                    if let Err(e) = app_reader.emit(
                        "pty:output",
                        serde_json::json!({
                            "sessionId": sid_reader,
                            "data": bytes_vec,
                        }),
                    ) {
                        log::error!("[PTY reader] emit failed for session {}: {}", sid_reader, e);
                    }
                }
                Err(e) => {
                    if reader_exit_signal.load(Ordering::Acquire) {
                        log::info!("[PTY reader] Read interrupted after child exit for session {}, total_bytes={}, emit_count={}", sid_reader, total_bytes, emit_count);
                        break;
                    }
                    log::error!("[PTY reader] read error for session {}: {}", sid_reader, e);
                    break;
                }
            }
        }

        log::info!("[PTY reader] Finished for session {}, total_bytes={}, chunks={}, log_size={}", sid_reader, total_bytes, emit_count, output_log.len());

        if let Ok(agents_dir) = crate::agents::session::agents_dir() {
            let log_path = agents_dir.join("outputs");
            let _ = std::fs::create_dir_all(&log_path);
            let _ = std::fs::write(log_path.join(format!("{}.log", sid_reader)), &output_log);
        }

        if let Ok(mut map) = processes_reader.processes.lock() {
            map.remove(&sid_reader);
        }
    });

    // Wait thread
    let app_exit = app.clone();
    let sid_exit = session_id.clone();
    let project_path_exit = project_path.to_string();
    let db_conn_exit = db_conn.clone();
    let agent_type_exit = agent_type.clone();
    let processes_wait = processes.clone();
    let wait_exit_signal = child_exited.clone();
    std::thread::spawn(move || {
        log::info!("[PTY wait] Waiting for session {} to exit", sid_exit);
        let status = child.wait();
        log::info!("[PTY wait] Session {} exited: {:?}", sid_exit, status.as_ref().map(|s| s.exit_code()));

        wait_exit_signal.store(true, Ordering::Release);

        if let Ok(mut map) = processes_wait.processes.lock() {
            if map.remove(&sid_exit).is_some() {
                log::info!("[PTY wait] Dropped PTY master for session {}, reader should unblock", sid_exit);
            }
        }

        let (exit_code, session_status): (Option<i32>, SessionStatus) = match status {
            Ok(s) => (
                Some(s.exit_code() as i32),
                if s.success() { SessionStatus::Completed } else { SessionStatus::Failed },
            ),
            Err(_) => (None, SessionStatus::Failed),
        };

        std::thread::sleep(std::time::Duration::from_millis(500));

        // Read output summary FIRST so key_output can be derived from it.
        let output_summary = read_output_summary(&sid_exit).or_else(|| {
            if exit_code.unwrap_or(-1) == 0 {
                Some("(Agent completed with no text output)".to_string())
            } else {
                Some(format!("(Process exited with code {:?})", exit_code))
            }
        });
        log::info!("[completion] Session {} capturing context snapshot...", sid_exit);
        let snapshot = extract_context_snapshot(&project_path_exit, &sid_exit, output_summary.as_deref());
        log::info!(
            "[completion] Session {} snapshot done ({} files changed, {} key_output chars)",
            sid_exit,
            snapshot.as_ref().map(|s| s.files_changed.len()).unwrap_or(0),
            snapshot.as_ref().map(|s| s.key_output.chars().count()).unwrap_or(0)
        );
        let files_for_activity = snapshot.as_ref().map(|s| s.files_changed.clone());

        let mut patch = serde_json::json!({
            "status": session_status.as_str(),
            "finishedAt": chrono::Local::now().to_rfc3339(),
        });
        if let Some(code) = exit_code {
            patch["exitCode"] = code.into();
        }
        if let Some(snap) = snapshot {
            patch["contextSnapshot"] = serde_json::to_value(snap).unwrap();
        }
        if let Some(summary) = output_summary {
            patch["outputSummary"] = serde_json::Value::String(summary);
        }

        log::info!("[completion] Session {} locking DB for completion update...", sid_exit);
        if let Ok(conn) = db_conn_exit.get() {
            log::info!("[completion] Session {} DB locked, writing completion...", sid_exit);
            let _ = crate::agents::session::update_session_db(&conn, &sid_exit, patch);
            let event_type = match session_status {
                SessionStatus::Completed => "session_completed",
                _ => "session_failed",
            };
            let _ = crate::activity::record_event(&conn, &crate::activity::make_activity_event(
                &sid_exit,
                &project_path_exit,
                &agent_type_exit,
                event_type,
                &format!("{} session {}", agent_type_exit.display_name(), session_status.as_str()),
                None,
                files_for_activity,
            ));
        }
        let _ = app_exit.emit(
            "agent:completed",
            serde_json::json!({
                "sessionId": sid_exit,
                "status": session_status.as_str(),
                "exitCode": exit_code,
            }),
        );

        run_post_session_hooks(
            db_conn_exit.clone(),
            project_path_exit.clone(),
            sid_exit.to_string(),
            agent_type_exit.clone(),
            session_status,
        );
    });

    Ok(session)
}

// ---------------------------------------------------------------------------
// Pipe fallback path
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Session lifecycle helpers (shared by pipe spawn + kernel react-chat driver)
// Extracted verbatim from spawn_pipe_fallback so the ReactAgent chat path can
// reuse the exact same conversation resolution + completion bookkeeping. The
// pipe path now calls these — behavior is byte-identical to the prior inline
// code; only the structure changed.
// ---------------------------------------------------------------------------

/// Resolve the conversation this turn belongs to. `None` ⇒ first turn of a
/// brand-new conversation (create it, title = prompt head). `Some` ⇒ attach
/// (the row already exists; the caller touches last_activity on register).
pub(crate) fn resolve_or_create_conversation(
    db_conn: &crate::db::DbState,
    conversation_id: Option<&str>,
    project_path: &str,
    prompt: &str,
    agent_type: &AgentType,
) -> Result<String, String> {
    let conn = db_conn.get().map_err(|e| e.to_string())?;
    let resolved_conv_id: String = match conversation_id {
        Some(id) => id.to_string(),
        None => {
            let new_id = uuid::Uuid::new_v4().to_string();
            let now = chrono::Local::now().to_rfc3339();
            let title: String = prompt.chars().take(40).collect();
            let conv = crate::models::Conversation {
                id: new_id.clone(),
                project_path: project_path.to_string(),
                title,
                last_agent: Some(agent_type.clone()),
                status: "active".to_string(),
                started_at: now.clone(),
                last_activity_at: now,
                pinned: false,
            };
            crate::agents::session::insert_conversation_db(&conn, &conv)
                .map_err(|e| e.to_string())?;
            new_id
        }
    };
    Ok(resolved_conv_id)
}

/// Build a `Running` Session row ready for `insert_session_db`. Does not write.
pub(crate) fn build_running_session_row(
    session_id: &str,
    project_path: &str,
    agent_type: &AgentType,
    prompt: &str,
    model: Option<&str>,
    conversation_id: &str,
    linked_requirement_id: Option<&str>,
    parent_session_id: Option<&str>,
) -> Session {
    Session {
        id: session_id.to_string(),
        project_path: project_path.to_string(),
        agent_type: agent_type.clone(),
        status: SessionStatus::Running,
        prompt: prompt.to_string(),
        model: model.map(|m| m.to_string()),
        started_at: chrono::Local::now().to_rfc3339(),
        finished_at: None,
        exit_code: None,
        output_summary: None,
        context_snapshot: None,
        linked_requirement_id: linked_requirement_id.map(|s| s.to_string()),
        parent_session_id: parent_session_id.map(|s| s.to_string()),
        conversation_id: Some(conversation_id.to_string()),
    }
}

/// Insert the session row, touch the conversation's last_activity (when
/// attaching), record a `session_started` activity event, and emit
/// `agent:started`. This is the synchronous setup half of spawn — it must
/// complete before the caller hands the session back to the UI.
pub(crate) fn register_running_session(
    db_conn: &crate::db::DbState,
    app: &tauri::AppHandle,
    session: &Session,
    conversation_id: Option<&str>,
    resolved_conv_id: &str,
    project_path: &str,
    agent_type: &AgentType,
) -> Result<(), String> {
    let conn = db_conn.get().map_err(|e| e.to_string())?;
    crate::agents::session::insert_session_db(&conn, session)
        .map_err(|e| e.to_string())?;

    if conversation_id.is_some() {
        let now = chrono::Local::now().to_rfc3339();
        let patch = serde_json::json!({
            "lastAgent": serde_json::to_string(agent_type).unwrap_or_default().trim_matches('"'),
            "lastActivityAt": now,
        });
        let _ = crate::agents::session::update_conversation_db(&conn, resolved_conv_id, patch);
    }

    let _ = crate::activity::record_event(&conn, &crate::activity::make_activity_event(
        &session.id,
        project_path,
        agent_type,
        "session_started",
        &format!("{} session started", agent_type.display_name()),
        None,
        None,
    ));
    let _ = app.emit("agent:started", session);
    Ok(())
}

/// Write the final session state: status/finishedAt/exit/context/summary patch,
/// a `session_completed`/`session_failed` activity event (carrying the changed
/// file list), and emit `agent:completed`. Then kick off the post-session hooks
/// (knowledge collection, quality gate) on a background thread. The caller
/// prepares `output_summary` + `context_snapshot`; this fn only persists the
/// terminal state — so the ReactAgent driver can call it with the same shape the
/// pipe wait-thread does.
pub(crate) fn finalize_session(
    db_conn: &crate::db::DbState,
    app: &tauri::AppHandle,
    session_id: &str,
    project_path: &str,
    agent_type: &AgentType,
    session_status: SessionStatus,
    exit_code: Option<i32>,
    output_summary: Option<String>,
    context_snapshot: Option<ContextSnapshot>,
) {
    let files_for_activity = context_snapshot.as_ref().map(|s| s.files_changed.clone());

    let mut patch = serde_json::json!({
        "status": session_status.as_str(),
        "finishedAt": chrono::Local::now().to_rfc3339(),
    });
    if let Some(code) = exit_code {
        patch["exitCode"] = code.into();
    }
    if let Some(snap) = context_snapshot {
        patch["contextSnapshot"] = serde_json::to_value(snap).unwrap();
    }
    if let Some(summary) = output_summary {
        patch["outputSummary"] = serde_json::Value::String(summary);
    }

    log::info!("[completion] Session {} locking DB for completion update...", session_id);
    if let Ok(conn) = db_conn.get() {
        log::info!("[completion] Session {} DB locked, writing completion...", session_id);
        let _ = crate::agents::session::update_session_db(&conn, session_id, patch);
        let event_type = match session_status {
            SessionStatus::Completed => "session_completed",
            _ => "session_failed",
        };
        let _ = crate::activity::record_event(&conn, &crate::activity::make_activity_event(
            session_id,
            project_path,
            agent_type,
            event_type,
            &format!("{} session {}", agent_type.display_name(), session_status.as_str()),
            None,
            files_for_activity,
        ));
    } else {
        log::error!("[finalize] Failed to lock DB for session {} completion update", session_id);
    }
    log::info!("[finalize] Emitting agent:completed for session {}", session_id);
    let _ = app.emit(
        "agent:completed",
        serde_json::json!({
            "sessionId": session_id,
            "status": session_status.as_str(),
            "exitCode": exit_code,
        }),
    );

    run_post_session_hooks(
        db_conn.clone(),
        project_path.to_string(),
        session_id.to_string(),
        agent_type.clone(),
        session_status,
    );
}

fn spawn_pipe_fallback(
    app: &tauri::AppHandle,
    processes: Arc<AgentProcesses>,
    db_conn: crate::db::DbState,
    config: &SpawnConfig,
    agent_type: &AgentType,
    project_path: &str,
    linked_requirement_id: Option<&str>,
    parent_session_id: Option<&str>,
    conversation_id: Option<&str>,
    prompt: &str,
    model: Option<&str>,
) -> Result<Session, String> {
    log::info!("[PIPE spawn] program={}, args={:?}, cwd={}, stdin_prompt={}", config.program.display(), config.args, config.cwd.display(), config.stdin_prompt.is_some());
    let mut cmd = std::process::Command::new(&config.program);
    let use_stdin = config.stdin_prompt.is_some();
    cmd.args(&config.args)
        .current_dir(&config.cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(if use_stdin { std::process::Stdio::piped() } else { std::process::Stdio::null() });

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动 {} 失败: {}", agent_type.display_name(), e))?;
    let pid = child.id();

    // Write prompt to stdin if using stdin delivery
    if let Some(ref prompt_text) = config.stdin_prompt {
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(e) = std::io::Write::write_all(&mut stdin, prompt_text.as_bytes()) {
                log::warn!("[PIPE spawn] Failed to write prompt to stdin: {}", e);
            }
            drop(stdin); // close stdin to signal EOF
        }
    } else {
        drop(child.stdin.take());
    }

    let session_id = uuid::Uuid::new_v4().to_string();

    processes
        .processes
        .lock().unwrap_or_else(|e| e.into_inner())
        .insert(session_id.clone(), TrackedProcess::Pipe(pid));

    // Capture pre-diff in background — agent won't modify files in the first
    // few hundred milliseconds, so racing this thread against the agent is safe.
    {
        let bg_sid = session_id.clone();
        let bg_pp = project_path.to_string();
        std::thread::Builder::new()
            .name("pre-diff-capture".into())
            .spawn(move || {
                let pre_diff = capture_git_diff_names(&bg_pp);
                if let Ok(agents_dir) = crate::agents::session::agents_dir() {
                    let dir = agents_dir.join("outputs");
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(dir.join(format!("{}.pre-diff", bg_sid)), pre_diff.join("\n"));
                }
            })
            .ok();
    }

    let resolved_conv_id = resolve_or_create_conversation(
        &db_conn, conversation_id, project_path, prompt, agent_type,
    )?;
    let session = build_running_session_row(
        &session_id, project_path, agent_type, prompt, model,
        &resolved_conv_id, linked_requirement_id, parent_session_id,
    );
    register_running_session(
        &db_conn, app, &session, conversation_id, &resolved_conv_id, project_path, agent_type,
    )?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // stdin already taken/handled above

    // Reader thread
    let app_reader = app.clone();
    let sid_reader = session_id.clone();
    let processes_reader = processes.clone();
    // Shared idle tracker: reader stamps it per output chunk; the wait thread
    // kills the process if it stays quiet past the idle timeout. Replaces the
    // fixed 600s wall-clock kill that chopped down healthy streaming long tasks.
    let last_activity = Arc::new(AtomicU64::new(now_millis()));

    let output_mode = config.output_mode; // Copy — drives stdout interpretation
    let last_activity_reader = last_activity.clone();
    std::thread::spawn(move || {
        let mut output_log: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];

        if let Some(mut out) = stdout {
            match output_mode {
                OutputMode::Raw => {
                    loop {
                        match out.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                let data = &buf[..n];
                                output_log.extend_from_slice(data);
                                last_activity_reader.store(now_millis(), Ordering::Relaxed);
                                let _ = app_reader.emit(
                                    "pty:output",
                                    serde_json::json!({
                                        "sessionId": sid_reader,
                                        "data": data.to_vec(),
                                    }),
                                );
                            }
                            Err(_) => break,
                        }
                    }
                }
                OutputMode::ClaudeStreamJson => {
                    // Each stdout line is one claude event. Parse it ONCE into
                    // structured blocks, then fan out TWO channels from the same
                    // parse (no double-parse):
                    //   - agent:event : structured ChatStreamEvent per block, for
                    //     the chat block-card UI (text / tool_use / tool_result /
                    //     result). Raw JSON never reaches the user.
                    //   - pty:output   : the rendered ANSI text, for the terminal
                    //     fallback view AND the {sid}.log replay file.
                    use std::io::BufRead;
                    let reader = std::io::BufReader::new(out);
                    for line in reader.lines() {
                        let line = match line {
                            Ok(l) => l,
                            Err(_) => break,
                        };
                        let blocks = parse_claude_line(&line);
                        if blocks.is_empty() {
                            continue; // system / api_retry noise — neither channel cares
                        }
                        // claude produced output → it's alive, reset the idle timer.
                        last_activity_reader.store(now_millis(), Ordering::Relaxed);
                        // Structured channel first (the chat UI consumes only this).
                        for event in blocks.iter().map(ClaudeBlock::to_event) {
                            let _ = app_reader.emit(
                                "agent:event",
                                serde_json::json!({
                                    "sessionId": sid_reader,
                                    "event": event,
                                }),
                            );
                        }
                        // Rendered text channel (terminal fallback + log replay).
                        if let Some(text) = render_blocks(&blocks) {
                            output_log.extend_from_slice(text.as_bytes());
                            let _ = app_reader.emit(
                                "pty:output",
                                serde_json::json!({
                                    "sessionId": sid_reader,
                                    "data": text.into_bytes(),
                                }),
                            );
                        }
                    }
                }
            }
        }
        if let Some(mut err) = stderr {
            loop {
                match err.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = &buf[..n];
                        output_log.extend_from_slice(data);
                        last_activity_reader.store(now_millis(), Ordering::Relaxed);
                        let _ = app_reader.emit(
                            "pty:output",
                            serde_json::json!({
                                "sessionId": sid_reader,
                                "data": data.to_vec(),
                            }),
                        );
                    }
                    Err(_) => break,
                }
            }
        }

        if let Ok(agents_dir) = crate::agents::session::agents_dir() {
            let log_path = agents_dir.join("outputs");
            let _ = std::fs::create_dir_all(&log_path);
            let _ = std::fs::write(log_path.join(format!("{}.log", sid_reader)), &output_log);
        }

        if let Ok(mut map) = processes_reader.processes.lock() {
            map.remove(&sid_reader);
        }
    });

    // Wait thread — kills the process only if it goes IDLE (no output) past the
    // idle timeout. Unlike a fixed wall-clock kill, a task that keeps streaming
    // (healthy long runs) never trips this; only a truly hung process does.
    let app_exit = app.clone();
    let sid_exit = session_id.clone();
    let project_path_exit = project_path.to_string();
    let db_conn_exit = db_conn.clone();
    let agent_type_exit = agent_type.clone();
    let processes_kill = processes.clone();
    let last_activity_wait = last_activity.clone();
    std::thread::spawn(move || {
        let idle_secs = session_idle_timeout_secs();
        log::info!("[PIPE wait] Waiting for session {} (idle_timeout={}s, 0=disabled)", sid_exit, idle_secs);
        let mut timed_out = false;
        let (exit_code, session_status) = loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let code = status.code();
                    let st = if status.success() {
                        SessionStatus::Completed
                    } else {
                        SessionStatus::Failed
                    };
                    break (code, st);
                }
                Ok(None) => {
                    if idle_secs > 0 {
                        let idle_ms = now_millis()
                            .saturating_sub(last_activity_wait.load(Ordering::Relaxed));
                        if idle_ms > idle_secs * 1000 {
                            log::warn!(
                                "[PIPE wait] Session {} idle {}ms (>{}s, no output) — killing",
                                sid_exit, idle_ms, idle_secs,
                            );
                            timed_out = true;
                            // Force-kill the process tree
                            let _ = stop_agent(&processes_kill, &sid_exit);
                            break (None, SessionStatus::Failed);
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                Err(_) => {
                    break (None, SessionStatus::Failed);
                }
            }
        };

        log::info!("[PIPE wait] Session {} exited: code={:?}, status={}, timed_out={}", sid_exit, exit_code, session_status.as_str(), timed_out);

        std::thread::sleep(std::time::Duration::from_millis(500));

        // Read output summary FIRST so key_output can be derived from it.
        let output_summary = read_output_summary(&sid_exit).or_else(|| {
            if timed_out {
                Some(format!("(Session killed: no output for {}s — likely hung)", idle_secs))
            } else if exit_code.unwrap_or(-1) == 0 {
                Some("(Agent completed with no text output)".to_string())
            } else {
                Some(format!("(Process exited with code {:?})", exit_code))
            }
        });
        log::info!("[completion] Session {} capturing context snapshot...", sid_exit);
        let snapshot = extract_context_snapshot(&project_path_exit, &sid_exit, output_summary.as_deref());
        log::info!(
            "[completion] Session {} snapshot done ({} files changed, {} key_output chars)",
            sid_exit,
            snapshot.as_ref().map(|s| s.files_changed.len()).unwrap_or(0),
            snapshot.as_ref().map(|s| s.key_output.chars().count()).unwrap_or(0)
        );
        finalize_session(
            &db_conn_exit,
            &app_exit,
            &sid_exit,
            &project_path_exit,
            &agent_type_exit,
            session_status,
            exit_code,
            output_summary,
            snapshot,
        );
    });

    Ok(session)
}

// ---------------------------------------------------------------------------
// Interactive I/O (real for PTY, no-op for pipe)
// ---------------------------------------------------------------------------

/// Write data to the agent's stdin. Pipe sessions close stdin right after
/// delivering the prompt, so interactive writes are not supported (no-op).
pub fn pty_write(
    _processes: &Arc<AgentProcesses>,
    _session_id: &str,
    _data: &str,
) -> Result<(), String> {
    Ok(())
}

/// Resize the terminal. Pipe sessions have no PTY to resize (no-op).
pub fn pty_resize(
    _processes: &Arc<AgentProcesses>,
    _session_id: &str,
    _cols: u16,
    _rows: u16,
) -> Result<(), String> {
    Ok(())
}

/// Stop a running agent session.
pub fn stop_agent(processes: &Arc<AgentProcesses>, session_id: &str) -> Result<(), String> {
    let tracked = {
        let mut map = processes.processes.lock().map_err(|e| format!("进程表锁失败: {}", e))?;
        map.remove(session_id)
    };

    match tracked {
        Some(TrackedProcess::Pipe(pid)) => {
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                    .output();
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = std::process::Command::new("kill")
                    .args(["-TERM", &pid.to_string()])
                    .output();
            }
        }
        None => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read the FULL output log for a session (ANSI stripped, not truncated).
/// Used by the completed-session terminal view so users see the entire agent output
/// instead of the tail-truncated `outputSummary` (which only keeps the end for list previews).
pub(crate) fn read_full_session_output(session_id: &str) -> Option<String> {
    let agents_dir = crate::agents::session::agents_dir().ok()?;
    let log_path = agents_dir.join("outputs").join(format!("{}.log", session_id));
    if !log_path.exists() {
        return None;
    }
    let bytes = std::fs::read(&log_path).ok()?;
    let text = strip_ansi(&bytes);
    if text.trim().is_empty() {
        return None;
    }
    Some(text)
}

/// Read the output log for a session and return a truncated summary (list preview only).
/// The completed-session view uses [`read_full_session_output`] for the complete text.
fn read_output_summary(session_id: &str) -> Option<String> {
    let text = read_full_session_output(session_id)?;
    Some(truncate_tail(&text, OUTPUT_SUMMARY_MAX_CHARS))
}

/// Keep only the tail of `text` (up to `max` bytes), prefixed with `...`.
/// The slice start is snapped to a UTF-8 char boundary so it never panics when
/// `max` lands inside a multibyte (e.g. CJK) character. List-preview only — the
/// completed-session view reads the full, untruncated text via [`read_full_session_output`].
fn truncate_tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    format!("...{}", &text[start..])
}

/// Strip ANSI escape sequences from raw bytes, return clean UTF-8 string.
fn strip_ansi(bytes: &[u8]) -> String {
    crate::utils::strip_ansi(&String::from_utf8_lossy(bytes))
}

/// Build a context snapshot: files changed (git diff) + key output (the most
/// relevant tail of the agent's output, stripped of ANSI noise). The key_output
/// is a compact summary -- previously always empty, which made the column
/// near-useless for resume/search.
fn extract_context_snapshot(
    project_path: &str,
    session_id: &str,
    output_summary: Option<&str>,
) -> Option<ContextSnapshot> {
    let post_diff = capture_git_diff_numstat(project_path);
    let pre_diff = read_pre_diff(session_id).unwrap_or_default();
    // Keep only files this session touched (not already-dirty before it ran).
    let file_diffs: Vec<FileDiff> = post_diff
        .into_iter()
        .filter(|d| !pre_diff.contains(&d.path))
        .collect();
    let files_changed: Vec<String> = file_diffs.iter().map(|d| d.path.clone()).collect();

    let key_output = output_summary
        .map(|s| {
            let stripped = strip_ansi_basic(s);
            let compact: String = stripped
                .lines()
                .map(|l| l.trim_end())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            compact.chars().take(500).collect()
        })
        .unwrap_or_default();

    Some(ContextSnapshot {
        files_changed,
        key_output,
        file_diffs,
    })
}

/// Minimal ANSI escape stripper: removes common CSI sequences for clean key_output.
fn strip_ansi_basic(s: &str) -> String {
    crate::utils::strip_ansi(s)
}

/// Max time to wait for git diff --name-only before giving up.
const GIT_DIFF_TIMEOUT_SECS: u64 = 15;

/// Path-only projection of the working-tree diff — used to capture the
/// pre-session "already dirty" baseline (`.pre-diff`) so extract_context_snapshot
/// can attribute only the files this session actually touched. Thin wrapper
/// over capture_git_diff_numstat; kept because pre-diff capture only needs paths.
fn capture_git_diff_names(project_path: &str) -> Vec<String> {
    capture_git_diff_numstat(project_path)
        .into_iter()
        .map(|d| d.path)
        .collect()
}

fn capture_git_diff_numstat(project_path: &str) -> Vec<FileDiff> {
    let pp = project_path.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("git-diff-numstat".into())
        .spawn(move || {
            let mut cmd = std::process::Command::new("git");
            cmd.args(["diff", "--numstat"]).current_dir(&pp);
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            let result = cmd
                .output()
                .ok()
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .filter_map(parse_numstat_line)
                        .collect::<Vec<FileDiff>>()
                })
                .unwrap_or_default();
            let _ = tx.send(result);
        })
        .ok();

    match rx.recv_timeout(std::time::Duration::from_secs(GIT_DIFF_TIMEOUT_SECS)) {
        Ok(v) => v,
        Err(_) => {
            log::warn!(
                "[git diff numstat] timed out after {}s for {} — skipping context snapshot",
                GIT_DIFF_TIMEOUT_SECS,
                project_path
            );
            Vec::new()
        }
    }
}

/// Parse one `git diff --numstat` line: `<added>\t<removed>\t<path>`.
/// added/removed are `-` for binary files — coerce parse failures to 0.
/// The path itself may contain tabs (renames with spaces), so rejoin the tail.
fn parse_numstat_line(line: &str) -> Option<FileDiff> {
    let mut parts = line.splitn(3, '\t');
    let added_raw = parts.next()?;
    let removed_raw = parts.next()?;
    let path = parts.next()?.trim();
    if path.is_empty() {
        return None;
    }
    let added = added_raw.trim().parse::<i64>().unwrap_or(0);
    let removed = removed_raw.trim().parse::<i64>().unwrap_or(0);
    Some(FileDiff {
        path: path.to_string(),
        added,
        removed,
    })
}

fn read_pre_diff(session_id: &str) -> Option<Vec<String>> {
    let path = crate::agents::session::agents_dir().ok()?
        .join("outputs")
        .join(format!("{}.pre-diff", session_id));
    let content = std::fs::read_to_string(&path).ok()?;
    Some(
        content
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Knowledge injection with timeout (avoids blocking UI on slow DB queries)
// ---------------------------------------------------------------------------

/// Inject knowledge into the prompt in a background thread with a 2s timeout.
/// Falls back to the original prompt if injection takes too long.
fn inject_knowledge_with_timeout(
    db_conn: &crate::db::DbState,
    agent_type: &AgentType,
    project_path: &str,
    prompt: &str,
) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    let conn = db_conn.clone();
    let at = agent_type.clone();
    let pp = project_path.to_string();
    let p = prompt.to_string();

    std::thread::Builder::new()
        .name("knowledge-inject".into())
        .spawn(move || {
            let result = match conn.get() {
                Ok(conn) => crate::knowledge::injector::inject_for_agent(&conn, &at, &pp, &p),
                Err(_) => p,
            };
            let _ = tx.send(result);
        })
        .ok();

    match rx.recv_timeout(std::time::Duration::from_secs(2)) {
        Ok(injected) => injected,
        Err(_) => {
            log::warn!("Knowledge injection timed out for project {}, using original prompt", project_path);
            prompt.to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// File reference injection (@path → actual file content)
// ---------------------------------------------------------------------------

/// Maximum bytes per file to inject.
const FILE_INJECT_MAX_BYTES: usize = 50 * 1024;
/// Maximum total bytes across all file injections.
const FILE_INJECT_TOTAL_MAX_BYTES: usize = 200 * 1024;

/// Parse `@/path/to/file`, `@X:\path`, or `@filename.ext` references from the
/// prompt and replace them with the actual file content wrapped in markers.
fn inject_file_references(project_path: &str, prompt: &str) -> String {
    let mut result = prompt.to_string();
    let mut total_injected: usize = 0;
    let mut replacements: Vec<(String, String)> = Vec::new();

    // Manual scan for @-prefixed file paths
    let chars: Vec<char> = prompt.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '@' {
            i += 1;
            continue;
        }

        // @ must be preceded by whitespace or start-of-string (avoid matching emails)
        if i > 0 && !chars[i - 1].is_whitespace() && chars[i - 1] != '\n' {
            i += 1;
            continue;
        }

        // Collect candidate path chars until whitespace or end
        let mut path_end = i + 1;
        while path_end < chars.len()
            && !chars[path_end].is_whitespace()
            && chars[path_end] != '@'
        {
            path_end += 1;
        }

        if path_end <= i + 1 {
            i += 1;
            continue;
        }

        let candidate: String = chars[i + 1..path_end].iter().collect();

        // Check if candidate looks like a file path:
        // - Starts with / or \ (absolute Unix-style)
        // - Starts with X:\ (Windows absolute)
        // - Contains a '.' (likely a filename like Cargo.toml, src/main.rs)
        // - Contains '/' or '\' (path separator)
        let looks_like_path = candidate.starts_with('/')
            || candidate.starts_with('\\')
            || (candidate.len() >= 3
                && candidate.as_bytes()[0].is_ascii_alphabetic()
                && candidate.as_bytes()[1] == b':'
                && (candidate.as_bytes()[2] == b'\\' || candidate.as_bytes()[2] == b'/'))
            || candidate.contains('.')
            || candidate.contains('/')
            || candidate.contains('\\');

        if !looks_like_path {
            i = path_end;
            continue;
        }

        let full_match: String = chars[i..path_end].iter().collect();

        // Skip if already processed
        if replacements.iter().any(|(m, _)| m == &full_match) {
            i = path_end;
            continue;
        }

        let path = if std::path::Path::new(&candidate).is_absolute() {
            std::path::PathBuf::from(&candidate)
        } else {
            // Relative path like Cargo.toml or /src/main.rs → project_path/...
            let relative = candidate.trim_start_matches('/').trim_start_matches('\\');
            std::path::PathBuf::from(project_path).join(relative)
        };

        // Read file content (with size limit)
        let content = match std::fs::read(&path) {
            Ok(bytes) => {
                if bytes.len() > FILE_INJECT_MAX_BYTES {
                    format!(
                        "[File too large: {} bytes, showing first {} bytes]\n{}",
                        bytes.len(),
                        FILE_INJECT_MAX_BYTES,
                        String::from_utf8_lossy(&bytes[..FILE_INJECT_MAX_BYTES])
                    )
                } else {
                    String::from_utf8_lossy(&bytes).to_string()
                }
            }
            Err(e) => format!("[Could not read file {}: {}]", path.display(), e),
        };

        let injected_len = content.len();
        if total_injected + injected_len > FILE_INJECT_TOTAL_MAX_BYTES {
            replacements.push((
                full_match.to_string(),
                format!("[File {} skipped: total injection limit reached]", path.display()),
            ));
            continue;
        }
        total_injected += injected_len;

        let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let wrapped = format!(
            "--- BEGIN FILE: {} ({}) ---\n{}\n--- END FILE: {} ---",
            file_name, path.display(), content, file_name
        );
        replacements.push((full_match.to_string(), wrapped));
    }

    for (pattern, replacement) in replacements {
        result = result.replace(&pattern, &replacement);
    }

    result
}

// ---------------------------------------------------------------------------
// Post-session hooks (shared by PTY and Pipe paths)
// ---------------------------------------------------------------------------

/// Run knowledge collection and quality gate in a background thread.
/// Extracted to avoid code duplication between PTY and Pipe wait threads.
fn run_post_session_hooks(
    db: crate::db::DbState,
    project_path: String,
    session_id: String,
    agent_type: AgentType,
    session_status: SessionStatus,
) {
    let sid_for_log = session_id.clone();
    let result = std::thread::Builder::new()
        .name("post-session-hooks".into())
        .spawn(move || {
            // 1. Knowledge collection (only on success)
            if session_status == SessionStatus::Completed {
                if let Ok(conn) = db.get() {
                    let _ = crate::knowledge::collector::collect_from_session(
                        &conn, &project_path, &session_id, &agent_type,
                    );
                }
            }
            // 2. Quality gate — run subprocess
            let forge_result = crate::quality::forge::run_forge_gate(std::path::Path::new(&project_path));
            match forge_result {
                Ok(report) => {
                    if let Ok(conn) = db.get() {
                        let _ = crate::quality::report::save_report(&conn, &report);
                        let _ = crate::quality::feedback::create_feedback(
                            &conn, &report, &project_path, &agent_type,
                        );
                    }
                }
                Err(crate::error::AppError::ForgeNotInstalled) => { /* graceful skip */ }
                Err(e) => log::warn!("Quality gate failed: {}", e),
            }
        });

    if let Err(e) = result {
        log::error!(
            "Failed to spawn post-session-hooks thread for session {}: {}",
            sid_for_log, e
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `capture_git_diff_names` must return tracked files modified in the
    /// working tree. Guards the timeout wrapper added to stop the completion
    /// event from stalling on slow `git diff` in large/dirty repos.
    #[test]
    fn capture_git_diff_names_lists_modified_tracked_files() {
        // Skip when git isn't on PATH (some CI / sandboxed envs).
        let git_ok = std::process::Command::new("git")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok();
        if !git_ok {
            eprintln!("git unavailable — skipping capture_git_diff_names test");
            return;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();

        let run_git = |args: &[&str]| {
            let mut c = std::process::Command::new("git");
            c.args(args)
                .current_dir(path)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "test@example.com");
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            c.output().expect("git command succeeds")
        };

        run_git(&["init"]);
        std::fs::write(path.join("base.txt"), "base").unwrap();
        run_git(&["add", "."]);
        run_git(&["commit", "-m", "init"]);

        // Modify the tracked file → creates an unstaged diff entry.
        std::fs::write(path.join("base.txt"), "changed").unwrap();

        let changed = capture_git_diff_names(&path.to_string_lossy());
        assert!(
            changed.iter().any(|f| f.ends_with("base.txt")),
            "expected base.txt in diff, got {:?}",
            changed
        );
    }

    #[test]
    fn parse_numstat_line_parses_added_removed_path() {
        let d = parse_numstat_line("12\t3\tsrc/main.rs").unwrap();
        assert_eq!(d.path, "src/main.rs");
        assert_eq!(d.added, 12);
        assert_eq!(d.removed, 3);
    }

    #[test]
    fn parse_numstat_line_coerces_binary_dash_to_zero() {
        // Binary files report `-` for both counts — must not panic, must be 0.
        let d = parse_numstat_line("-\t-\timage.png").unwrap();
        assert_eq!(d.path, "image.png");
        assert_eq!(d.added, 0);
        assert_eq!(d.removed, 0);
    }

    #[test]
    fn parse_numstat_line_rejects_malformed() {
        assert!(parse_numstat_line("").is_none());
        assert!(parse_numstat_line("only-one-field").is_none());
        assert!(parse_numstat_line("1\t2\t").is_none(), "empty path rejected");
    }

    #[test]
    fn truncate_tail_keeps_short_text_unchanged() {
        assert_eq!(truncate_tail("hello", 2000), "hello");
        assert_eq!(truncate_tail("短文本", 2000), "短文本");
    }

    #[test]
    fn truncate_tail_keeps_tail_and_snaps_to_char_boundary() {
        // 6 CJK chars = 18 bytes; truncate to 4 bytes. Before the char-boundary
        // fix this panicked by slicing mid-character. The tail must be a valid
        // UTF-8 suffix of the original text.
        let text = "一二三四五六";
        let out = truncate_tail(text, 4);
        assert!(out.starts_with("..."), "truncated preview must be prefixed with ...; got {out:?}");
        let tail = out.strip_prefix("...").unwrap_or(&out);
        assert!(text.ends_with(tail), "tail must be a suffix of the original; got {out:?}");
    }

    #[test]
    fn strip_ansi_removes_escape_sequences() {
        let raw = b"\x1b[1mbold\x1b[0m and \x1b[90mgray\x1b[0m";
        assert_eq!(strip_ansi(raw), "bold and gray");
    }

    // ---- Conversation context bridge ----

    fn mk_turn(agent: AgentType, prompt: &str, output: Option<&str>) -> crate::models::Session {
        crate::models::Session {
            id: uuid::Uuid::new_v4().to_string(),
            project_path: "/p".to_string(),
            agent_type: agent,
            status: crate::models::SessionStatus::Completed,
            prompt: prompt.to_string(),
            model: None,
            started_at: "2026-01-01T00:00:00Z".to_string(),
            finished_at: None,
            exit_code: Some(0),
            output_summary: output.map(|s| s.to_string()),
            context_snapshot: None,
            linked_requirement_id: None,
            parent_session_id: None,
            conversation_id: Some("c1".to_string()),
        }
    }

    // ---- Spawn config (no bare --continue) ----

    #[test]
    fn claude_spawn_never_injects_bare_continue() {
        // Regression: claude's bare `--continue`/`-c` resumes "the most recent
        // conversation in [cwd]" — a claude-internal notion of "recent" that does
        // NOT correspond to this DevWorkbench conversation. With users switching
        // across conversations/projects, `--continue` resumed an unrelated session
        // and the agent answered off-topic (the "答非所问" symptom). We removed
        // --continue entirely; continuity now comes from inject_conversation_context.
        // This test locks the removal so it isn't reintroduced.
        //
        // Pre-fill the EXE cache so build_spawn_config skips the PATH scan — keeps
        // the test logic-only (no real claude install required on the runner).
        {
            let mut cache = EXE_CACHE.lock().unwrap();
            cache.insert("claude".to_string(), std::path::PathBuf::from("claude"));
        }
        let cfg = build_spawn_config(&AgentType::ClaudeCode, "/p", "hello", None)
            .expect("build_spawn_config for ClaudeCode");
        assert!(
            cfg.args.contains(&"--print".to_string()),
            "ClaudeCode must use --print: {:?}",
            cfg.args,
        );
        assert!(
            !cfg.args.iter().any(|a| a == "--continue" || a == "-c"),
            "ClaudeCode must NOT inject bare --continue/-c (causes cwd-recent session bleed): {:?}",
            cfg.args,
        );
    }

    #[test]
    fn inject_context_first_turn_is_unchanged() {
        // No prior turns ⇒ the prompt must come back verbatim (no preamble).
        let out = inject_conversation_context("do the thing", &[], &AgentType::ClaudeCode);
        assert_eq!(out, "do the thing");
    }

    #[test]
    fn inject_context_includes_prior_turn_prompt_and_output() {
        let prior = vec![mk_turn(
            AgentType::ClaudeCode,
            "add a login page",
            Some("created src/Login.tsx"),
        )];
        let out = inject_conversation_context("now add validation", &prior, &AgentType::Codex);
        assert!(out.contains("add a login page"), "prior user prompt injected: {out}");
        assert!(out.contains("created src/Login.tsx"), "prior output injected: {out}");
        assert!(out.contains("now add validation"), "current request appended: {out}");
        assert!(out.contains("Claude Code"), "prior agent named in bridge: {out}");
    }

    #[test]
    fn inject_context_caps_total_and_keeps_newest() {
        // 20 turns each producing a big block — total must stay under the cap,
        // and the MOST RECENT turn's content must survive the trim.
        let big = "x".repeat(CONTEXT_BRIDGE_OUTPUT_MAX_CHARS);
        let prior: Vec<_> = (0..20)
            .map(|i| mk_turn(AgentType::ClaudeCode, &format!("turn-{i}"), Some(&format!("{big} marker-{i}"))))
            .collect();
        let out = inject_conversation_context("current", &prior, &AgentType::ClaudeCode);
        assert!(
            out.len() < CONTEXT_BRIDGE_TOTAL_MAX_CHARS + 4096,
            "injected history must stay near the cap; got {} bytes",
            out.len()
        );
        // The newest turn's marker must be present; the oldest should have been dropped.
        assert!(out.contains("marker-19"), "newest turn preserved: {out}");
        assert!(!out.contains("marker-0"), "oldest turn dropped to fit cap");
    }

    #[test]
    fn tail_keeps_suffix_and_snaps_to_char_boundary() {
        assert_eq!(tail("hello", 10), "hello");
        // 6 CJK chars = 18 bytes; tail to 4 bytes snaps to a char boundary.
        let t = tail("一二三四五六", 4);
        assert!(t.starts_with("..."));
        assert!("一二三四五六".ends_with(t.trim_start_matches('.')));
    }

    #[test]
    fn claude_spawn_uses_stream_json_for_live_output() {
        // Regression for the "Agent 运行中，等待输出" UX problem: claude's
        // DEFAULT text mode buffers all output until process exit when stdout is
        // a non-TTY pipe. We force stream-json so the reader sees realtime
        // events. Lock the flags + the reader-side output_mode together.
        {
            let mut cache = EXE_CACHE.lock().unwrap();
            cache.insert("claude".to_string(), std::path::PathBuf::from("claude"));
        }
        let cfg = build_spawn_config(&AgentType::ClaudeCode, "/p", "hello", None)
            .expect("build_spawn_config for ClaudeCode");
        assert!(
            cfg.args.contains(&"--output-format".to_string())
                && cfg.args.contains(&"stream-json".to_string()),
            "ClaudeCode must emit stream-json for realtime output: {:?}",
            cfg.args,
        );
        assert!(
            cfg.args.contains(&"--verbose".to_string()),
            "stream-json requires --verbose: {:?}",
            cfg.args,
        );
        assert_eq!(
            cfg.output_mode,
            OutputMode::ClaudeStreamJson,
            "reader must parse claude stdout as stream-json events",
        );
        // Other agents stay on raw byte streaming.
        {
            let mut cache = EXE_CACHE.lock().unwrap();
            cache.insert("codex".to_string(), std::path::PathBuf::from("codex"));
        }
        let cfg_codex = build_spawn_config(&AgentType::Codex, "/p", "hello", None)
            .expect("build_spawn_config for Codex");
        assert_eq!(cfg_codex.output_mode, OutputMode::Raw);
    }

    #[test]
    fn parse_claude_line_assistant_text() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello world"}]}}"#;
        assert_eq!(
            parse_claude_line(line),
            vec![ClaudeBlock::Text { content: "hello world".to_string() }],
        );
    }

    #[test]
    fn parse_claude_line_assistant_text_and_tool_use() {
        // One assistant line can carry text + a tool call — both survive as
        // ordered blocks (order matters for the terminal render).
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"reading"},{"type":"tool_use","name":"Read","input":{"file_path":"src/main.rs"}}]}}"#;
        assert_eq!(
            parse_claude_line(line),
            vec![
                ClaudeBlock::Text { content: "reading".to_string() },
                ClaudeBlock::ToolUse {
                    name: "Read".to_string(),
                    input: Some(serde_json::json!({"file_path":"src/main.rs"})),
                },
            ],
        );
    }

    #[test]
    fn parse_claude_line_multiple_tool_use() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"a"}},{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#;
        let blocks = parse_claude_line(line);
        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0], ClaudeBlock::ToolUse { ref name, .. } if name == "Read"));
        assert!(matches!(blocks[1], ClaudeBlock::ToolUse { ref name, .. } if name == "Bash"));
    }

    #[test]
    fn parse_claude_line_tool_use_without_input_field() {
        // claude occasionally omits `input`; render must still match the legacy
        // empty-preview output (not "null"), so input is parsed as Option.
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"WebSearch"}]}}"#;
        assert_eq!(
            parse_claude_line(line),
            vec![ClaudeBlock::ToolUse {
                name: "WebSearch".to_string(),
                input: None,
            }],
        );
    }

    #[test]
    fn parse_claude_line_tool_result() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"42 lines","is_error":false}]}}"#;
        let blocks = parse_claude_line(line);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ClaudeBlock::ToolResult { tool_use_id, is_error, .. } => {
                assert_eq!(tool_use_id.as_deref(), Some("t1"));
                assert_eq!(*is_error, Some(false));
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn parse_claude_line_result_success_and_error() {
        let ok = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":45000}"#;
        assert_eq!(
            parse_claude_line(ok),
            vec![ClaudeBlock::Result { is_error: false, secs: 45 }],
        );
        let err = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"duration_ms":1000}"#;
        assert_eq!(
            parse_claude_line(err),
            vec![ClaudeBlock::Result { is_error: true, secs: 1 }],
        );
        // subtype != success also counts as failure even without is_error.
        let bad = r#"{"type":"result","subtype":"error_during_execution","is_error":false,"duration_ms":500}"#;
        assert_eq!(
            parse_claude_line(bad),
            vec![ClaudeBlock::Result { is_error: true, secs: 0 }],
        );
    }

    #[test]
    fn parse_claude_line_system_and_malformed_are_empty() {
        // system (init / api_retry) carries no user-facing content.
        assert!(parse_claude_line(r#"{"type":"system","subtype":"init","session_id":"x"}"#).is_empty());
        // Malformed / non-JSON / empty → empty (must not panic, never break stream).
        assert!(parse_claude_line("not json at all").is_empty());
        assert!(parse_claude_line("").is_empty());
    }

    #[test]
    fn render_blocks_is_byte_identical_to_legacy_output() {
        // Golden snapshots: render_blocks(parse(line)) must equal the exact
        // ANSI text the old single-pass renderer produced. Locks the render
        // contract so the terminal replay and {sid}.log stay byte-identical.
        let assistant = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello world"}]}}"#;
        assert_eq!(
            render_blocks(&parse_claude_line(assistant)),
            Some("hello world\n".to_string()),
        );
        let tool = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"src/main.rs"}}]}}"#;
        let rendered = render_blocks(&parse_claude_line(tool)).expect("tool_use renders");
        assert_eq!(rendered, "\x1b[36m🔧 Read \x1b[90m{\"file_path\":\"src/main.rs\"}\x1b[0m\n");
        let ok = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":45000}"#;
        assert_eq!(
            render_blocks(&parse_claude_line(ok)),
            Some("\x1b[32m✓ 完成 (45s)\x1b[0m\n".to_string()),
        );
        let err = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"duration_ms":1000}"#;
        assert_eq!(
            render_blocks(&parse_claude_line(err)),
            Some("\x1b[31m✗ 失败 (1s)\x1b[0m\n".to_string()),
        );
        // Zero blocks (system / malformed) → None, not an empty string.
        assert_eq!(render_blocks(&parse_claude_line(r#"{"type":"system","subtype":"init"}"#)), None);
        assert_eq!(render_blocks(&parse_claude_line("not json")), None);
    }

    #[test]
    fn chat_stream_event_serializes_with_kind_tag() {
        // The wire schema must carry `kind` as the discriminator tag so the TS
        // union narrows on it. Verify each variant's serialized shape.
        let text = ChatStreamEvent::Text { content: "hi".to_string() };
        let v = serde_json::to_value(&text).unwrap();
        assert_eq!(v["kind"], "text");
        assert_eq!(v["content"], "hi");

        let tool = ChatStreamEvent::ToolUse {
            name: "Read".to_string(),
            input: serde_json::json!({"file_path":"a.rs"}),
        };
        let v = serde_json::to_value(&tool).unwrap();
        assert_eq!(v["kind"], "tool_use");
        assert_eq!(v["name"], "Read");
        assert_eq!(v["input"]["file_path"], "a.rs");

        let res_ok = ChatStreamEvent::Result { is_error: false, secs: 12 };
        let v = serde_json::to_value(&res_ok).unwrap();
        assert_eq!(v["kind"], "result");
        assert_eq!(v["is_error"], false);
        assert_eq!(v["secs"], 12);
    }

    #[test]
    fn claude_line_to_events_maps_each_block_kind() {
        // text → [Text]
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}"#;
        let evs = claude_line_to_events(line);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0], ChatStreamEvent::Text { content: "hello".to_string() });

        // text + tool_use → [Text, ToolUse], order preserved
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"thinking"},{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#;
        let evs = claude_line_to_events(line);
        assert_eq!(evs.len(), 2);
        assert!(matches!(&evs[0], ChatStreamEvent::Text { content } if content == "thinking"));
        assert!(matches!(&evs[1], ChatStreamEvent::ToolUse { name, .. } if name == "Bash"));

        // tool_result → [ToolResult]
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"done","is_error":false}]}}"#;
        let evs = claude_line_to_events(line);
        assert_eq!(evs.len(), 1);
        assert!(matches!(&evs[0], ChatStreamEvent::ToolResult { is_error, .. } if !is_error));

        // result → [Result]
        let line = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":3000}"#;
        let evs = claude_line_to_events(line);
        assert_eq!(evs, vec![ChatStreamEvent::Result { is_error: false, secs: 3 }]);

        // system → []
        assert!(claude_line_to_events(r#"{"type":"system","subtype":"init"}"#).is_empty());
    }

    #[test]
    fn claude_line_to_events_emits_full_ordered_sequence() {
        // A realistic 5-line claude run: text → tool_use → tool_result → text →
        // result. The agent:event channel must emit exactly this ordered
        // sequence — the chat UI renders blocks in arrival order.
        let lines = [
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"let me check"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"pkg.json"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"deps here","is_error":false}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"found it"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":5000}"#,
        ];
        let seq: Vec<ChatStreamEvent> = lines.iter().flat_map(|l| claude_line_to_events(l)).collect();
        let kinds: Vec<&str> = seq
            .iter()
            .map(|e| match e {
                ChatStreamEvent::Text { .. } => "text",
                ChatStreamEvent::ToolUse { .. } => "tool_use",
                ChatStreamEvent::ToolResult { .. } => "tool_result",
                ChatStreamEvent::Result { .. } => "result",
            })
            .collect();
        assert_eq!(kinds, vec!["text", "tool_use", "tool_result", "text", "result"]);
    }

    #[test]
    fn session_idle_timeout_defaults_to_300_when_env_unset() {
        // The idle timeout replaces the old fixed 600s wall-clock kill. Default
        // is 300s of NO output — a streaming long task never trips it; only a
        // hung process (zero output) does. (Env override path verified manually:
        // DEVWORKBENCH_SESSION_IDLE_TIMEOUT_SECS=0 disables, =N sets N seconds.)
        std::env::remove_var("DEVWORKBENCH_SESSION_IDLE_TIMEOUT_SECS");
        assert_eq!(
            session_idle_timeout_secs(),
            DEFAULT_SESSION_IDLE_TIMEOUT_SECS,
            "without the env override the idle timeout is the default",
        );
        assert_eq!(DEFAULT_SESSION_IDLE_TIMEOUT_SECS, 300);
    }

    #[test]
    fn now_millis_is_nonzero_and_non_decreasing() {
        let a = now_millis();
        let b = now_millis();
        assert!(a > 0, "epoch millis must be nonzero: {a}");
        assert!(b >= a, "successive reads must not go backwards: {a} -> {b}");
    }

    #[test]
    fn pi_spawn_uses_print_and_stdin_not_prompt_flag() {
        // Regression: pi CLI (v0.79.3) has NO --prompt option. The old code did
        // `pi --prompt "<text>"` and pi died with "Unknown option: --prompt" —
        // pi never ran at all (the "调用pi不起作用" symptom). Correct call is
        // `pi --print` with the prompt on stdin, exactly like claude --print.
        {
            let mut cache = EXE_CACHE.lock().unwrap();
            cache.insert("pi".to_string(), std::path::PathBuf::from("pi"));
        }
        let cfg = build_spawn_config(&AgentType::Pi, "/p", "hello", None)
            .expect("build_spawn_config for Pi");
        assert!(
            cfg.args.contains(&"--print".to_string()),
            "Pi must use --print (non-interactive stdin mode): {:?}",
            cfg.args,
        );
        assert!(
            !cfg.args.contains(&"--prompt".to_string()),
            "Pi must NOT pass --prompt (pi errors 'Unknown option'): {:?}",
            cfg.args,
        );
        assert!(
            cfg.stdin_prompt.is_some(),
            "Pi prompt must go via stdin (like claude), not argv: {:?}",
            cfg.args,
        );
        // The prompt must NOT leak into argv (would double-deliver alongside stdin).
        assert!(
            !cfg.args.contains(&"hello".to_string()),
            "Pi prompt leaked into positional argv: {:?}",
            cfg.args,
        );
    }

    #[test]
    fn qwen_resolves_to_qwen_command_not_qwen_code() {
        // Regression: command_name was "qwen-code", but the installed CLI is
        // `qwen` (qwen-code is NOT in PATH). resolve_agent_exe could never find
        // it, so QwenCode never spawned at all. The command MUST be "qwen".
        assert_eq!(AgentType::QwenCode.command_name(), "qwen");
        {
            let mut cache = EXE_CACHE.lock().unwrap();
            cache.insert("qwen".to_string(), std::path::PathBuf::from("qwen"));
        }
        let cfg = build_spawn_config(&AgentType::QwenCode, "/p", "hello", None)
            .expect("build_spawn_config for QwenCode must resolve the 'qwen' command");
        assert_eq!(
            cfg.program,
            std::path::PathBuf::from("qwen"),
            "program must be the resolved qwen exe, not a phantom qwen-code",
        );
        // qwen passes --prompt (deprecated but functional per `qwen --help`; -p/--prompt
        // is the documented non-interactive flag). The real fix is the command name above.
        assert!(
            cfg.args.contains(&"--prompt".to_string()),
            "QwenCode passes --prompt: {:?}",
            cfg.args,
        );
    }
}
