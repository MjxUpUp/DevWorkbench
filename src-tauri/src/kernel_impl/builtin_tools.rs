//! Built-in coding tools for the kernel agent — read/search/exec/write.
//!
//! The self-hosted ReactAgent used to ship with ONLY McpTool + SubAgentTool +
//! SkillTool, so it could not read a file, search the repo, run a command, or
//! write anything — it told users "I only have dispatch_subagent". This module
//! fills that gap with the minimal coding toolset every agent needs.
//!
//! Each tool implements `kernel_core::Tool` the same way McpTool/SkillTool do:
//! `info()` declares a JSON-schema to the model, `invoke()` parses the JSON
//! argument string and returns text fed back as a tool message. The run loop
//! (react_agent.rs) + UI mapping (react_chat.rs) are Tool-agnostic, so these
//! automatically get tool_use/tool_result cards end-to-end — no other wiring.
//!
//! Safety:
//! - read_file/glob/grep are `is_read_only = true` → they enter the sub-agent's
//!   read-only subset (investigation capability sinks to children) and run for
//!   real even in dry-run mode.
//! - bash/write_file are NOT read-only (`is_dangerous = true`) → excluded from
//!   the sub-agent subset (a child can't mutate), and simulated in dry-run.
//! - bash is named `bash` with a `command` arg key, so the existing
//!   CommandGuardHook (hooks.rs) auto-classifies dangerous commands (rm -rf /,
//!   fork bombs, shutdown) with zero adapter code.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use kernel_core::{Error, Tool, ToolContext, ToolInfo};
use serde_json::{json, Value};

/// Parse the model-supplied argument JSON. Empty string → empty object (the
/// streaming tool-call protocol sometimes sends `""` for no-arg calls).
fn parse_args(arguments: &str) -> Result<Value, Error> {
    if arguments.is_empty() {
        Ok(json!({}))
    } else {
        serde_json::from_str(arguments)
            .map_err(|e| Error::Tool(format!("bad args json: {e}")))
    }
}

/// Resolve a possibly-relative path against the tool's working directory.
/// Absolute paths pass through; relative ones join onto `working_dir` (the
/// project root the agent operates in, set by build_react_agent).
fn resolve_path(working_dir: &Option<String>, file_path: &str) -> PathBuf {
    let p = PathBuf::from(file_path);
    if p.is_absolute() {
        p
    } else {
        working_dir
            .as_ref()
            .map(|d| Path::new(d).join(&p))
            .unwrap_or(p)
    }
}

/// Pull a required string field off the args object.
fn req_str(args: &Value, key: &str) -> Result<String, Error> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| Error::Tool(format!("missing required argument '{key}'")))
}

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "read_file".into(),
            description: "读取文本文件内容（按行，带行号）。参数 {file_path: 相对/绝对路径, offset?: 起始行(1-based), limit?: 读取行数}。用于查看源码、配置、日志。".into(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "要读取的文件路径" },
                    "offset": { "type": "number", "description": "起始行号(1-based)，默认1" },
                    "limit": { "type": "number", "description": "读取的行数，默认到文件末尾" }
                },
                "required": ["file_path"]
            }),
        }
    }

    async fn invoke(&self, arguments: &str, ctx: &ToolContext) -> Result<String, Error> {
        let args = parse_args(arguments)?;
        let file_path = req_str(&args, "file_path")?;
        let offset = args.get("offset").and_then(|v| v.as_u64()).map(|n| n as usize);
        let limit = args.get("limit").and_then(|v| v.as_u64()).map(|n| n as usize);
        let path = resolve_path(&ctx.working_dir, &file_path);

        let content =
            std::fs::read_to_string(&path).map_err(|e| Error::Tool(format!("read {}: {e}", path.display())))?;

        // Number lines (1-based) so the model can reference them. Claude Code
        // convention: offset is the first line to show, limit caps the count.
        let lines: Vec<&str> = content.lines().collect();
        const MAX_LINES: usize = 2000;
        let start = offset.unwrap_or(1).saturating_sub(1).min(lines.len());
        let end = match limit {
            Some(n) => (start + n).min(lines.len()),
            None => lines.len(),
        };
        let end = end.min(start + MAX_LINES);
        let mut out = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            out.push_str(&format!("{}\t{}\n", start + i + 1, line));
        }
        if lines.len() > end {
            out.push_str(&format!("\n... ({} more lines, pass a larger offset)\n", lines.len() - end));
        }
        if out.is_empty() {
            return Ok("(empty file)".into());
        }
        Ok(out)
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// glob
// ---------------------------------------------------------------------------

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "glob".into(),
            description: "按通配模式查找文件路径(* 匹配任意字符含/, ** 等价于 *, ? 匹配单字符)。参数 {pattern: 如 '**/*.rs' 或 'src/*.ts', path?: 搜索根目录，默认工作目录}。尊重 .gitignore。".into(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "通配模式" },
                    "path": { "type": "string", "description": "搜索根目录，默认工作目录" }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn invoke(&self, arguments: &str, ctx: &ToolContext) -> Result<String, Error> {
        let args = parse_args(arguments)?;
        let pattern = req_str(&args, "pattern")?;
        let root = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                ctx.working_dir
                    .as_ref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."))
            });
        if !root.is_dir() {
            return Err(Error::Tool(format!("glob root not a directory: {}", root.display())));
        }

        // ignore::WalkBuilder respects .gitignore + standard ignores, matching
        // what a developer expects "search the repo" to mean.
        let mut matches = Vec::new();
        for entry in ignore::WalkBuilder::new(&root)
            .hidden(false)
            .build()
            .flatten()
        {
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(&root)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .replace('\\', "/");
            if glob_match(&pattern, &rel) {
                matches.push(rel);
            }
        }
        matches.sort();
        const MAX: usize = 200;
        let truncated = matches.len() > MAX;
        if truncated {
            matches.truncate(MAX);
        }
        if matches.is_empty() {
            return Ok("(no matches)".into());
        }
        let mut out = matches.join("\n");
        if truncated {
            out.push_str("\n... (truncated, refine the pattern)");
        }
        Ok(out)
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// grep
// ---------------------------------------------------------------------------

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "grep".into(),
            description: "在文件中搜索文本(子串匹配，按行)。参数 {pattern: 搜索串, path?: 搜索根目录默认工作目录, glob?: 限定文件模式如 '*.rs'}。返回 file:lineno:line。".into(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "要搜索的文本" },
                    "path": { "type": "string", "description": "搜索根目录，默认工作目录" },
                    "glob": { "type": "string", "description": "限定文件通配模式，如 '*.rs'" }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn invoke(&self, arguments: &str, ctx: &ToolContext) -> Result<String, Error> {
        let args = parse_args(arguments)?;
        let pattern = req_str(&args, "pattern")?;
        let glob_filter = args.get("glob").and_then(|v| v.as_str()).map(str::to_owned);
        let root = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                ctx.working_dir
                    .as_ref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."))
            });
        if !root.is_dir() {
            return Err(Error::Tool(format!("grep root not a directory: {}", root.display())));
        }

        let mut hits: Vec<String> = Vec::new();
        for entry in ignore::WalkBuilder::new(&root)
            .hidden(false)
            .build()
            .flatten()
        {
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(&root)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .replace('\\', "/");
            if let Some(ref g) = glob_filter {
                if !glob_match(g, &rel) {
                    continue;
                }
            }
            // Skip binary/large files — read_to_string would error or waste RAM.
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            for (i, line) in content.lines().enumerate() {
                if line.contains(pattern.as_str()) {
                    hits.push(format!("{rel}:{}:{}", i + 1, line));
                }
            }
        }
        const MAX: usize = 100;
        let truncated = hits.len() > MAX;
        if truncated {
            hits.truncate(MAX);
        }
        if hits.is_empty() {
            return Ok("(no matches)".into());
        }
        let mut out = hits.join("\n");
        if truncated {
            out.push_str(&format!("\n... ({} more matches, narrow the pattern)", hits.len()));
        }
        Ok(out)
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// bash
// ---------------------------------------------------------------------------

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            // Named `bash` + arg key `command` so CommandGuardHook (hooks.rs)
            // auto-classifies dangerous commands (rm -rf /, fork bombs) — zero
            // adapter code on the hook side.
            name: "bash".into(),
            description: "运行 shell 命令(Windows: cmd /C, Unix: sh -c)，30秒超时。参数 {command: 命令字符串}。危险命令会被 CommandGuard 拦截。返回 stdout/stderr/exit code。".into(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "要执行的 shell 命令" }
                },
                "required": ["command"]
            }),
        }
    }

    async fn invoke(&self, arguments: &str, ctx: &ToolContext) -> Result<String, Error> {
        let args = parse_args(arguments)?;
        let command = req_str(&args, "command")?;

        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = tokio::process::Command::new("cmd");
            c.arg("/C").arg(&command);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.arg("-c").arg(&command);
            c
        };
        // CREATE_NO_WINDOW — react_kernel 的 bash 工具每调一次都 spawn 一个
        // cmd/sh 子进程；不隐藏窗口的话每跑一条命令就弹一次控制台黑框（用户
        // 观察到的"agent 运行中终端不断闪现"）。与 honesty.rs/git.rs/pty.rs 等
        // 所有子进程 spawn 一致。tokio::process::Command 同样 impl CommandExt。
        #[cfg(target_os = "windows")]
        {
            // tokio::process::Command exposes creation_flags as an INHERENT
            // method on Windows (unlike std::process::Command, which needs the
            // CommandExt trait import — see honesty.rs). No trait import here.
            cmd.creation_flags(0x0800_0000);
        }
        if let Some(dir) = &ctx.working_dir {
            cmd.current_dir(dir);
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = cmd
            .spawn()
            .map_err(|e| Error::Tool(format!("spawn command: {e}")))?;
        // Bound execution — a hung command must not pin the agent's turn.
        match tokio::time::timeout(Duration::from_secs(30), child.wait_with_output()).await {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                let code = out.status.code().unwrap_or(-1);
                Ok(format!("[exit {code}]\n{stdout}\n--- stderr ---\n{stderr}"))
            }
            Ok(Err(e)) => Err(Error::Tool(format!("command wait: {e}"))),
            Err(_) => Err(Error::Tool("command timed out (30s)".into())),
        }
    }

    fn is_dangerous(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// write_file
// ---------------------------------------------------------------------------

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "write_file".into(),
            description: "写入文本文件(覆盖)。参数 {file_path: 路径, content: 内容}。自动创建父目录。用于创建/修改源码、配置。".into(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "要写入的文件路径" },
                    "content": { "type": "string", "description": "文件完整内容" }
                },
                "required": ["file_path", "content"]
            }),
        }
    }

    async fn invoke(&self, arguments: &str, ctx: &ToolContext) -> Result<String, Error> {
        let args = parse_args(arguments)?;
        let file_path = req_str(&args, "file_path")?;
        let content = req_str(&args, "content")?;
        let path = resolve_path(&ctx.working_dir, &file_path);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Tool(format!("create dirs {}: {e}", parent.display())))?;
        }
        std::fs::write(&path, &content)
            .map_err(|e| Error::Tool(format!("write {}: {e}", path.display())))?;
        Ok(format!("wrote {} bytes to {}", content.len(), path.display()))
    }

    fn is_dangerous(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Minimal wildcard matcher (no regex/globset dep — keeps the toolset offline-safe)
// ---------------------------------------------------------------------------

/// Classic backtracking wildcard match: `*` matches any run of chars (incl. `/`),
/// `?` matches one char, `**` collapses to `*`. Matches the WHOLE string.
pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    glob_match_bytes(pattern.as_bytes(), text.as_bytes())
}

fn glob_match_bytes(pattern: &[u8], text: &[u8]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star_pi: Option<usize> = None;
    let mut star_ti = 0usize;
    while ti < text.len() {
        if pi < pattern.len() && (pattern[pi] == text[ti] || pattern[pi] == b'?') {
            pi += 1;
            ti += 1;
        } else if pi < pattern.len() && pattern[pi] == b'*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(spi) = star_pi {
            pi = spi + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }
    pi == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_core::ToolContext;

    fn ctx(dir: &Path) -> ToolContext {
        ToolContext {
            working_dir: Some(dir.to_string_lossy().into_owned()),
            conversation_id: None,
        }
    }

    #[test]
    fn glob_match_basics() {
        assert!(glob_match("*.rs", "a.rs"));
        assert!(glob_match("*.rs", "src/a.rs")); // * spans /
        assert!(!glob_match("*.rs", "a.ts"));
        assert!(glob_match("**/*.rs", "src/deep/a.rs"));
        assert!(glob_match("src/*.ts", "src/x.ts"));
        assert!(!glob_match("src/*.ts", "other/x.ts"));
        assert!(glob_match("a?c", "abc"));
        assert!(glob_match("*", "anything/here"));
    }

    #[tokio::test]
    async fn read_file_returns_numbered_lines() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("note.txt");
        std::fs::write(&p, "alpha\nbeta\ngamma\n").unwrap();
        let out = ReadFileTool.invoke(r#"{"file_path":"note.txt"}"#, &ctx(dir.path())).await.unwrap();
        assert!(out.contains("1\talpha"));
        assert!(out.contains("3\tgamma"));
    }

    #[tokio::test]
    async fn read_file_offset_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("lines.txt");
        std::fs::write(&p, "one\ntwo\nthree\nfour\n").unwrap();
        let out = ReadFileTool
            .invoke(r#"{"file_path":"lines.txt","offset":2,"limit":1}"#, &ctx(dir.path()))
            .await
            .unwrap();
        assert!(out.contains("2\ttwo"));
        assert!(!out.contains("three"));
    }

    #[tokio::test]
    async fn read_file_missing_errors() {
        let dir = tempfile::tempdir().unwrap();
        let r = ReadFileTool.invoke(r#"{"file_path":"nope.txt"}"#, &ctx(dir.path())).await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn glob_finds_files_by_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::fs::write(dir.path().join("b.ts"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("c.rs"), "").unwrap();
        let out = GlobTool.invoke(r#"{"pattern":"*.rs"}"#, &ctx(dir.path())).await.unwrap();
        assert!(out.contains("a.rs"));
        assert!(out.contains("sub/c.rs"));
        assert!(!out.contains("b.ts"));
    }

    #[tokio::test]
    async fn grep_finds_matching_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello world\nbye\n").unwrap();
        let out = GrepTool.invoke(r#"{"pattern":"hello"}"#, &ctx(dir.path())).await.unwrap();
        assert!(out.contains("a.txt:1:hello world"));
        // no false positive
        let miss = GrepTool.invoke(r#"{"pattern":"zzz"}"#, &ctx(dir.path())).await.unwrap();
        assert_eq!(miss, "(no matches)");
    }

    #[tokio::test]
    async fn write_file_creates_and_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let out = WriteFileTool
            .invoke(r#"{"file_path":"nested/x.txt","content":"hi"}"#, &ctx(dir.path()))
            .await
            .unwrap();
        assert!(out.contains("wrote 2 bytes"));
        assert_eq!(std::fs::read_to_string(dir.path().join("nested").join("x.txt")).unwrap(), "hi");
    }

    #[tokio::test]
    async fn bash_runs_echo() {
        // Cross-platform: cmd /C echo hi (win) / sh -c echo hi (unix) both print hi.
        // Also implicitly covers the Windows CREATE_NO_WINDOW flag on BashTool's
        // Command: on Windows this test compiles WITH creation_flags(0x0800_0000)
        // and still returns stdout normally — so the flag hides the console popup
        // (the "agent 运行中终端不断闪现" symptom) WITHOUT breaking spawn/output.
        // The popup itself is a GUI/OS behavior no unit test can assert; this echo
        // round-trip is the closest executable guarantee that the flag didn't
        // regress the command path.
        let dir = tempfile::tempdir().unwrap();
        let out = BashTool.invoke(r#"{"command":"echo agent_smoke"}"#, &ctx(dir.path())).await.unwrap();
        assert!(out.contains("agent_smoke"), "got: {out}");
    }

    #[test]
    fn resolve_path_relative_joins_working_dir() {
        let p = resolve_path(&Some("/proj".into()), "src/a.rs");
        assert_eq!(p, PathBuf::from("/proj/src/a.rs"));
    }

    #[test]
    fn resolve_path_absolute_passes_through() {
        let p = resolve_path(&Some("/proj".into()), "/abs/a.rs");
        assert_eq!(p, PathBuf::from("/abs/a.rs"));
    }

    #[test]
    fn read_only_and_dangerous_flags_match_safety_contract() {
        // Read-only tools enter the sub-agent's read_only_subset and run for
        // real in dry-run mode. Mutators are excluded from the child subset
        // (a sub-agent can't mutate) and simulated in dry-run. build_react_agent
        // itself isn't unit-testable (dirs_home/providers/MCP deps), so we pin
        // the flags that drive the registry's subset + dry-run branching here.
        assert!(ReadFileTool.is_read_only());
        assert!(GlobTool.is_read_only());
        assert!(GrepTool.is_read_only());
        assert!(!BashTool.is_read_only());
        assert!(BashTool.is_dangerous());
        assert!(!WriteFileTool.is_read_only());
        assert!(WriteFileTool.is_dangerous());
    }

    #[test]
    fn tool_names_are_stable_and_distinct() {
        // The run loop dispatches tool_calls by info().name; these must stay
        // stable (the model learns them from the schema) and not collide with
        // the skill__/mcp__/dispatch_subagent prefixes.
        let names = [
            ReadFileTool.info().name,
            GlobTool.info().name,
            GrepTool.info().name,
            BashTool.info().name,
            WriteFileTool.info().name,
        ];
        assert_eq!(names, ["read_file", "glob", "grep", "bash", "write_file"]);
    }
}
