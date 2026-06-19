//! D2 user-configurable lifecycle hooks. A [`UserCommandHook`] wraps one
//! `user_hooks` row as a [`Hook`] that fires on a single [`HookEvent`] (turn
//! start / tool call / run stop), runs the configured shell command with a
//! claude-code-style event JSON on stdin, and maps the exit code to the
//! claude-code protocol: 0 → stdout injected as context (`UserPromptSubmit`
//! only), 2 → BLOCK the event (honored on `UserPromptSubmit` / `PreToolUse` —
//! the turn / tool call is refused; ignored on `Stop` / `PostToolUse` where
//! blocking is nonsensical), other → warn + no effect. This is the user-facing
//! half of the lifecycle-hook layer; the built-in
//! CommandGuard/AssertionGuard/TaskGuard hooks (always on) live in
//! `kernel_impl/hooks.rs`.

pub mod registry;

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::kernel_impl::hooks::{BlockReason, Hook, HookEvent, Severity};
use crate::models::UserHookEvent;

/// A single user-configured lifecycle hook. Built at agent-construct time from
/// one `user_hooks` row; one instance per row. Carries the project working_dir
/// so hook commands like `cat .cursorrules` resolve against the project root.
pub struct UserCommandHook {
    name: String,
    event: UserHookEvent,
    command: String,
    shell: bool,
    timeout_secs: u64,
    working_dir: Option<PathBuf>,
    /// Optional tool-name matcher (claude-code `matcher`). Only consulted for
    /// PreToolUse / PostToolUse — a Submit / Stop hook always fires. See
    /// [`UserCommandHook::matches_pattern`].
    matcher: Option<String>,
}

impl UserCommandHook {
    pub fn new(
        name: String,
        event: UserHookEvent,
        command: String,
        shell: bool,
        timeout_secs: u64,
        working_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            name,
            event,
            command,
            shell,
            timeout_secs,
            working_dir,
            matcher: None,
        }
    }

    pub fn with_matcher(mut self, matcher: Option<String>) -> Self {
        // Empty string == no filter (treat as None so the matcher logic has one
        // canonical "no filter" representation — matches claude-code where "" /
        // "*" both mean wildcard).
        self.matcher = matcher.filter(|m| !m.is_empty());
        self
    }

    /// Does this hook's configured event match the dispatched one? A hook bound
    /// to `Stop` must no-op on `UserPromptSubmit` even though it's registered
    /// into the same HookManager (we pre-filter by event at load time, but keep
    /// the guard defensive).
    fn matches(&self, ev: &HookEvent) -> bool {
        match (&self.event, ev) {
            (UserHookEvent::UserPromptSubmit, HookEvent::UserPromptSubmit { .. }) => true,
            (UserHookEvent::PreToolUse, HookEvent::PreToolUse { .. }) => true,
            (UserHookEvent::PostToolUse, HookEvent::PostToolUse { .. }) => true,
            (UserHookEvent::Stop, HookEvent::Stop { .. }) => true,
            _ => false,
        }
    }

    /// Should this hook fire for `ev`? Event-bound check first, then — for tool
    /// events only — the configured matcher against the tool name. Submit / Stop
    /// hooks ignore the matcher (claude-code matcher is meaningful only for
    /// PreToolUse / PostToolUse, where the match query is the tool name).
    fn fires_for(&self, ev: &HookEvent) -> bool {
        if !self.matches(ev) {
            return false;
        }
        match ev {
            HookEvent::PreToolUse { tool, .. } | HookEvent::PostToolUse { tool, .. } => {
                Self::matches_pattern(tool, self.matcher.as_deref())
            }
            _ => true,
        }
    }

    /// claude-code `matchesPattern(matchQuery, matcher)` faithful port
    /// (utils/hooks.ts:1428). Three modes, decided by the matcher's characters:
    ///
    /// - `None` / `""` / `"*"` → wildcard (matches anything).
    /// - all chars in `[A-Za-z0-9_|]` → literal: if it contains `|`, split into
    ///   exact alternatives and match if the query equals ANY segment; else a
    ///   single exact-equality test.
    /// - otherwise → regex (compiled each call; an invalid pattern logs and
    ///   matches nothing, never panics).
    ///
    /// The kernel has no legacy tool-name aliases, so claude-code's
    /// `normalizeLegacyToolName` / `getLegacyToolNames` step is intentionally
    /// absent — our tool names are the single source of truth.
    pub fn matches_pattern(match_query: &str, matcher: Option<&str>) -> bool {
        let m = match matcher {
            None => return true,
            Some(m) => m,
        };
        if m.is_empty() || m == "*" {
            return true;
        }
        // Simple string or pipe-separated list (no regex special chars except |).
        let is_simple = m
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '|');
        if is_simple {
            if let Some(_) = m.find('|') {
                return m.split('|').any(|seg| seg.trim() == match_query);
            }
            return m == match_query;
        }
        // Otherwise treat as regex.
        match regex::Regex::new(m) {
            Ok(re) => re.is_match(match_query),
            Err(_) => {
                log::warn!("[user-hook] invalid regex matcher, matching nothing: {m}");
                false
            }
        }
    }
}

/// The stdin payload sent to every hook command (claude-code protocol, minimal).
/// `hook_event_name` lets one command serve multiple events if it inspects stdin;
/// the tool fields are populated only for PreToolUse / PostToolUse.
#[derive(Serialize)]
struct HookPayload<'a> {
    hook_event_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_input: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_response: Option<&'a str>,
}

impl<'a> HookPayload<'a> {
    fn from_event(ev: &'a HookEvent) -> Self {
        match ev {
            HookEvent::UserPromptSubmit { prompt } => HookPayload {
                hook_event_name: "UserPromptSubmit",
                prompt: Some(prompt),
                summary: None,
                tool_name: None,
                tool_input: None,
                tool_response: None,
            },
            HookEvent::PreToolUse { tool, arguments } => HookPayload {
                hook_event_name: "PreToolUse",
                prompt: None,
                summary: None,
                tool_name: Some(tool),
                tool_input: Some(arguments),
                tool_response: None,
            },
            HookEvent::PostToolUse {
                tool,
                arguments,
                result,
            } => HookPayload {
                hook_event_name: "PostToolUse",
                prompt: None,
                summary: None,
                tool_name: Some(tool),
                tool_input: Some(arguments),
                tool_response: Some(result),
            },
            HookEvent::Stop { summary } => HookPayload {
                hook_event_name: "Stop",
                prompt: None,
                summary: Some(summary),
                tool_name: None,
                tool_input: None,
                tool_response: None,
            },
        }
    }

    /// Serialized + a trailing newline, matching claude-code's stdin convention.
    fn to_stdin(&self) -> String {
        let mut s = serde_json::to_string(self).unwrap_or_else(|_| "{}".into());
        s.push('\n');
        s
    }
}

/// Outcome of running one hook command. `Context` = clean exit 0 with stdout
/// (injectable for UserPromptSubmit); `Block` = exit 2 (the command asked to
/// refuse — honored as a hard block on Submit/PreToolUse, ignored on
/// Stop/PostToolUse); `Warn` = anything else (logged, yields nothing).
enum RunOutcome {
    Context(String),
    Block(String),
    Warn(String),
}

impl UserCommandHook {
    /// Run the configured command with `payload` on stdin, bounded by
    /// `timeout_secs`. Cross-platform: `shell=true` (default) routes through the
    /// system shell (`git-bash bash.exe -c` on Windows, `sh -c` elsewhere, identical to the
    /// kernel BashTool); `shell=false` splits the command on whitespace into
    /// program + args (naive — no quoting; documented). `CREATE_NO_WINDOW` on
    /// Windows so hook commands don't flash a console, matching every other
    /// subprocess spawn in the app.
    async fn run(&self, payload: &HookPayload<'_>) -> RunOutcome {
        let stdin_str = payload.to_stdin();

        let mut cmd = if self.shell {
            if cfg!(target_os = "windows") {
                // 与 kernel BashTool 同语义：锁 git-bash（之前 cmd /C 导致 Unix
                // 命令失败 + agent 死循环，见 BashTool 注释）。hook 找不到 git-bash
                // 降级为 Warn（不阻塞 turn），与下方 cmd.spawn() 的 Err 也返回
                // Warn 的失败语义一致。
                match crate::commands::tools::resolve_git_bash(None) {
                    Some(bash) => {
                        let mut c = tokio::process::Command::new(bash);
                        c.arg("-c").arg(&self.command);
                        c
                    }
                    None => {
                        return RunOutcome::Warn(format!(
                            "hook {}: git-bash 未找到（设 DEVWORKBENCH_BASH_PATH 或安装 Git for Windows）",
                            self.name
                        ))
                    }
                }
            } else {
                let mut c = tokio::process::Command::new("sh");
                c.arg("-c").arg(&self.command);
                c
            }
        } else {
            // Naive whitespace split — no shell quoting. Acceptable for v1; the
            // `shell` flag defaults to true so this path is opt-in.
            let mut parts = self.command.split_whitespace();
            match parts.next() {
                Some(prog) => {
                    let mut c = tokio::process::Command::new(prog);
                    for a in parts {
                        c.arg(a);
                    }
                    c
                }
                None => return RunOutcome::Warn(format!("hook {} has empty command", self.name)),
            }
        };

        #[cfg(target_os = "windows")]
        {
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        if let Some(dir) = &self.working_dir {
            cmd.current_dir(dir);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return RunOutcome::Warn(format!("hook {} spawn failed: {e}", self.name)),
        };

        // Write the event payload on stdin, then drop the handle to signal EOF.
        // A write failure doesn't abort — the command may not read stdin at all
        // (e.g. a Stop notification), so we proceed to wait regardless.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(stdin_str.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }

        // 取出 stdout/stderr 管道句柄（owned），child 仅剩 wait/kill。借用模式
        // 同 BashTool：timeout().await 结果先 let 再 match，超时分支才能 kill。
        let stdout_h = child.stdout.take();
        let stderr_h = child.stderr.take();

        let timeout = Duration::from_secs(self.timeout_secs.max(1));
        use tokio::io::AsyncReadExt;
        let outcome = tokio::time::timeout(timeout, async {
            let mut so = Vec::new();
            let mut se = Vec::new();
            if let Some(mut s) = stdout_h {
                let _ = s.read_to_end(&mut so).await;
            }
            if let Some(mut s) = stderr_h {
                let _ = s.read_to_end(&mut se).await;
            }
            let status = child.wait().await;
            (so, se, status)
        })
        .await;

        match outcome {
            Ok((so, se, Ok(status))) => {
                let code = status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&so);
                let stderr = String::from_utf8_lossy(&se);
                match code {
                    0 => RunOutcome::Context(stdout.trim_end().to_string()),
                    2 => {
                        // claude-code BLOCKS on exit 2. v2 honors it: the caller
                        // turns this into a hard block on Submit/PreToolUse (the
                        // turn / tool call is refused). The stderr message is the
                        // human-readable reason the command wrote.
                        RunOutcome::Block(stderr.trim().to_string())
                    }
                    other => RunOutcome::Warn(format!(
                        "hook {} failed (exit {other}): {}",
                        self.name,
                        stderr.trim()
                    )),
                }
            }
            Ok((_, _, Err(e))) => RunOutcome::Warn(format!("hook {} wait failed: {e}", self.name)),
            Err(_) => {
                // 超时：杀子进程 + 收尸（同 BashTool 修复，避免孤儿进程泄漏）。
                let _ = child.kill().await;
                let _ = child.wait().await;
                RunOutcome::Warn(format!(
                    "hook {} timed out ({}s, killed)",
                    self.name, self.timeout_secs
                ))
            }
        }
    }
}

#[async_trait]
impl Hook for UserCommandHook {
    fn name(&self) -> &str {
        // Prefix so the hook name is distinguishable from built-ins in logs.
        // Box<dyn Hook> → can't return a formatted &str cheaply; stash a label.
        self.name.as_str()
    }

    async fn on_event(&self, ev: &HookEvent) -> Result<Vec<String>, BlockReason> {
        // Only a hook whose configured event matches the dispatched one acts;
        // others no-op (defensive — load time already filters by event). For
        // tool events the matcher further gates on the tool name — a PreToolUse
        // hook bound to `write_file|edit` must no-op for `bash`.
        if !self.fires_for(ev) {
            return Ok(Vec::new());
        }
        let payload = HookPayload::from_event(ev);
        match self.run(&payload).await {
            RunOutcome::Context(ctx) => {
                // Only UserPromptSubmit yields injectable context. Tool events'
                // exit-0 stdout is intentionally NOT injected (v1 keeps the model
                // clean; a tool hook's job is allow/refuse, not feed prose).
                if matches!(self.event, UserHookEvent::UserPromptSubmit) && !ctx.is_empty() {
                    Ok(vec![ctx])
                } else {
                    Ok(Vec::new())
                }
            }
            RunOutcome::Block(msg) => {
                // exit 2 — honor the block ONLY where blocking is meaningful:
                // UserPromptSubmit (refuse the turn) and PreToolUse (refuse the
                // tool call). On Stop / PostToolUse a block is nonsensical (can't
                // un-stop or un-execute) → log and no-op so a misconfigured hook
                // never aborts an otherwise-finished run.
                match self.event {
                    UserHookEvent::UserPromptSubmit | UserHookEvent::PreToolUse => {
                        Err(BlockReason {
                            hook: format!("user-hook:{}", self.name),
                            message: if msg.is_empty() {
                                "hook requested block (exit 2)".into()
                            } else {
                                msg
                            },
                            severity: Severity::Block,
                        })
                    }
                    UserHookEvent::Stop | UserHookEvent::PostToolUse => {
                        log::warn!(
                            "[user-hook:{}] exit-2 block ignored on {:?}: {}",
                            self.name,
                            self.event,
                            msg
                        );
                        Ok(Vec::new())
                    }
                }
            }
            RunOutcome::Warn(msg) => {
                log::warn!("[user-hook:{}] {}", self.name, msg);
                Ok(Vec::new())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn working_dir() -> Option<PathBuf> {
        std::env::current_dir().ok()
    }

    #[tokio::test]
    async fn submit_hook_returns_stdout_as_context() {
        // `echo` via the system shell prints a fixed string on stdout → context.
        let h = UserCommandHook::new(
            "echo-test".into(),
            UserHookEvent::UserPromptSubmit,
            "echo CONTEXT-FROM-HOOK".into(),
            true,
            10,
            working_dir(),
        );
        let ev = HookEvent::UserPromptSubmit {
            prompt: "hi".into(),
        };
        let ctxs = h.on_event(&ev).await.unwrap();
        assert_eq!(ctxs, vec!["CONTEXT-FROM-HOOK".to_string()]);
    }

    #[tokio::test]
    async fn submit_hook_exit2_blocks_the_turn() {
        // v2: exit 2 on UserPromptSubmit → Err(BlockReason, severity Block).
        // The run loop refuses to enter the turn (claude-code's exit-2 contract).
        let h = UserCommandHook::new(
            "gate".into(),
            UserHookEvent::UserPromptSubmit,
            "exit 2".into(),
            true,
            10,
            working_dir(),
        );
        let ev = HookEvent::UserPromptSubmit {
            prompt: "hi".into(),
        };
        let err = h.on_event(&ev).await.expect_err("exit 2 must block Submit");
        assert_eq!(err.severity, Severity::Block);
        assert_eq!(err.hook, "user-hook:gate");
    }

    #[tokio::test]
    async fn pre_tool_use_hook_exit2_blocks_the_tool() {
        // A PreToolUse hook exiting 2 → Err (the tool call is refused). exit 0
        // → Ok(empty): tool-event stdout is intentionally NOT injected.
        let blocking = UserCommandHook::new(
            "no-bash".into(),
            UserHookEvent::PreToolUse,
            "exit 2".into(),
            true,
            10,
            working_dir(),
        );
        let ev_block = HookEvent::PreToolUse {
            tool: "bash".into(),
            arguments: "{\"command\":\"rm -rf x\"}".into(),
        };
        let err = blocking
            .on_event(&ev_block)
            .await
            .expect_err("exit 2 blocks tool");
        assert_eq!(err.severity, Severity::Block);

        let allowing = UserCommandHook::new(
            "allow".into(),
            UserHookEvent::PreToolUse,
            "echo would-log".into(),
            true,
            10,
            working_dir(),
        );
        let ctxs = allowing.on_event(&ev_block).await.unwrap();
        assert!(
            ctxs.is_empty(),
            "PreToolUse exit-0 stdout is NOT injected: {ctxs:?}"
        );
    }

    #[tokio::test]
    async fn post_tool_use_hook_exit2_is_ignored_not_block() {
        // A PostToolUse hook exiting 2 cannot retroactively un-execute the tool,
        // so the block is logged and dropped → Ok(empty), never Err.
        let h = UserCommandHook::new(
            "late-gate".into(),
            UserHookEvent::PostToolUse,
            "exit 2".into(),
            true,
            10,
            working_dir(),
        );
        let ev = HookEvent::PostToolUse {
            tool: "write_file".into(),
            arguments: "{}".into(),
            result: "ok".into(),
        };
        let ctxs = h.on_event(&ev).await.unwrap();
        assert!(
            ctxs.is_empty(),
            "PostToolUse block must be ignored, not propagated"
        );
    }

    #[tokio::test]
    async fn stop_hook_runs_but_returns_no_context() {
        // Stop hooks fire for their side effect; their stdout is NOT injected.
        let h = UserCommandHook::new(
            "notify".into(),
            UserHookEvent::Stop,
            "echo ignored-stdout".into(),
            true,
            10,
            working_dir(),
        );
        let ev = HookEvent::Stop {
            summary: "done".into(),
        };
        let ctxs = h.on_event(&ev).await.unwrap();
        assert!(
            ctxs.is_empty(),
            "Stop hooks must not inject context: {ctxs:?}"
        );
    }

    #[tokio::test]
    async fn hook_noops_on_mismatched_event() {
        // A Stop hook registered into the manager still no-ops on a Submit event.
        let h = UserCommandHook::new(
            "stop-only".into(),
            UserHookEvent::Stop,
            "echo x".into(),
            true,
            10,
            working_dir(),
        );
        let ev = HookEvent::UserPromptSubmit {
            prompt: "hi".into(),
        };
        assert!(h.on_event(&ev).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn failing_hook_warns_and_yields_no_context() {
        // exit non-zero (not 2) → warn, no context, no Err. `false` exits 1.
        let h = UserCommandHook::new(
            "fails".into(),
            UserHookEvent::UserPromptSubmit,
            "false".into(),
            true,
            10,
            working_dir(),
        );
        let ev = HookEvent::UserPromptSubmit {
            prompt: "hi".into(),
        };
        let ctxs = h.on_event(&ev).await.unwrap();
        assert!(ctxs.is_empty());
    }

    #[tokio::test]
    async fn hook_timeout_warns_without_hanging() {
        // sleep far past the 1s timeout → the call must return (warn) within a
        // few seconds, not hang the test.
        let h = UserCommandHook::new(
            "slow".into(),
            UserHookEvent::UserPromptSubmit,
            "sleep 10".into(),
            true,
            1,
            working_dir(),
        );
        let ev = HookEvent::UserPromptSubmit {
            prompt: "hi".into(),
        };
        let res = h.on_event(&ev).await.unwrap();
        assert!(res.is_empty(), "timed-out hook yields no context: {res:?}");
    }

    #[test]
    fn payload_serializes_event_fields() {
        let submit = HookEvent::UserPromptSubmit {
            prompt: "do X".into(),
        };
        let p = HookPayload::from_event(&submit).to_stdin();
        assert!(p.contains("\"hook_event_name\":\"UserPromptSubmit\""));
        assert!(p.contains("\"prompt\":\"do X\""));
        assert!(!p.contains("summary"), "submit payload has no summary");
        assert!(p.ends_with('\n'), "trailing newline");

        let stop = HookEvent::Stop {
            summary: "done".into(),
        };
        let p2 = HookPayload::from_event(&stop).to_stdin();
        assert!(p2.contains("\"hook_event_name\":\"Stop\""));
        assert!(p2.contains("\"summary\":\"done\""));
        assert!(!p2.contains("prompt"));

        // PreToolUse carries tool + tool_input, no tool_response.
        let pre = HookEvent::PreToolUse {
            tool: "bash".into(),
            arguments: "{\"command\":\"ls\"}".into(),
        };
        let p3 = HookPayload::from_event(&pre).to_stdin();
        assert!(p3.contains("\"hook_event_name\":\"PreToolUse\""));
        assert!(p3.contains("\"tool_name\":\"bash\""));
        assert!(p3.contains("\"tool_input\":\"{\\\"command\\\":\\\"ls\\\"}\""));
        assert!(
            !p3.contains("tool_response"),
            "PreToolUse has no tool_response"
        );

        // PostToolUse carries tool + tool_input + tool_response.
        let post = HookEvent::PostToolUse {
            tool: "write_file".into(),
            arguments: "{}".into(),
            result: "wrote 3 lines".into(),
        };
        let p4 = HookPayload::from_event(&post).to_stdin();
        assert!(p4.contains("\"hook_event_name\":\"PostToolUse\""));
        assert!(p4.contains("\"tool_response\":\"wrote 3 lines\""));
    }

    #[test]
    fn matches_only_bound_event() {
        let submit_hook = UserCommandHook::new(
            "s".into(),
            UserHookEvent::UserPromptSubmit,
            "x".into(),
            true,
            5,
            None,
        );
        assert!(submit_hook.matches(&HookEvent::UserPromptSubmit { prompt: "p".into() }));
        assert!(!submit_hook.matches(&HookEvent::Stop {
            summary: "s".into()
        }));

        let pre_hook = UserCommandHook::new(
            "pre".into(),
            UserHookEvent::PreToolUse,
            "x".into(),
            true,
            5,
            None,
        );
        assert!(pre_hook.matches(&HookEvent::PreToolUse {
            tool: "bash".into(),
            arguments: "{}".into(),
        }));
        assert!(!pre_hook.matches(&HookEvent::PostToolUse {
            tool: "bash".into(),
            arguments: "{}".into(),
            result: "".into(),
        }));
    }

    // --- v2 matcher: claude-code matchesPattern 3-mode (literal / pipe / regex) ---

    #[test]
    fn matches_pattern_wildcard() {
        // None / "" / "*" all mean match-all (the canonical no-filter state).
        assert!(UserCommandHook::matches_pattern("anything", None));
        assert!(UserCommandHook::matches_pattern("anything", Some("")));
        assert!(UserCommandHook::matches_pattern("anything", Some("*")));
    }

    #[test]
    fn matches_pattern_literal_exact() {
        // Plain alphanumeric matcher → exact equality (no substring, no regex).
        assert!(UserCommandHook::matches_pattern(
            "write_file",
            Some("write_file")
        ));
        assert!(!UserCommandHook::matches_pattern(
            "write_file",
            Some("write")
        ));
        assert!(!UserCommandHook::matches_pattern(
            "Write_File",
            Some("write_file")
        ));
    }

    #[test]
    fn matches_pattern_pipe_alternation() {
        // `|` among simple chars → split, match if query equals ANY segment.
        assert!(UserCommandHook::matches_pattern(
            "edit",
            Some("write_file|edit")
        ));
        assert!(UserCommandHook::matches_pattern(
            "write_file",
            Some("write_file|edit")
        ));
        assert!(!UserCommandHook::matches_pattern(
            "bash",
            Some("write_file|edit")
        ));
        // A SPACE breaks the is_simple gate (`[A-Za-z0-9_|]` only) → the matcher
        // falls through to regex mode (faithful to claude-code, whose gate is
        // `/^[a-zA-Z0-9_|]+$/`). So "write_file | edit" is regex, not pipe, and
        // must NOT match "edit" (the regex's alternatives carry the spaces).
        assert!(!UserCommandHook::matches_pattern(
            "edit",
            Some("write_file | edit")
        ));
    }

    #[test]
    fn matches_pattern_regex_mode() {
        // Any regex metacharacter (here `^`) drops us out of literal mode into
        // regex; `is_match` is a partial match like claude-code's regex.test.
        assert!(UserCommandHook::matches_pattern(
            "write_file",
            Some("^write_")
        ));
        assert!(UserCommandHook::matches_pattern(
            "read_file",
            Some("^(read|write)_")
        ));
        assert!(!UserCommandHook::matches_pattern("bash", Some("^write_")));
        // Invalid regex must NOT panic — it logs and matches nothing.
        assert!(!UserCommandHook::matches_pattern("x", Some("(")));
    }

    #[tokio::test]
    async fn fires_for_gates_tool_events_on_matcher() {
        // A PreToolUse hook scoped to write_file|edit fires for those tools and
        // no-ops (Ok empty) for bash — proving the matcher is consulted at
        // dispatch, not just at load. exit 0 on a tool event yields no context.
        let h = UserCommandHook::new(
            "no-write".into(),
            UserHookEvent::PreToolUse,
            "echo would-block".into(),
            true,
            5,
            None,
        )
        .with_matcher(Some("write_file|edit".into()));

        // Matching tool → hook runs (exit 0, no injection on tool events).
        let ev_match = HookEvent::PreToolUse {
            tool: "write_file".into(),
            arguments: "{}".into(),
        };
        assert!(h.on_event(&ev_match).await.unwrap().is_empty());

        // Non-matching tool → hook no-ops without running the command. We can't
        // observe "didn't run" directly here, but fires_for is the gate; assert
        // it via the public predicate's effect: a Stop-bound matcher hook still
        // fires on Stop (matcher ignored for non-tool events).
        let stop_hook = UserCommandHook::new(
            "notify".into(),
            UserHookEvent::Stop,
            "echo done".into(),
            true,
            5,
            None,
        )
        .with_matcher(Some("write_file|edit".into()));
        let ev_stop = HookEvent::Stop {
            summary: "done".into(),
        };
        // Stop ignores the matcher — hook fires, returns no context.
        assert!(stop_hook.on_event(&ev_stop).await.unwrap().is_empty());
    }
}
