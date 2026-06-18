//! D2 user-configurable lifecycle hooks. A [`UserCommandHook`] wraps one
//! `user_hooks` row as a [`Hook`] that fires on a single [`HookEvent`] (turn
//! start / run stop), runs the configured shell command with a claude-code-style
//! event JSON on stdin, and (for `UserPromptSubmit`) returns stdout as injected
//! context. This is the user-facing half of the lifecycle-hook layer; the
//! built-in CommandGuard/AssertionGuard/TaskGuard hooks (always on) live in
//! `kernel_impl/hooks.rs`.
//!
//! v1 scope (see design doc): command-type hooks only, `UserPromptSubmit` +
//! `Stop` events only, exit-code protocol honored but NON-blocking (the
//! `Hook::on_event` contract is "observation only, never gates" — exit 2 logs a
//! warning rather than refusing the turn; full gating needs a future trait
//! evolution).

pub mod registry;

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::kernel_impl::hooks::{Hook, HookEvent};
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
        Self { name, event, command, shell, timeout_secs, working_dir }
    }

    /// Does this hook's configured event match the dispatched one? A hook bound
    /// to `Stop` must no-op on `UserPromptSubmit` even though it's registered
    /// into the same HookManager (we pre-filter by event at load time, but keep
    /// the guard defensive).
    fn matches(&self, ev: &HookEvent) -> bool {
        match (&self.event, ev) {
            (UserHookEvent::UserPromptSubmit, HookEvent::UserPromptSubmit { .. }) => true,
            (UserHookEvent::Stop, HookEvent::Stop { .. }) => true,
            _ => false,
        }
    }
}

/// The stdin payload sent to every hook command (claude-code protocol, minimal).
/// `hook_event_name` lets one command serve multiple events if it inspects stdin.
#[derive(Serialize)]
struct HookPayload<'a> {
    hook_event_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<&'a str>,
}

impl<'a> HookPayload<'a> {
    fn from_event(ev: &'a HookEvent) -> Self {
        match ev {
            HookEvent::UserPromptSubmit { prompt } => HookPayload {
                hook_event_name: "UserPromptSubmit",
                prompt: Some(prompt),
                summary: None,
            },
            HookEvent::Stop { summary } => HookPayload {
                hook_event_name: "Stop",
                prompt: None,
                summary: Some(summary),
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

/// Outcome of running one hook command. `stdout` is set only on a clean exit 0
/// (the only case that yields injectable context); everything else is a warning
/// that's logged by the caller and yields no context.
enum RunOutcome {
    Context(String),
    Warn(String),
}

impl UserCommandHook {
    /// Run the configured command with `payload` on stdin, bounded by
    /// `timeout_secs`. Cross-platform: `shell=true` (default) routes through the
    /// system shell (`cmd /C` on Windows, `sh -c` elsewhere, identical to the
    /// kernel BashTool); `shell=false` splits the command on whitespace into
    /// program + args (naive — no quoting; documented). `CREATE_NO_WINDOW` on
    /// Windows so hook commands don't flash a console, matching every other
    /// subprocess spawn in the app.
    async fn run(&self, payload: &HookPayload<'_>) -> RunOutcome {
        let stdin_str = payload.to_stdin();

        let mut cmd = if self.shell {
            if cfg!(target_os = "windows") {
                let mut c = tokio::process::Command::new("cmd");
                c.arg("/C").arg(&self.command);
                c
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

        let timeout = Duration::from_secs(self.timeout_secs.max(1));
        match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(out)) => {
                let code = out.status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                match code {
                    0 => RunOutcome::Context(stdout.trim_end().to_string()),
                    2 => {
                        // claude-code BLOCKS here; v1 honors the on_event
                        // non-gating contract → warn + no context. Logged by caller.
                        RunOutcome::Warn(format!(
                            "hook {} requested block (exit 2): {}",
                            self.name,
                            stderr.trim()
                        ))
                    }
                    other => RunOutcome::Warn(format!(
                        "hook {} failed (exit {other}): {}",
                        self.name,
                        stderr.trim()
                    )),
                }
            }
            Ok(Err(e)) => RunOutcome::Warn(format!("hook {} wait failed: {e}", self.name)),
            Err(_) => RunOutcome::Warn(format!(
                "hook {} timed out ({}s)",
                self.name, self.timeout_secs
            )),
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

    async fn on_event(&self, ev: &HookEvent) -> Vec<String> {
        // Only Stop hooks produce side-effects worth running for their own sake;
        // for UserPromptSubmit the stdout becomes context. A hook whose event
        // doesn't match the dispatched one is a no-op (defensive — load time
        // already filters by event).
        if !self.matches(ev) {
            return Vec::new();
        }
        let payload = HookPayload::from_event(ev);
        match self.run(&payload).await {
            RunOutcome::Context(ctx) => {
                if matches!(self.event, UserHookEvent::UserPromptSubmit) && !ctx.is_empty() {
                    vec![ctx]
                } else {
                    // Stop hooks (or empty stdout) carry no injectable context.
                    Vec::new()
                }
            }
            RunOutcome::Warn(msg) => {
                log::warn!("[user-hook:{}] {}", self.name, msg);
                Vec::new()
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
            if cfg!(target_os = "windows") {
                "echo CONTEXT-FROM-HOOK".into()
            } else {
                "echo CONTEXT-FROM-HOOK".into()
            },
            true,
            10,
            working_dir(),
        );
        let ev = HookEvent::UserPromptSubmit { prompt: "hi".into() };
        let ctxs = h.on_event(&ev).await;
        assert_eq!(ctxs, vec!["CONTEXT-FROM-HOOK".to_string()]);
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
        let ev = HookEvent::Stop { summary: "done".into() };
        let ctxs = h.on_event(&ev).await;
        assert!(ctxs.is_empty(), "Stop hooks must not inject context: {ctxs:?}");
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
        let ev = HookEvent::UserPromptSubmit { prompt: "hi".into() };
        assert!(h.on_event(&ev).await.is_empty());
    }

    #[tokio::test]
    async fn failing_hook_warns_and_yields_no_context() {
        // exit non-zero → warn, no context, no panic. `false` exits 1.
        let h = UserCommandHook::new(
            "fails".into(),
            UserHookEvent::UserPromptSubmit,
            if cfg!(target_os = "windows") {
                "cmd /C exit 1".into()
            } else {
                "false".into()
            },
            true,
            10,
            working_dir(),
        );
        let ev = HookEvent::UserPromptSubmit { prompt: "hi".into() };
        assert!(h.on_event(&ev).await.is_empty());
    }

    #[tokio::test]
    async fn hook_timeout_warns_without_hanging() {
        // sleep far past the 1s timeout → the call must return (warn) within a
        // few seconds, not hang the test.
        let h = UserCommandHook::new(
            "slow".into(),
            UserHookEvent::UserPromptSubmit,
            if cfg!(target_os = "windows") {
                "ping -n 10 127.0.0.1 > nul".into()
            } else {
                "sleep 10".into()
            },
            true,
            1,
            working_dir(),
        );
        let ev = HookEvent::UserPromptSubmit { prompt: "hi".into() };
        let res = h.on_event(&ev).await;
        assert!(res.is_empty(), "timed-out hook yields no context: {res:?}");
    }

    #[test]
    fn payload_serializes_event_fields() {
        let submit = HookEvent::UserPromptSubmit { prompt: "do X".into() };
        let p = HookPayload::from_event(&submit).to_stdin();
        assert!(p.contains("\"hook_event_name\":\"UserPromptSubmit\""));
        assert!(p.contains("\"prompt\":\"do X\""));
        assert!(!p.contains("summary"), "submit payload has no summary");
        assert!(p.ends_with('\n'), "trailing newline");

        let stop = HookEvent::Stop { summary: "done".into() };
        let p2 = HookPayload::from_event(&stop).to_stdin();
        assert!(p2.contains("\"hook_event_name\":\"Stop\""));
        assert!(p2.contains("\"summary\":\"done\""));
        assert!(!p2.contains("prompt"));
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
        assert!(!submit_hook.matches(&HookEvent::Stop { summary: "s".into() }));
    }
}
