use crate::models::{AgentType, ContextSnapshot, FileDiff, Session, SessionStatus};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::sync::{Arc, Mutex};
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

impl Default for AgentProcesses {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentProcesses {
    pub fn new() -> Self {
        Self {
            processes: Mutex::new(HashMap::new()),
        }
    }

    /// Kill every tracked agent process — used on app exit so closing the
    /// window while an agent is running doesn't leave orphan CLI children
    /// (claude/codex/gemini) holding file locks and burning API quota (B3).
    /// Windows `taskkill /F /T` takes the whole tree (MCP server grandchildren
    /// included). Best-effort: a kill failure is logged, not fatal, since the
    /// app is exiting anyway.
    pub fn kill_all(&self) {
        let map = match self.processes.lock() {
            Ok(m) => m,
            Err(e) => {
                log::warn!("[agent-processes] processes lock poisoned on exit: {e}");
                return;
            }
        };
        for (sid, tracked) in map.iter() {
            // TrackedProcess has a single Pipe(pid) variant today (stop_agent
            // matches `Some(TrackedProcess::Pipe(pid))` with no other arm), so
            // this destructures unconditionally.
            let TrackedProcess::Pipe(pid) = tracked;
            // Best-effort: on exit the process may already be gone, so a
            // non-zero kill status is expected. Distinguish success from
            // failure in the log rather than claiming "killed" unconditionally
            // — an honest warn when the signal didn't land helps diagnose
            // orphaned-process leaks instead of masking them.
            if kill_orphan(*pid) {
                log::info!("[agent-processes] killed orphan on exit: {sid} (pid {pid})");
            } else {
                log::warn!(
                    "[agent-processes] failed to kill orphan on exit: {sid} (pid {pid}) — likely already exited"
                );
            }
        }
    }
}

/// Signal an agent/orphan process to terminate. Returns whether the kill
/// command ran AND reported success. Best-effort: the target may already be
/// gone, in which case the OS reports a non-zero status (callers log a warn,
/// not a spurious "killed"). `CREATE_NO_WINDOW` avoids a flashing console on
/// Windows; `-TERM` gives the child a chance to clean up on Unix. Shared by
/// `kill_all` (exit cleanup) and `stop_agent` (user-initiated stop) so the
/// two stay in sync instead of duplicating the platform branches.
#[cfg(target_os = "windows")]
fn kill_orphan(pid: u32) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(not(target_os = "windows"))]
fn kill_orphan(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
            let mappings: &[(&str, &str, &str)] =
                &[("claude", "@anthropic-ai/claude-code", "claude.exe")];
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
    /// whose non-interactive mode already streams human-readable text and has
    /// no structured stream-json mode (codex/cursor/copilot/pi).
    Raw,
    /// `-o stream-json` (claude/qwen/gemini): each stdout line is one JSON
    /// event. The reader selects a parser by agent_type — claude+qwen share
    /// `parse_claude_line` (near-identical Anthropic-style schema), gemini uses
    /// `parse_gemini_line` (a distinct flat schema) — then renders
    /// human-readable text (assistant text, tool calls, results) so the user
    /// watches progress live. Without stream-json these CLIs buffer their
    /// non-interactive output (claude) or emit plain text with no structured
    /// blocks (gemini/qwen), so the terminal shows no tool cards.
    StructuredJson,
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
        let cache = EXE_CACHE
            .lock()
            .map_err(|e| format!("EXE 缓存锁失败: {}", e))?;
        if let Some(path) = cache.get(&key) {
            return Ok(path.clone());
        }
    }
    let path = resolve_agent_exe(agent_type)?;
    let mut cache = EXE_CACHE
        .lock()
        .map_err(|e| format!("EXE 缓存锁失败: {}", e))?;
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
    let stdin_prompt = if use_stdin {
        Some(prompt.to_string())
    } else {
        None
    };

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
            // stream-json (verified on gemini 0.18.4: `-o`/`--output-format`
            // with choices text/json/stream-json). Each stdout line is one flat
            // top-level JSON event (init/message/tool_use/tool_result/result),
            // parsed by `parse_gemini_line` — a schema distinct from claude's
            // message.content[] array. Without it gemini's non-interactive
            // output is plain text with no structured blocks / tool cards.
            args.push("-o".to_string());
            args.push("stream-json".to_string());
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
            // stream-json (verified on qwen 0.14.3: same `-o` flag as claude).
            // qwen's schema is Anthropic-SDK-style and near-identical to
            // claude's (type/subtype/is_error/duration_ms + extra uuid/
            // num_turns/usage), so it reuses `parse_claude_line`. We do NOT add
            // `--include-partial-messages`: that switches qwen to per-token
            // deltas (content_block_delta) needing a separate accumulator —
            // without it each message arrives whole, same shape as claude.
            args.push("-o".to_string());
            args.push("stream-json".to_string());
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
        output_mode: if matches!(
            agent_type,
            AgentType::ClaudeCode | AgentType::GeminiCli | AgentType::QwenCode
        ) {
            OutputMode::StructuredJson
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
        /// claude wire `tool_use` block id (`toolu_...`), used to pair the
        /// later tool_result by id instead of FIFO position. None when claude
        /// omits it or for synthetic blocks. Mirrors the symmetric
        /// `ToolResult.tool_use_id` that points back here.
        id: Option<String>,
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
                                blocks.push(ClaudeBlock::Text {
                                    content: t.to_string(),
                                });
                            }
                        }
                    }
                    "tool_use" => {
                        let id = block
                            .get("id")
                            .and_then(|s| s.as_str())
                            .map(|s| s.to_string());
                        let name = block
                            .get("name")
                            .and_then(|s| s.as_str())
                            .unwrap_or("tool")
                            .to_string();
                        let input = block.get("input").cloned();
                        blocks.push(ClaudeBlock::ToolUse { id, name, input });
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

/// Parse one `gemini -o stream-json` line into zero or more structured blocks.
///
/// gemini's stream-json schema (verified on 0.18.4 by capturing real events) is
/// a FLAT top-level event per line — fundamentally different from claude's
/// `message.content[]` array:
///   - `init`        → session/model bootstrap, no user-facing content → []
///   - `message`     → `content` is a FLAT STRING (not an array); `role` marks
///     user vs assistant turns. Both user echo and assistant text flow through
///     here as Text blocks.
///   - `tool_use`    → top-level event with `tool_name` / `parameters` /
///     `tool_id` (NOT nested in message.content, NOT named `name`/`input` like
///     claude).
///   - `tool_result` → top-level event with `tool_id` / `output` / `status`.
///   - `result`      → terminal event; `status: "success"|"error"` (NOT
///     `subtype`), NO `is_error` field (verdict = status != "success"), and
///     `duration_ms` is NESTED under `stats` (NOT top-level like claude).
///   - `error`       → pre-result API failure → [] (the following `result`
///     event carries the terminal verdict; emitting here would double it).
///
/// Tolerant like `parse_claude_line`: a `Value` + `.get()` chain degrades
/// gracefully — unknown/missing fields ⇒ skip, never panic — so one odd line
/// never breaks the stream.
pub fn parse_gemini_line(line: &str) -> Vec<ClaudeBlock> {
    let v: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let ty = match v.get("type").and_then(|s| s.as_str()) {
        Some(t) => t,
        None => return Vec::new(),
    };
    match ty {
        "message" => {
            // content is a FLAT string (NOT an array like claude). gemini
            // echoes the user's prompt back as role:"user" — rendering that
            // would duplicate the prompt already shown in the chat input. Skip
            // role:"user" ONLY; do NOT assume the assistant role value (gemini
            // may use "model"/"assistant"/…), so every non-user role renders.
            let role = v.get("role").and_then(|s| s.as_str()).unwrap_or("");
            if role == "user" {
                return Vec::new();
            }
            match v.get("content").and_then(|c| c.as_str()) {
                Some(t) if !t.is_empty() => vec![ClaudeBlock::Text {
                    content: t.to_string(),
                }],
                _ => Vec::new(),
            }
        }
        "tool_use" => {
            // gemini's tool_use carries the pairing key as `tool_id` — the SAME
            // field the tool_result arm below reads back. Surface it so the
            // reverse map (chat_event_to_agent_events) pairs by id instead of
            // FIFO, mirroring claude's `id` on the OpaqueAgent path.
            let id = v
                .get("tool_id")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
            let name = v
                .get("tool_name")
                .and_then(|s| s.as_str())
                .unwrap_or("tool")
                .to_string();
            let input = v.get("parameters").cloned();
            vec![ClaudeBlock::ToolUse { id, name, input }]
        }
        "tool_result" => {
            let tool_use_id = v
                .get("tool_id")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
            let content = v.get("output").cloned();
            // status is a string ("success"/"error"); map to Option<bool>.
            let is_error = v
                .get("status")
                .and_then(|s| s.as_str())
                .map(|s| s == "error");
            vec![ClaudeBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            }]
        }
        "result" => {
            // gemini nests duration_ms under `stats` (verified) and uses
            // `status` as the verdict discriminator (no is_error field):
            // success vs anything else (error / interrupted / …).
            let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("");
            let dur_ms = v
                .get("stats")
                .and_then(|s| s.get("duration_ms"))
                .and_then(|d| d.as_u64())
                .unwrap_or(0);
            vec![ClaudeBlock::Result {
                is_error: status != "success",
                secs: dur_ms / 1000,
            }]
        }
        // init (bootstrap) / error (pre-result API failure — the result event
        // carries the terminal verdict) / unknown future event types: skip.
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// A1 — pty usage → cost_records
// ---------------------------------------------------------------------------
//
// The survival-line 断链: claude/codex/gemini/qwen CLI runs went through pty,
// their stream-json `result` event carried token usage (+ sometimes USD cost),
// but NOTHING recorded it to `cost_records`. Only the GLM kernel path
// (DbCostSink on GlmChatModel) ever inserted rows — so the Dashboard's cost /
// token totals silently omitted every CLI agent run. This closes that gap.
//
// Blueprints: cline `claude-code.ts:52-206` (chunk-type dispatch — only the
// terminal `result` chunk carries usage), `:177` (Anthropic `input_tokens`
// ALREADY includes cache tokens — record verbatim, never re-add), `:50/66/192`
// (subscription fallback: when the CLI reports no `total_cost_usd` — subscription
// apiKeySource=="none", or qwen/gemini which never report cost — compute locally
// from the pricing table instead of a silent 0).

/// Token usage + optional provider-reported cost extracted from a stream-json
/// `result` event. Mirrors what cline pulls from the claude result chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct PtyUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// Provider-reported USD cost (claude `result.total_cost_usd`). `None` when
    /// the CLI doesn't report cost → downstream falls back to local pricing.
    pub cost_usd: Option<f64>,
}

/// Extract token usage + cost from one stream-json `result` event line.
/// Returns `None` for non-result lines, malformed JSON, or a result with no
/// usable token fields.
///
/// Token field paths differ per CLI (all verified from captured events):
///   - claude: `stats.input_tokens` / `stats.output_tokens` + top-level `total_cost_usd`.
///   - qwen (reuses the claude parser): top-level `usage.input_tokens` / `usage.output_tokens`, no cost.
///   - gemini: nested `stats.input_tokens` / `stats.output_tokens`, no cost.
///
/// `stats` is tried first (claude/gemini), then `usage` (qwen). Tokens are
/// recorded AS-IS (Anthropic `input_tokens` already includes cache, so no
/// re-derivation — the cline `:177` double-count trap can't trigger here).
pub fn extract_pty_usage(line: &str) -> Option<PtyUsage> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type").and_then(|s| s.as_str()) != Some("result") {
        return None;
    }
    let stats = v.get("stats");
    let usage = v.get("usage");
    let inp = stats
        .and_then(|s| s.get("input_tokens"))
        .or_else(|| usage.and_then(|u| u.get("input_tokens")))
        .and_then(|n| n.as_u64());
    let out = stats
        .and_then(|s| s.get("output_tokens"))
        .or_else(|| usage.and_then(|u| u.get("output_tokens")))
        .and_then(|n| n.as_u64());
    let (input_tokens, output_tokens) = match (inp, out) {
        (Some(i), Some(o)) => (i as u32, o as u32),
        // A result with no token fields carries nothing bookable.
        _ => return None,
    };
    let cost_usd = v.get("total_cost_usd").and_then(|c| c.as_f64());
    Some(PtyUsage {
        input_tokens,
        output_tokens,
        cost_usd,
    })
}

/// Record one pty CLI run's usage into `cost_records`, fire-and-forget.
///
/// Runs on a plain `std::thread` (NOT `tokio::spawn_blocking`) because the pty
/// reader is itself a bare std::thread with no runtime in scope — mirroring the
/// existing `{sid}.log` write pattern in the same reader. A cost-write failure
/// is logged and never breaks the agent stream.
///
/// Subscription fallback (cline `claude-code.ts:50/66/192`): when the CLI
/// reports no `total_cost_usd` (subscription, or qwen/gemini), compute from the
/// local pricing table. An unknown model honestly records cost 0 rather than a
/// guess — the tokens are still booked, so usage visibility is preserved.
fn record_pty_usage(
    db: crate::db::DbState,
    session_id: &str,
    agent_type: &str,
    model: &str,
    usage: &PtyUsage,
) {
    let cost = usage.cost_usd.unwrap_or_else(|| {
        crate::cost::pricing::cost(
            usage.input_tokens,
            usage.output_tokens,
            crate::cost::pricing::pricing_for(model),
        )
    });
    let rec = crate::models::CostRecord {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: Some(session_id.to_string()),
        agent_type: agent_type.to_string(),
        model: model.to_string(),
        input_tokens: usage.input_tokens as i64,
        output_tokens: usage.output_tokens as i64,
        // PTY/raw-agent usage (claude-code stream-json) records Anthropic
        // input_tokens verbatim, which ALREADY includes prompt-cache tokens (per
        // cline claude-code.ts:177 — never re-add). The cache tiers aren't
        // surfaced separately by the CLI stream, so they stay 0 here; the
        // ReactAgent path is the one that reports distinct cache tiers (B5).
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_usd: cost,
        recorded_at: chrono::Utc::now().to_rfc3339(),
    };
    let sid = session_id.to_string();
    std::thread::spawn(move || match db.get() {
        Ok(conn) => {
            if let Err(e) = crate::cost::agentfare::insert_cost_record(&conn, &rec) {
                log::warn!("[pty-cost] insert failed sid={sid}: {e}");
            }
        }
        Err(e) => log::warn!("[pty-cost] db lock failed sid={sid}: {e}"),
    });
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
            ClaudeBlock::ToolUse { name, input, .. } => {
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum ChatStreamEvent {
    #[serde(rename = "text")]
    Text { content: String },
    /// Reasoning/thinking trace (GLM Interleaved Thinking, claude extended
    /// thinking). Rendered as a collapsible thinking block, separate from the
    /// answer text. Streamed chunk-by-chunk by the transparent ReactAgent;
    /// opaque agents emit it when their CLI parser surfaces a thinking block.
    #[serde(rename = "thinking")]
    Thinking { content: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        /// tool_call_id pairing key. Populated end-to-end on BOTH paths now:
        /// OpaqueAgent (claude wire `id` / gemini `tool_id`, preserved via pty
        /// `to_event`) AND ReactKernel (`ToolCall.id` — the LLM-issued
        /// correlation id — forwarded into `ToolCallEvent.id` by react_agent,
        /// so DB replay pairs by id instead of degrading to FIFO). `Option` +
        /// `skip_serializing_if` keeps the wire clean and lets pre-id session
        /// blocks deserialize unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
        content: String,
        is_error: bool,
    },
    #[serde(rename = "result")]
    Result { is_error: bool, secs: u64 },
    /// A file was changed on disk by the agent (a write_file/patch tool landed).
    /// Surfaced as a lightweight path line so the user sees per-write mutations
    /// as they happen — distinct from the aggregated `Done.files_changed` list
    /// (a git-diff snapshot taken once at run end) and from a tool_result card
    /// (which shows tool output, not which path was touched). Maps from
    /// kernel-core `AgentEvent::FileChanged`.
    #[serde(rename = "file_changed")]
    FileChanged { path: String },
    /// Context auto-compaction meta-event (v1.3 C2). NOT produced by a model
    /// turn — emitted by the compaction sink when `maybe_compact` replaces part
    /// of the history. A meta-event: it never enters the model's history
    /// (dropped in turns_to_history / blocks_to_assistant_message), it only
    /// tells the UI to render a "context compacted" summary card. Expand the
    /// card to read the archived原文 via `read_compact_archive_cmd`. `is_error`
    /// marks a breaker trip (summarizer failed repeatedly; compaction suspended
    /// for the rest of the run — the run continues, just without further
    /// compression).
    #[serde(rename = "compact")]
    Compact {
        summary: String,
        archived_at: Option<String>,
        dropped_count: usize,
        is_error: bool,
    },
    /// Human-Gate approval request (Clutch #3). NOT a chat block — a control
    /// signal: emitted when a destructive action is about to land in
    /// `PermissionMode::HumanGate`, telling the UI to open an approval modal.
    /// The agent SUSPENDS until `resolve_human_gate_cmd` delivers a decision
    /// (or 300s auto-rejects). Never persisted into session.blocks and never
    /// enters model history (`react_chat` filters it out, like `Compact`).
    #[serde(rename = "approval_required")]
    ApprovalRequired {
        /// Tool name about to run (e.g. `bash`, `write_file`).
        tool: String,
        /// Raw JSON arguments string — the modal previews these so the user
        /// sees exactly what would execute.
        arguments: String,
        /// `approve__{session_id}__{seq}` — the UI returns this verbatim in
        /// `resolve_human_gate_cmd` to resume the right suspended call.
        resume_token: String,
        /// One-line "why this is destructive" summary (modal title).
        summary: String,
    },
}

impl ClaudeBlock {
    /// Map a parsed block to its wire event form for the `agent:event` channel.
    pub fn to_event(&self) -> ChatStreamEvent {
        match self {
            ClaudeBlock::Text { content } => ChatStreamEvent::Text {
                content: content.clone(),
            },
            ClaudeBlock::ToolUse { id, name, input } => ChatStreamEvent::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone().unwrap_or(serde_json::Value::Null),
            },
            // ToolResult: collapse the raw content value into a readable string.
            // Longer than the 120-char terminal preview — the chat card folds it,
            // so give it more room to stay useful. `tool_use_id` is carried
            // through so the reverse map (chat_event_to_agent_events) can pair
            // by id instead of FIFO position (defect ① root cause).
            ClaudeBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => ChatStreamEvent::ToolResult {
                tool_use_id: tool_use_id.clone(),
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

/// Fold consecutive `Text`/`Thinking` events into one run each (same semantics
/// as the frontend's `appendBlock` merge). Persisted blocks should match what
/// the live in-memory Map held — not one entry per streaming token delta —
/// otherwise a reloaded session renders N tiny text/thinking cards instead of
/// the single merged paragraph.
///
/// Thinking must fold too: GLM Interleaved Thinking streams chunk-by-chunk, and
/// the old Text-only fold left thinking as one block per token. Session 82e56ebe
/// (4 min, glm-5.2) persisted 1681 single-token thinking blocks into a 128 KB
/// `sessions.blocks` row — the frontend merges for LIVE render
/// (`BlocksView::normalizeEvents`), but the persisted replica did not, so
/// history replay / direct DB reads saw the碎片. Folding both kinds here makes
/// the persisted copy match the live view.
pub(crate) fn merge_consecutive_runs(events: Vec<ChatStreamEvent>) -> Vec<ChatStreamEvent> {
    let mut out: Vec<ChatStreamEvent> = Vec::with_capacity(events.len());
    for ev in events {
        match (&ev, out.last_mut()) {
            (
                ChatStreamEvent::Text { content: incoming },
                Some(ChatStreamEvent::Text { content: acc }),
            ) => acc.push_str(incoming),
            (
                ChatStreamEvent::Thinking { content: incoming },
                Some(ChatStreamEvent::Thinking { content: acc }),
            ) => acc.push_str(incoming),
            _ => out.push(ev),
        }
    }
    out
}

/// Cap every string value nested inside a JSON value to `max_chars` (appending
/// "…") — recursing through objects and arrays. Used to shrink ToolUse.input
/// for the persisted copy while keeping the JSON structure intact so the
/// frontend still renders it.
fn cap_json_string_values(value: serde_json::Value, max_chars: usize) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            if s.chars().count() > max_chars {
                let capped: String = s.chars().take(max_chars).collect();
                serde_json::Value::String(format!("{}…", capped))
            } else {
                serde_json::Value::String(s)
            }
        }
        serde_json::Value::Array(arr) => serde_json::Value::Array(
            arr.into_iter()
                .map(|v| cap_json_string_values(v, max_chars))
                .collect(),
        ),
        serde_json::Value::Object(obj) => serde_json::Value::Object(
            obj.into_iter()
                .map(|(k, v)| (k, cap_json_string_values(v, max_chars)))
                .collect(),
        ),
        other => other,
    }
}

/// Apply persistence caps to a block list: only `ToolUse.input` strings are
/// capped. Live emit is NOT capped — only the DB-bound replica. Prevents a
/// giant Edit `new_string` from ballooning the row. Text is the user-facing
/// answer (left whole), Result carries no payload, and ToolResult.content was
/// already preview-capped at emit time.
pub(crate) fn cap_blocks_for_persist(
    events: Vec<ChatStreamEvent>,
    max_chars: usize,
) -> Vec<ChatStreamEvent> {
    events
        .into_iter()
        .map(|ev| match ev {
            ChatStreamEvent::ToolUse { id, name, input } => ChatStreamEvent::ToolUse {
                id,
                name,
                input: cap_json_string_values(input, max_chars),
            },
            other => other,
        })
        .collect()
}

/// Test-only wrapper: parse a claude stream-json line into the wire events the
/// chat frontend consumes. The production reader thread calls `ClaudeBlock::
/// to_event` directly (no double-parse); this wraps it for unit tests, gated
/// cfg(test) so it never ships in a release binary.
#[cfg(test)]
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
#[allow(clippy::too_many_arguments)]
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
        Some(cid) => load_prior_turns(&db_conn, cid, parent_session_id),
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
        prior_turns
            .last()
            .filter(|t| t.agent_type == agent_type)
            .map(|t| t.id.as_str())
    };

    let injected_prompt = inject_conversation_context(prompt, &prior_turns, &agent_type);
    // Inject project knowledge context into the prompt — run in background thread
    // with a 2-second timeout to avoid blocking the UI on slow DB queries.
    let injected_prompt =
        inject_knowledge_with_timeout(&db_conn, &agent_type, project_path, &injected_prompt);
    // Inject @file references with actual file content
    let injected_prompt = inject_file_references(project_path, &injected_prompt);
    // D3: resolve @memory:<title> explicit references against the project's
    // active knowledge entries (after @file injection, before spawn). Best-effort:
    // a DB error leaves the prompt untouched.
    let injected_prompt = match db_conn.get() {
        Ok(conn) => {
            let hash = crate::activity::hash_project_path(project_path);
            crate::knowledge::memory_ref::resolve_memory_refs(&injected_prompt, &conn, &hash)
        }
        Err(_) => injected_prompt,
    };
    let config = build_spawn_config(&agent_type, project_path, &injected_prompt, model)?;

    // Unified pipe mode for all platforms. PTY path removed from runtime:
    // all target CLIs support non-interactive --print/exec pipe mode.
    spawn_pipe_fallback(
        app,
        processes,
        db_conn,
        &config,
        &agent_type,
        project_path,
        linked_requirement_id,
        parent_for_resume,
        conversation_id,
        prompt,
        model,
    )
}

// ---------------------------------------------------------------------------
// Conversation context bridge (cross-agent history injection)
// ---------------------------------------------------------------------------

/// Load the completed prior turns of a conversation (oldest-first), best-effort.
/// The currently-spawning turn isn't in the DB yet, so it's naturally excluded.
/// A DB failure degrades to "no prior history" rather than blocking the spawn.
pub(crate) fn load_prior_turns(
    db_conn: &crate::db::DbState,
    conversation_id: &str,
    parent_session_id: Option<&str>,
) -> Vec<crate::models::Session> {
    let Ok(conn) = db_conn.get() else {
        return Vec::new();
    };
    match parent_session_id {
        // Branch-pure: walk ONLY the ancestor chain of this turn's parent. This
        // is what makes edit-and-regenerate fork safely — a forked turn's parent
        // is the edited turn's own parent, so its history is exactly that
        // parent's ancestors, never the sibling branches being replaced. Without
        // this, the conversation-wide loader would leak the edited-out branch.
        Some(pid) => crate::agents::session::load_turn_chain_db(&conn, pid).unwrap_or_default(),
        // Flat: first turn, or a linear continue with no explicit parent (the
        // pipe path derives a parent afterwards). A linear conversation is its
        // own single branch, so conversation-wide loading == the ancestor chain.
        None => crate::agents::session::load_turns_for_conversation_db(&conn, conversation_id)
            .unwrap_or_default(),
    }
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

    let mut selected: Vec<String> = blocks.to_vec();
    while selected.iter().map(|b| b.len()).sum::<usize>() > CONTEXT_BRIDGE_TOTAL_MAX_CHARS
        && selected.len() > 1
    {
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_running_session_row(
    session_id: &str,
    project_path: &str,
    agent_type: &AgentType,
    prompt: &str,
    model: Option<&str>,
    conversation_id: &str,
    linked_requirement_id: Option<&str>,
    parent_session_id: Option<&str>,
    task_ref: Option<&str>,
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
        blocks: None,
        task_ref: task_ref.map(|s| s.to_string()),
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
    crate::agents::session::insert_session_db(&conn, session).map_err(|e| e.to_string())?;

    if conversation_id.is_some() {
        let now = chrono::Local::now().to_rfc3339();
        let patch = serde_json::json!({
            "lastAgent": serde_json::to_string(agent_type).unwrap_or_default().trim_matches('"'),
            "lastActivityAt": now,
        });
        let _ = crate::agents::session::update_conversation_db(&conn, resolved_conv_id, patch);
    }

    let _ = crate::activity::record_event(
        &conn,
        &crate::activity::make_activity_event(
            &session.id,
            project_path,
            agent_type,
            "session_started",
            &format!("{} session started", agent_type.display_name()),
            None,
            None,
        ),
    );
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
#[allow(clippy::too_many_arguments)]
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
    blocks: Option<Vec<ChatStreamEvent>>,
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
    if let Some(blocks) = blocks {
        // Persist the chat blocks so a finalized session replays via BlocksView
        // instead of falling back to the raw terminal log. Merge consecutive
        // text deltas (match the live Map's shape) and cap giant ToolUse inputs
        // before serializing — live emit is untouched.
        let persisted = cap_blocks_for_persist(merge_consecutive_runs(blocks), 8000);
        if let Ok(val) = serde_json::to_value(persisted) {
            patch["blocks"] = val;
        }
    }

    log::info!(
        "[completion] Session {} locking DB for completion update...",
        session_id
    );
    // won_race == false only when update_session_db returned Ok(0): the session
    // was already terminal (a racing stop_agent_session won). In that case skip
    // BOTH the activity record and the agent:completed emit — finalize already
    // lost, and re-emitting would double-fire / log the wrong terminal status.
    // On a DB write Err (rare) keep the prior best-effort behavior: still record
    // + emit so the UI spinner clears instead of hanging.
    let mut won_race = true;
    if let Ok(conn) = db_conn.get() {
        log::info!(
            "[completion] Session {} DB locked, writing completion...",
            session_id
        );
        match crate::agents::session::update_session_db(&conn, session_id, patch) {
            Ok(rows) => won_race = rows > 0,
            Err(e) => log::error!("[finalize] status update failed for {}: {e}", session_id),
        }
        if won_race {
            let event_type = match session_status {
                SessionStatus::Completed => "session_completed",
                _ => "session_failed",
            };
            let _ = crate::activity::record_event(
                &conn,
                &crate::activity::make_activity_event(
                    session_id,
                    project_path,
                    agent_type,
                    event_type,
                    &format!(
                        "{} session {}",
                        agent_type.display_name(),
                        session_status.as_str()
                    ),
                    None,
                    files_for_activity,
                ),
            );
        }
    } else {
        log::error!(
            "[finalize] Failed to lock DB for session {} completion update",
            session_id
        );
    }
    if won_race {
        log::info!(
            "[finalize] Emitting agent:completed for session {}",
            session_id
        );
        let _ = app.emit(
            "agent:completed",
            serde_json::json!({
                "sessionId": session_id,
                "status": session_status.as_str(),
                "exitCode": exit_code,
            }),
        );
    } else {
        log::info!(
            "[finalize] Session {} already terminal — skipping agent:completed emit",
            session_id
        );
    }

    run_post_session_hooks(
        db_conn.clone(),
        project_path.to_string(),
        session_id.to_string(),
        agent_type.clone(),
        session_status,
    );
}

#[allow(clippy::too_many_arguments)]
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
    log::info!(
        "[PIPE spawn] program={}, args={:?}, cwd={}, stdin_prompt={}",
        config.program.display(),
        config.args,
        config.cwd.display(),
        config.stdin_prompt.is_some()
    );
    let mut cmd = std::process::Command::new(&config.program);
    let use_stdin = config.stdin_prompt.is_some();
    cmd.args(&config.args)
        .current_dir(&config.cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(if use_stdin {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        });

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
        .lock()
        .unwrap_or_else(|e| e.into_inner())
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
                    let _ = std::fs::write(
                        dir.join(format!("{}.pre-diff", bg_sid)),
                        pre_diff.join("\n"),
                    );
                }
            })
            .ok();
    }

    // DB registration runs AFTER spawn + track. If it fails here, the reader/
    // wait threads were never started, so the child would run with nobody
    // draining its stdout/stderr — it blocks once the OS pipe buffer fills and
    // hangs indefinitely (un-killable from the UI, since the caller sees this
    // spawn as failed and never wires up stop). Kill the child and untrack it
    // before propagating the error. The process-table entry from above is
    // already present; this removes it so the failed run leaves no live process.
    let session = {
        let mut abort_on_db_fail = |err: String| -> String {
            let _ = child.kill();
            let _ = child.wait();
            processes
                .processes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&session_id);
            err
        };
        let resolved_conv_id = match resolve_or_create_conversation(
            &db_conn,
            conversation_id,
            project_path,
            prompt,
            agent_type,
        ) {
            Ok(v) => v,
            Err(e) => return Err(abort_on_db_fail(e)),
        };
        let session = build_running_session_row(
            &session_id,
            project_path,
            agent_type,
            prompt,
            model,
            &resolved_conv_id,
            linked_requirement_id,
            parent_session_id,
            None,
        );
        if let Err(e) = register_running_session(
            &db_conn,
            app,
            &session,
            conversation_id,
            &resolved_conv_id,
            project_path,
            agent_type,
        ) {
            return Err(abort_on_db_fail(e));
        }
        session
    };

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
    // Accumulated wire events mirrored from the agent:event emits, so the
    // finalized session can be persisted and replayed via BlocksView. Shared
    // between the reader thread (push) and the wait thread (take at finalize).
    let session_blocks = Arc::new(std::sync::Mutex::new(Vec::<ChatStreamEvent>::new()));

    let output_mode = config.output_mode; // Copy — drives stdout interpretation
                                          // agent_type selects the StructuredJson parser (claude+qwen reuse
                                          // parse_claude_line; gemini uses parse_gemini_line). Captured by value into
                                          // the reader closure, the same way output_mode is above.
    let agent_type_reader = agent_type.clone();
    let last_activity_reader = last_activity.clone();
    let session_blocks_reader = Arc::clone(&session_blocks);
    // A1 — capture what the pty cost recorder needs: the DB handle, the agent's
    // CLI name (cost_records.agent_type), and the model (pricing lookup; falls
    // back to the CLI name when DevWorkbench let the CLI pick its own default).
    let db_reader = db_conn.clone();
    let agent_str_reader = agent_type.command_name().to_string();
    let model_str_reader = model
        .map(|m| m.to_string())
        .unwrap_or_else(|| agent_str_reader.clone());
    std::thread::spawn(move || {
        let mut output_log: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];

        if let Some(mut out) = stdout {
            match output_mode {
                OutputMode::Raw => loop {
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
                },
                OutputMode::StructuredJson => {
                    // Each stdout line is one structured event. Parse it ONCE
                    // into ClaudeBlocks — selecting the parser by agent_type
                    // (claude+qwen share parse_claude_line for their
                    // near-identical Anthropic-style schema; gemini uses
                    // parse_gemini_line for its flat schema) — then fan out TWO
                    // channels from the same parse (no double-parse):
                    //   - agent:event : structured ChatStreamEvent per block, for
                    //     the chat block-card UI (text / tool_use / tool_result /
                    //     result). Raw JSON never reaches the user.
                    //   - pty:output   : the rendered ANSI text, for the terminal
                    //     fallback view AND the {sid}.log replay file.
                    use std::io::BufRead;
                    let parse_fn: fn(&str) -> Vec<ClaudeBlock> = match agent_type_reader {
                        AgentType::GeminiCli => parse_gemini_line,
                        // claude + qwen share the Anthropic-style parser.
                        _ => parse_claude_line,
                    };
                    let reader = std::io::BufReader::new(out);
                    for line in reader.lines() {
                        let line = match line {
                            Ok(l) => l,
                            Err(_) => break,
                        };
                        let blocks = parse_fn(&line);
                        // A1 — the terminal `result` line carries the run's
                        // token usage (+ claude's USD cost). Record it to
                        // cost_records so CLI runs show up in the Dashboard
                        // (previously only the GLM kernel path was booked). The
                        // all-zero guard drops auth-failure error results that
                        // genuinely burned no tokens.
                        if let Some(usage) = extract_pty_usage(&line) {
                            if usage.input_tokens > 0
                                || usage.output_tokens > 0
                                || usage.cost_usd.unwrap_or(0.0) > 0.0
                            {
                                record_pty_usage(
                                    db_reader.clone(),
                                    &sid_reader,
                                    &agent_str_reader,
                                    &model_str_reader,
                                    &usage,
                                );
                            }
                        }
                        if blocks.is_empty() {
                            continue; // system / api_retry noise — neither channel cares
                        }
                        // claude produced output → it's alive, reset the idle timer.
                        last_activity_reader.store(now_millis(), Ordering::Relaxed);
                        // Structured channel first (the chat UI consumes only this).
                        for event in blocks.iter().map(ClaudeBlock::to_event) {
                            // Mirror into the persistence accumulator (cloned —
                            // `event` is also moved into the emit below).
                            if let Ok(mut buf) = session_blocks_reader.lock() {
                                buf.push(event.clone());
                            }
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
    let session_blocks_wait = session_blocks.clone();
    std::thread::spawn(move || {
        let idle_secs = session_idle_timeout_secs();
        log::info!(
            "[PIPE wait] Waiting for session {} (idle_timeout={}s, 0=disabled)",
            sid_exit,
            idle_secs
        );
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
                        let idle_ms =
                            now_millis().saturating_sub(last_activity_wait.load(Ordering::Relaxed));
                        if idle_ms > idle_secs * 1000 {
                            log::warn!(
                                "[PIPE wait] Session {} idle {}ms (>{}s, no output) — killing",
                                sid_exit,
                                idle_ms,
                                idle_secs,
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

        log::info!(
            "[PIPE wait] Session {} exited: code={:?}, status={}, timed_out={}",
            sid_exit,
            exit_code,
            session_status.as_str(),
            timed_out
        );

        std::thread::sleep(std::time::Duration::from_millis(500));

        // Read output summary FIRST so key_output can be derived from it.
        let output_summary = read_output_summary(&sid_exit).or_else(|| {
            if timed_out {
                Some(format!(
                    "(Session killed: no output for {}s — likely hung)",
                    idle_secs
                ))
            } else if exit_code.unwrap_or(-1) == 0 {
                Some("(Agent completed with no text output)".to_string())
            } else {
                Some(format!("(Process exited with code {:?})", exit_code))
            }
        });
        log::info!(
            "[completion] Session {} capturing context snapshot...",
            sid_exit
        );
        let snapshot =
            extract_context_snapshot(&project_path_exit, &sid_exit, output_summary.as_deref());
        log::info!(
            "[completion] Session {} snapshot done ({} files changed, {} key_output chars)",
            sid_exit,
            snapshot
                .as_ref()
                .map(|s| s.files_changed.len())
                .unwrap_or(0),
            snapshot
                .as_ref()
                .map(|s| s.key_output.chars().count())
                .unwrap_or(0)
        );
        // Drain the accumulated blocks for persistence. A poisoned lock (panic
        // in the reader thread) falls back to None → terminal replay, never
        // panics the wait thread. Empty vec → None (raw agent / no agent:event).
        let blocks_snapshot = session_blocks_wait.lock().ok().and_then(|mut buf| {
            let taken = std::mem::take(&mut *buf);
            if taken.is_empty() {
                None
            } else {
                Some(taken)
            }
        });
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
            blocks_snapshot,
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

// Test-only accessors for the (private) process table, so other modules' tests
// can drive stop_agent / the kill-on-drop guard without a real spawned child.
// Track a synthetic Pipe entry (a bogus PID is harmless — stop_agent's kill is
// best-effort and ignores a missing process; the map removal is what tests assert).
#[cfg(test)]
pub(crate) fn track_test_pipe(procs: &Arc<AgentProcesses>, sid: &str, pid: u32) {
    if let Ok(mut m) = procs.processes.lock() {
        m.insert(sid.to_string(), TrackedProcess::Pipe(pid));
    }
}

#[cfg(test)]
pub(crate) fn is_tracked(procs: &Arc<AgentProcesses>, sid: &str) -> bool {
    procs
        .processes
        .lock()
        .map(|m| m.contains_key(sid))
        .unwrap_or(false)
}

/// Stop a running agent session.
pub fn stop_agent(processes: &Arc<AgentProcesses>, session_id: &str) -> Result<(), String> {
    let tracked = {
        let mut map = processes
            .processes
            .lock()
            .map_err(|e| format!("进程表锁失败: {}", e))?;
        map.remove(session_id)
    };

    match tracked {
        Some(TrackedProcess::Pipe(pid)) => {
            // Reuses kill_all's helper. Best-effort: the agent may have already
            // exited; a warn (not a silent `let _ =`) keeps stop failures visible.
            if !kill_orphan(pid) {
                log::warn!(
                    "[agent-processes] failed to kill session {session_id} (pid {pid}) — likely already exited"
                );
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
    let log_path = agents_dir
        .join("outputs")
        .join(format!("{}.log", session_id));
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

/// Persist one compaction chunk as a JSONL line under
/// `~/.dev-workbench/agents/compact/{session_id}.jsonl` (原文归档). Each line
/// is one compaction pass (`micro_clear` or `summarize`) holding the dropped
/// messages verbatim so the user can expand the summary card and read what was
/// compacted away. Append-only — a long run that compacts several times
/// accumulates one line per pass. Best-effort: a write failure is logged, never
/// surfaced (archiving is transparency-only, never blocks compaction).
///
/// Returns the archive path on success (reported back over the wire as
/// `archived_at` so the UI knows an expand view exists), or `None` on any I/O /
/// serialize failure (already logged).
pub(crate) fn append_compact_archive(
    session_id: &str,
    chunk: &crate::kernel_impl::context_compact::ArchivedChunk,
) -> Option<String> {
    let dir = crate::agents::session::agents_dir().ok()?.join("compact");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("[compact] create archive dir failed for {session_id}: {e}");
        return None;
    }
    let path = dir.join(format!("{session_id}.jsonl"));
    let line = serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "kind": chunk.kind,
        "summary": chunk.summary,
        "dropped_count": chunk.dropped_messages.len(),
        "dropped_messages": chunk.dropped_messages,
    });
    let mut serialized = match serde_json::to_string(&line) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[compact] serialize archive line failed for {session_id}: {e}");
            return None;
        }
    };
    serialized.push('\n');
    if let Err(e) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, serialized.as_bytes()))
    {
        log::warn!("[compact] write archive failed for {session_id}: {e}");
        return None;
    }
    Some(path.display().to_string())
}

/// Read all archived compaction chunks for a session (JSONL, oldest first) —
/// the expand view behind a summary card. Each line mirrors one
/// [`append_compact_archive`] write. Returns `None` when no archive exists.
pub(crate) fn read_compact_archive(session_id: &str) -> Option<Vec<serde_json::Value>> {
    let path = crate::agents::session::agents_dir()
        .ok()?
        .join("compact")
        .join(format!("{session_id}.jsonl"));
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => out.push(v),
            Err(e) => log::warn!("[compact] skip malformed archive line for {session_id}: {e}"),
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
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
    let path = crate::agents::session::agents_dir()
        .ok()?
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
            log::warn!(
                "Knowledge injection timed out for project {}, using original prompt",
                project_path
            );
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
        while path_end < chars.len() && !chars[path_end].is_whitespace() && chars[path_end] != '@' {
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

        // F13: only treat as a file reference if it actually exists on disk.
        // The loose `looks_like_path` heuristic (contains '.'/'/'/'\') otherwise
        // matches `@1.0`, `@docs/api`, version strings — reading + injecting
        // those as file contents pollutes the prompt / leaks unrelated files.
        if !std::path::Path::new(&full_match).exists() {
            i = path_end;
            continue;
        }

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
                format!(
                    "[File {} skipped: total injection limit reached]",
                    path.display()
                ),
            ));
            continue;
        }
        total_injected += injected_len;

        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let wrapped = format!(
            "--- BEGIN FILE: {} ({}) ---\n{}\n--- END FILE: {} ---",
            file_name,
            path.display(),
            content,
            file_name
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
                        &conn,
                        &project_path,
                        &session_id,
                        &agent_type,
                    );
                }
            }
            // 2. Quality gate — run subprocess
            let forge_result =
                crate::quality::forge::run_forge_gate(std::path::Path::new(&project_path));
            match forge_result {
                Ok(report) => {
                    if let Ok(conn) = db.get() {
                        let _ = crate::quality::report::save_report(&conn, &report);
                        let _ = crate::quality::feedback::create_feedback(
                            &conn,
                            &report,
                            &project_path,
                            &agent_type,
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
            sid_for_log,
            e
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_consecutive_runs_folds_runs_and_leaves_others() {
        let evs = vec![
            ChatStreamEvent::Text {
                content: "a".into(),
            },
            ChatStreamEvent::Text {
                content: "b".into(),
            },
            ChatStreamEvent::ToolUse {
                id: None,
                name: "Read".into(),
                input: serde_json::json!({"file_path": "/x"}),
            },
            ChatStreamEvent::Text {
                content: "c".into(),
            },
        ];
        let merged = merge_consecutive_runs(evs);
        assert_eq!(merged.len(), 3);
        assert_eq!(
            merged[0],
            ChatStreamEvent::Text {
                content: "ab".into()
            }
        );
        assert_eq!(
            merged[2],
            ChatStreamEvent::Text {
                content: "c".into()
            }
        );
    }

    #[test]
    fn merge_consecutive_runs_empty_and_single() {
        assert!(merge_consecutive_runs(vec![]).is_empty());
        let one = vec![ChatStreamEvent::Text {
            content: "solo".into(),
        }];
        assert_eq!(
            merge_consecutive_runs(one),
            vec![ChatStreamEvent::Text {
                content: "solo".into()
            }]
        );
    }

    #[test]
    fn merge_consecutive_runs_all_tool_use_unchanged() {
        let evs = vec![
            ChatStreamEvent::ToolUse {
                id: None,
                name: "A".into(),
                input: serde_json::Value::Null,
            },
            ChatStreamEvent::ToolUse {
                id: None,
                name: "B".into(),
                input: serde_json::Value::Null,
            },
        ];
        let merged = merge_consecutive_runs(evs.clone());
        assert_eq!(merged, evs);
    }

    #[test]
    fn merge_consecutive_runs_folds_thinking_too() {
        // Regression (session 82e56ebe): GLM streams thinking chunk-by-chunk;
        // the persisted row held 1681 single-token thinking blocks (128 KB).
        // Folding must collapse a run of Thinking into one exactly like Text —
        // and must NOT merge across a different-kind block in between.
        let evs = vec![
            ChatStreamEvent::Thinking { content: "The".into() },
            ChatStreamEvent::Thinking { content: " user".into() },
            ChatStreamEvent::Thinking { content: " wants".into() },
            ChatStreamEvent::Text { content: "answer".into() },
            ChatStreamEvent::Thinking { content: "more".into() },
        ];
        let merged = merge_consecutive_runs(evs);
        assert_eq!(merged.len(), 3, "two thinking runs + one text in between");
        assert_eq!(
            merged[0],
            ChatStreamEvent::Thinking { content: "The user wants".into() }
        );
        assert_eq!(merged[1], ChatStreamEvent::Text { content: "answer".into() });
        assert_eq!(merged[2], ChatStreamEvent::Thinking { content: "more".into() });
    }

    #[test]
    fn cap_blocks_for_persist_truncates_long_tool_use_input_strings() {
        let big = "x".repeat(10_000);
        let evs = vec![ChatStreamEvent::ToolUse {
            id: None,
            name: "Edit".into(),
            input: serde_json::json!({ "file_path": "/p", "new_string": big }),
        }];
        let capped = cap_blocks_for_persist(evs, 8000);
        match &capped[0] {
            ChatStreamEvent::ToolUse { input, .. } => {
                let new_string = input.get("new_string").unwrap().as_str().unwrap();
                assert!(new_string.ends_with('…'));
                // 8000 kept chars + 1 ellipsis char.
                assert_eq!(new_string.chars().count(), 8001);
                // Short sibling field untouched.
                assert_eq!(input.get("file_path").unwrap().as_str(), Some("/p"));
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn cap_blocks_for_persist_leaves_text_result_and_short_input() {
        let evs = vec![
            ChatStreamEvent::Text {
                content: "answer".into(),
            },
            ChatStreamEvent::ToolUse {
                id: None,
                name: "Read".into(),
                input: serde_json::json!({"file_path": "/short"}),
            },
            ChatStreamEvent::ToolResult {
                tool_use_id: None,
                content: "ok".into(),
                is_error: false,
            },
            ChatStreamEvent::Result {
                is_error: false,
                secs: 1,
            },
        ];
        let capped = cap_blocks_for_persist(evs.clone(), 8000);
        assert_eq!(capped, evs);
    }

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
        assert!(
            parse_numstat_line("1\t2\t").is_none(),
            "empty path rejected"
        );
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
        assert!(
            out.starts_with("..."),
            "truncated preview must be prefixed with ...; got {out:?}"
        );
        let tail = out.strip_prefix("...").unwrap_or(&out);
        assert!(
            text.ends_with(tail),
            "tail must be a suffix of the original; got {out:?}"
        );
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
            blocks: None,
            task_ref: None,
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
        assert!(
            out.contains("add a login page"),
            "prior user prompt injected: {out}"
        );
        assert!(
            out.contains("created src/Login.tsx"),
            "prior output injected: {out}"
        );
        assert!(
            out.contains("now add validation"),
            "current request appended: {out}"
        );
        assert!(
            out.contains("Claude Code"),
            "prior agent named in bridge: {out}"
        );
    }

    #[test]
    fn inject_context_caps_total_and_keeps_newest() {
        // 20 turns each producing a big block — total must stay under the cap,
        // and the MOST RECENT turn's content must survive the trim.
        let big = "x".repeat(CONTEXT_BRIDGE_OUTPUT_MAX_CHARS);
        let prior: Vec<_> = (0..20)
            .map(|i| {
                mk_turn(
                    AgentType::ClaudeCode,
                    &format!("turn-{i}"),
                    Some(&format!("{big} marker-{i}")),
                )
            })
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
            OutputMode::StructuredJson,
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
    fn gemini_spawn_uses_stream_json() {
        // gemini 0.18.4 supports `-o stream-json` (verified via `gemini --help`,
        // choices text/json/stream-json). Without it gemini emits plain text — no
        // structured blocks, no tool cards. Lock the flag + StructuredJson mode.
        {
            let mut cache = EXE_CACHE.lock().unwrap();
            cache.insert("gemini".to_string(), std::path::PathBuf::from("gemini"));
        }
        let cfg = build_spawn_config(&AgentType::GeminiCli, "/p", "hello", None)
            .expect("build_spawn_config for GeminiCli");
        assert!(
            cfg.args.contains(&"-o".to_string()) && cfg.args.contains(&"stream-json".to_string()),
            "GeminiCli must emit -o stream-json for structured output: {:?}",
            cfg.args,
        );
        assert_eq!(
            cfg.output_mode,
            OutputMode::StructuredJson,
            "reader must parse gemini stdout as stream-json (parse_gemini_line)",
        );
    }

    #[test]
    fn qwen_spawn_uses_stream_json() {
        // qwen 0.14.3 supports the same `-o stream-json` flag as claude (verified
        // via `qwen --help`). Its schema is Anthropic-style and reuses
        // parse_claude_line, so output_mode is StructuredJson. We deliberately do
        // NOT add --include-partial-messages (would need a delta accumulator).
        {
            let mut cache = EXE_CACHE.lock().unwrap();
            cache.insert("qwen".to_string(), std::path::PathBuf::from("qwen"));
        }
        let cfg = build_spawn_config(&AgentType::QwenCode, "/p", "hello", None)
            .expect("build_spawn_config for QwenCode");
        assert!(
            cfg.args.contains(&"-o".to_string()) && cfg.args.contains(&"stream-json".to_string()),
            "QwenCode must emit -o stream-json for structured output: {:?}",
            cfg.args,
        );
        assert!(
            !cfg.args.contains(&"--include-partial-messages".to_string()),
            "QwenCode must NOT pass --include-partial-messages (per-token deltas need a separate accumulator): {:?}",
            cfg.args,
        );
        assert_eq!(
            cfg.output_mode,
            OutputMode::StructuredJson,
            "reader must parse qwen stdout as stream-json (reuses parse_claude_line)",
        );
    }

    #[test]
    fn parse_claude_line_assistant_text() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello world"}]}}"#;
        assert_eq!(
            parse_claude_line(line),
            vec![ClaudeBlock::Text {
                content: "hello world".to_string()
            }],
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
                ClaudeBlock::Text {
                    content: "reading".to_string()
                },
                ClaudeBlock::ToolUse {
                    id: None,
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
                id: None,
                name: "WebSearch".to_string(),
                input: None,
            }],
        );
    }

    #[test]
    fn parse_claude_line_tool_use_with_id() {
        // claude's tool_use carries an `id` (toolu_...) that the later
        // tool_result's `tool_use_id` points back at. Surfacing it on the block
        // lets the reverse map (chat_event_to_agent_events) pair by id instead
        // of FIFO position (defect ① root cause). Mirrors the symmetric gemini
        // `parse_gemini_*` id assertions; guards against wire schema drift
        // (e.g. id nesting into a sub-field) that None-only tests would miss.
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_abc","name":"Read","input":{"file_path":"src/main.rs"}}]}}"#;
        assert_eq!(
            parse_claude_line(line),
            vec![ClaudeBlock::ToolUse {
                id: Some("toolu_abc".to_string()),
                name: "Read".to_string(),
                input: Some(serde_json::json!({"file_path":"src/main.rs"})),
            }],
        );
    }

    #[test]
    fn parse_claude_line_tool_result() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"42 lines","is_error":false}]}}"#;
        let blocks = parse_claude_line(line);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ClaudeBlock::ToolResult {
                tool_use_id,
                is_error,
                ..
            } => {
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
            vec![ClaudeBlock::Result {
                is_error: false,
                secs: 45
            }],
        );
        let err =
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"duration_ms":1000}"#;
        assert_eq!(
            parse_claude_line(err),
            vec![ClaudeBlock::Result {
                is_error: true,
                secs: 1
            }],
        );
        // subtype != success also counts as failure even without is_error.
        let bad = r#"{"type":"result","subtype":"error_during_execution","is_error":false,"duration_ms":500}"#;
        assert_eq!(
            parse_claude_line(bad),
            vec![ClaudeBlock::Result {
                is_error: true,
                secs: 0
            }],
        );
    }

    #[test]
    fn parse_claude_line_system_and_malformed_are_empty() {
        // system (init / api_retry) carries no user-facing content.
        assert!(
            parse_claude_line(r#"{"type":"system","subtype":"init","session_id":"x"}"#).is_empty()
        );
        // Malformed / non-JSON / empty → empty (must not panic, never break stream).
        assert!(parse_claude_line("not json at all").is_empty());
        assert!(parse_claude_line("").is_empty());
    }

    #[test]
    fn parse_gemini_line_message_text() {
        // gemini's `message.content` is a FLAT STRING (not an array like claude).
        // Assistant text arrives as a message event (gemini uses role:"model"
        // for the assistant turn) and renders as a Text block.
        let line = r#"{"type":"message","timestamp":"2026-06-16T10:26:52.645Z","role":"model","content":"here is the reply"}"#;
        assert_eq!(
            parse_gemini_line(line),
            vec![ClaudeBlock::Text {
                content: "here is the reply".to_string(),
            }],
        );
    }

    #[test]
    fn parse_gemini_line_message_user_echo_skipped() {
        // gemini echoes the user's prompt back as role:"user" with the prompt as
        // content (captured from a real gemini run). Rendering it would
        // duplicate the prompt already shown in the chat input — skip it. The
        // skip keys on role=="user" only, so any other role value (model/
        // assistant/…) still renders.
        let line = r#"{"type":"message","timestamp":"2026-06-16T10:26:52.645Z","role":"user","content":"reply with exactly the two letters: ok"}"#;
        assert!(parse_gemini_line(line).is_empty());
    }

    #[test]
    fn parse_gemini_line_message_empty_content_is_empty() {
        // Empty or missing content → no block (mirrors claude's empty-text skip).
        assert!(
            parse_gemini_line(r#"{"type":"message","role":"assistant","content":""}"#).is_empty()
        );
        assert!(parse_gemini_line(r#"{"type":"message","role":"assistant"}"#).is_empty());
    }

    #[test]
    fn parse_gemini_line_tool_use() {
        // gemini's tool fields are `tool_name` / `parameters` (NOT claude's
        // nested name/input inside message.content).
        let line = r#"{"type":"tool_use","tool_name":"read_file","parameters":{"path":"src/main.rs"},"tool_id":"t1"}"#;
        assert_eq!(
            parse_gemini_line(line),
            vec![ClaudeBlock::ToolUse {
                id: Some("t1".to_string()),
                name: "read_file".to_string(),
                input: Some(serde_json::json!({"path":"src/main.rs"})),
            }],
        );
    }

    #[test]
    fn parse_gemini_line_tool_use_without_parameters() {
        // Missing parameters → input:None (render stays byte-identical to the
        // legacy empty-preview, never "null").
        let line = r#"{"type":"tool_use","tool_name":"list_dir","tool_id":"t2"}"#;
        assert_eq!(
            parse_gemini_line(line),
            vec![ClaudeBlock::ToolUse {
                id: Some("t2".to_string()),
                name: "list_dir".to_string(),
                input: None,
            }],
        );
    }

    #[test]
    fn parse_gemini_line_tool_result_status_map() {
        // gemini `status` is a string ("success"/"error") → maps to Option<bool>.
        // output may be an object on error — content keeps the raw Value.
        let ok = r#"{"type":"tool_result","tool_id":"t1","output":"42 lines","status":"success"}"#;
        let blocks = parse_gemini_line(ok);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ClaudeBlock::ToolResult {
                tool_use_id,
                is_error,
                content,
            } => {
                assert_eq!(tool_use_id.as_deref(), Some("t1"));
                assert_eq!(*is_error, Some(false));
                assert_eq!(content.as_ref().and_then(|c| c.as_str()), Some("42 lines"));
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
        let err =
            r#"{"type":"tool_result","tool_id":"t1","output":{"error":"denied"},"status":"error"}"#;
        let blocks = parse_gemini_line(err);
        match &blocks[0] {
            ClaudeBlock::ToolResult {
                is_error, content, ..
            } => {
                assert_eq!(*is_error, Some(true));
                assert!(content.as_ref().map(|c| c.is_object()).unwrap_or(false));
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn parse_gemini_line_result_uses_nested_stats_duration() {
        // KEY gemini difference: `status` (not `subtype`), no `is_error` field
        // (verdict = status != "success"), and `duration_ms` NESTED under
        // `stats`. Both shapes captured from a real gemini run.
        let err = r#"{"type":"result","timestamp":"2026-06-16T10:26:53.368Z","status":"error","error":{"type":"Error","message":"auth"},"stats":{"total_tokens":0,"input_tokens":0,"output_tokens":0,"duration_ms":0,"tool_calls":0}}"#;
        assert_eq!(
            parse_gemini_line(err),
            vec![ClaudeBlock::Result {
                is_error: true,
                secs: 0
            }],
        );
        let ok = r#"{"type":"result","status":"success","stats":{"duration_ms":45000,"total_tokens":100}}"#;
        assert_eq!(
            parse_gemini_line(ok),
            vec![ClaudeBlock::Result {
                is_error: false,
                secs: 45
            }],
        );
        // status missing → treated as failure (never silently "success").
        let no_status = r#"{"type":"result","stats":{"duration_ms":1000}}"#;
        assert_eq!(
            parse_gemini_line(no_status),
            vec![ClaudeBlock::Result {
                is_error: true,
                secs: 1
            }],
        );
    }

    #[test]
    fn parse_gemini_line_init_error_unknown_are_empty() {
        // init (bootstrap) / error (pre-result API failure — the result event
        // carries the verdict) / malformed / unknown future types → skip, never
        // panic (forward-compat: gemini adding a new event type can't break us).
        assert!(parse_gemini_line(r#"{"type":"init","session_id":"x","model":"auto"}"#).is_empty());
        assert!(parse_gemini_line(r#"{"type":"error","error":{"message":"boom"}}"#).is_empty());
        assert!(parse_gemini_line("not json at all").is_empty());
        assert!(parse_gemini_line("").is_empty());
        assert!(parse_gemini_line(r#"{"type":"plan","content":"some future event"}"#).is_empty());
    }

    #[test]
    fn render_blocks_on_gemini_blocks_matches_claude_contract() {
        // gemini parses into the SAME ClaudeBlock variants, so render_blocks is
        // reused unchanged — verify the golden output for a message + result.
        let msg = r#"{"type":"message","role":"assistant","content":"hello world"}"#;
        assert_eq!(
            render_blocks(&parse_gemini_line(msg)),
            Some("hello world\n".to_string()),
        );
        let ok = r#"{"type":"result","status":"success","stats":{"duration_ms":45000}}"#;
        assert_eq!(
            render_blocks(&parse_gemini_line(ok)),
            Some("\x1b[32m✓ 完成 (45s)\x1b[0m\n".to_string()),
        );
        // Zero blocks (init/malformed) → None.
        assert_eq!(
            render_blocks(&parse_gemini_line(r#"{"type":"init"}"#)),
            None
        );
    }

    #[test]
    fn parse_claude_line_handles_qwen_thinking_block() {
        // qwen reuses parse_claude_line (Anthropic-style schema). Its assistant
        // turns can carry a `thinking` block + extra `uuid` field — thinking is
        // skipped (only text/tool_use are parsed), uuid is ignored. Only the
        // text block survives.
        let line = r#"{"type":"assistant","uuid":"u1","message":{"content":[{"type":"thinking","thinking":"reasoning here"},{"type":"text","text":"the answer"}]}}"#;
        assert_eq!(
            parse_claude_line(line),
            vec![ClaudeBlock::Text {
                content: "the answer".to_string()
            }],
        );
    }

    #[test]
    fn parse_claude_line_handles_qwen_result_with_extra_fields() {
        // The REAL qwen 0.14.3 result event (captured): same type/subtype/
        // is_error/duration_ms fields as claude, PLUS uuid/num_turns/usage/
        // duration_api_ms/permission_denials — all ignored by parse_claude_line.
        // subtype != "success" + is_error:true → failure. Verifies qwen reuses
        // the claude parser end-to-end with zero changes.
        let line = r#"{"type":"result","subtype":"error_during_execution","uuid":"7cec5b38","session_id":"8421a91e","is_error":true,"duration_ms":0,"duration_api_ms":0,"num_turns":0,"usage":{"input_tokens":0,"output_tokens":0},"permission_denials":[],"error":{"message":"No auth type is selected."}}"#;
        assert_eq!(
            parse_claude_line(line),
            vec![ClaudeBlock::Result {
                is_error: true,
                secs: 0
            }],
        );
    }

    // ---- A1: extract_pty_usage (CLI result → cost_records seam) ----

    #[test]
    fn extract_pty_usage_claude_stats_with_cost() {
        // claude result: tokens nested under `stats`, USD cost top-level.
        let line = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":12000,"stats":{"total_tokens":1500,"input_tokens":1000,"output_tokens":500},"total_cost_usd":0.0123,"session_id":"abc"}"#;
        assert_eq!(
            extract_pty_usage(line),
            Some(PtyUsage {
                input_tokens: 1000,
                output_tokens: 500,
                cost_usd: Some(0.0123),
            }),
        );
    }

    #[test]
    fn extract_pty_usage_qwen_top_level_usage_no_cost() {
        // qwen result: tokens under top-level `usage`; never reports cost.
        let line = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":8000,"usage":{"input_tokens":2200,"output_tokens":300}}"#;
        assert_eq!(
            extract_pty_usage(line),
            Some(PtyUsage {
                input_tokens: 2200,
                output_tokens: 300,
                cost_usd: None,
            }),
        );
    }

    #[test]
    fn extract_pty_usage_gemini_nested_stats_no_cost() {
        // gemini result: tokens nested under `stats` (same path as claude), no
        // cost field. Verifies the stats-first lookup covers gemini too.
        let line = r#"{"type":"result","status":"success","stats":{"duration_ms":45000,"total_tokens":900,"input_tokens":700,"output_tokens":200}}"#;
        assert_eq!(
            extract_pty_usage(line),
            Some(PtyUsage {
                input_tokens: 700,
                output_tokens: 200,
                cost_usd: None,
            }),
        );
    }

    #[test]
    fn extract_pty_usage_non_result_and_malformed_return_none() {
        // assistant / system / user lines and malformed JSON carry no usage.
        assert_eq!(
            extract_pty_usage(r#"{"type":"assistant","message":{"content":[]}}"#),
            None
        );
        assert_eq!(
            extract_pty_usage(r#"{"type":"system","subtype":"init"}"#),
            None
        );
        assert_eq!(extract_pty_usage("not json"), None);
        assert_eq!(extract_pty_usage(""), None);
    }

    #[test]
    fn extract_pty_usage_result_without_token_fields_returns_none() {
        // A result with neither stats nor usage → nothing bookable → None.
        let line = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":100}"#;
        assert_eq!(extract_pty_usage(line), None);
    }

    #[test]
    fn extract_pty_usage_all_zero_error_result_is_some_but_filtered_upstream() {
        // The auth-failure error result carries explicit 0/0 tokens. extract
        // returns Some(0,0,None) — honest about what the CLI reported — and the
        // call site's `> 0` guard drops it so it never books a phantom row.
        // (cline claude-code.ts:177: input_tokens recorded verbatim incl. cache.)
        let line = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"duration_ms":0,"usage":{"input_tokens":0,"output_tokens":0}}"#;
        let usage = extract_pty_usage(line).expect("explicit zeros are still Some");
        assert_eq!(
            usage,
            PtyUsage {
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: None
            }
        );
        // The guard the reader applies:
        let bookable = usage.input_tokens > 0
            || usage.output_tokens > 0
            || usage.cost_usd.unwrap_or(0.0) > 0.0;
        assert!(!bookable, "all-zero result must not book a cost row");
    }

    #[test]
    fn render_blocks_is_byte_identical_to_legacy_output() {
        // Golden snapshots: render_blocks(parse(line)) must equal the exact
        // ANSI text the old single-pass renderer produced. Locks the render
        // contract so the terminal replay and {sid}.log stay byte-identical.
        let assistant =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello world"}]}}"#;
        assert_eq!(
            render_blocks(&parse_claude_line(assistant)),
            Some("hello world\n".to_string()),
        );
        let tool = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"src/main.rs"}}]}}"#;
        let rendered = render_blocks(&parse_claude_line(tool)).expect("tool_use renders");
        assert_eq!(
            rendered,
            "\x1b[36m🔧 Read \x1b[90m{\"file_path\":\"src/main.rs\"}\x1b[0m\n"
        );
        let ok = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":45000}"#;
        assert_eq!(
            render_blocks(&parse_claude_line(ok)),
            Some("\x1b[32m✓ 完成 (45s)\x1b[0m\n".to_string()),
        );
        let err =
            r#"{"type":"result","subtype":"error_max_turns","is_error":true,"duration_ms":1000}"#;
        assert_eq!(
            render_blocks(&parse_claude_line(err)),
            Some("\x1b[31m✗ 失败 (1s)\x1b[0m\n".to_string()),
        );
        // Zero blocks (system / malformed) → None, not an empty string.
        assert_eq!(
            render_blocks(&parse_claude_line(r#"{"type":"system","subtype":"init"}"#)),
            None
        );
        assert_eq!(render_blocks(&parse_claude_line("not json")), None);
    }

    #[test]
    fn chat_stream_event_serializes_with_kind_tag() {
        // The wire schema must carry `kind` as the discriminator tag so the TS
        // union narrows on it. Verify each variant's serialized shape.
        let text = ChatStreamEvent::Text {
            content: "hi".to_string(),
        };
        let v = serde_json::to_value(&text).unwrap();
        assert_eq!(v["kind"], "text");
        assert_eq!(v["content"], "hi");

        let tool = ChatStreamEvent::ToolUse {
            id: None,
            name: "Read".to_string(),
            input: serde_json::json!({"file_path":"a.rs"}),
        };
        let v = serde_json::to_value(&tool).unwrap();
        assert_eq!(v["kind"], "tool_use");
        assert_eq!(v["name"], "Read");
        assert_eq!(v["input"]["file_path"], "a.rs");

        let res_ok = ChatStreamEvent::Result {
            is_error: false,
            secs: 12,
        };
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
        assert_eq!(
            evs[0],
            ChatStreamEvent::Text {
                content: "hello".to_string()
            }
        );

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
        assert_eq!(
            evs,
            vec![ChatStreamEvent::Result {
                is_error: false,
                secs: 3
            }]
        );

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
        let seq: Vec<ChatStreamEvent> = lines
            .iter()
            .flat_map(|l| claude_line_to_events(l))
            .collect();
        let kinds: Vec<&str> = seq
            .iter()
            .map(|e| match e {
                ChatStreamEvent::Text { .. } => "text",
                ChatStreamEvent::Thinking { .. } => "thinking",
                ChatStreamEvent::ToolUse { .. } => "tool_use",
                ChatStreamEvent::ToolResult { .. } => "tool_result",
                ChatStreamEvent::Result { .. } => "result",
                ChatStreamEvent::FileChanged { .. } => "file_changed",
                ChatStreamEvent::Compact { .. } => "compact",
                ChatStreamEvent::ApprovalRequired { .. } => "approval_required",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["text", "tool_use", "tool_result", "text", "result"]
        );
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
