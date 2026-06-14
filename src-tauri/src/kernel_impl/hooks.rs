//! Hook system — pre/post-action interception for the kernel.
//!
//! This is the kernel-level home of Forge's task-guard / bash-guard /
//! file-sentinel patterns: hooks fire before and after agent actions and can
//! veto dangerous ones or record/revert after the fact.
//!
//! - A [`Hook::before`] returning `Err` BLOCKS the action (the agent sees the
//!   error and must adapt).
//! - A [`Hook::after`] observes the outcome (and may flag it, e.g. assertion
//!   weakening) but cannot block retroactively.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::kernel_impl::honesty::{check_assertion_weakening, parse_diff, HonestyWarning};

/// The kind of agent action a hook can observe.
#[derive(Debug, Clone)]
pub enum Action {
    /// The agent is about to run a shell command.
    RunCommand { command: String },
    /// The agent is about to write/patch a file.
    WriteFile { path: String, content_preview: String },
    /// The agent is about to invoke a tool (named).
    CallTool { tool: String, arguments: String },
}

/// The outcome of an action, for post-hooks to inspect.
#[derive(Debug, Clone)]
pub struct ActionOutcome {
    pub action: Action,
    pub ok: bool,
    /// For file writes: the resulting git diff (so assertion-weakening can scan it).
    pub diff: Option<String>,
    pub error: Option<String>,
}

/// Why a hook blocked an action (carried in the Err).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockReason {
    pub hook: String,
    pub message: String,
    pub severity: Severity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Hard block — the action is refused.
    Block,
    /// Warn but allow (logged).
    Warn,
}

/// A hook. `before` may veto; `after` observes.
#[async_trait]
pub trait Hook: Send + Sync {
    fn name(&self) -> &str;
    async fn before(&self, _action: &Action) -> Result<(), BlockReason> {
        Ok(())
    }
    async fn after(&self, _outcome: &ActionOutcome) -> Vec<HonestyWarning> {
        Vec::new()
    }
}

/// Registry + dispatcher for hooks.
pub struct HookManager {
    hooks: Vec<Box<dyn Hook>>,
}

impl Default for HookManager {
    fn default() -> Self {
        Self::new()
    }
}

impl HookManager {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub fn register(&mut self, hook: Box<dyn Hook>) {
        self.hooks.push(hook);
    }

    /// Run all `before` hooks. Returns the first BlockReason, or Ok.
    /// (A BlockReason stops the action; warnings are logged but don't block.)
    pub async fn before(&self, action: &Action) -> Result<(), BlockReason> {
        for h in &self.hooks {
            if let Err(reason) = h.before(action).await {
                if reason.severity == Severity::Block {
                    return Err(reason);
                }
                log::warn!("[hook:{}] {}", h.name(), reason.message);
            }
        }
        Ok(())
    }

    /// Run all `after` hooks, collecting any findings (e.g. assertion weakening).
    pub async fn after(&self, outcome: &ActionOutcome) -> Vec<HonestyWarning> {
        let mut all = Vec::new();
        for h in &self.hooks {
            all.extend(h.after(outcome).await);
        }
        all
    }

    pub fn count(&self) -> usize {
        self.hooks.len()
    }
}

// ---------------------------------------------------------------------------
// Built-in hooks
// ---------------------------------------------------------------------------

/// Reject shell commands that are genuinely destructive. Uses token-based
/// detection (the Forge bash-guard analog) instead of naive substring matching,
/// so `rm -rf /home/user/old-build` (legitimate) is NOT blocked while
/// `rm -rf /` (wipe root) IS.
pub struct CommandGuardHook {
    /// User-configurable allowlist of command prefixes that bypass the guard.
    allowlist: Vec<String>,
}

impl Default for CommandGuardHook {
    fn default() -> Self {
        Self { allowlist: Vec::new() }
    }
}

impl CommandGuardHook {
    pub fn with_allowlist(allowlist: Vec<String>) -> Self {
        Self { allowlist }
    }

    /// Returns a BlockReason if the command is dangerous, else None.
    /// Token-based: splits on whitespace, inspects the program + flags + target.
    fn classify(&self, command: &str) -> Option<BlockReason> {
        let tokens: Vec<&str> = command.split_whitespace().collect();
        if tokens.is_empty() {
            return None;
        }
        let prog = tokens[0].to_lowercase();
        let joined = command.to_lowercase();

        // Allowlist bypass.
        for allowed in &self.allowlist {
            if joined.starts_with(&allowed.to_lowercase()) {
                return None;
            }
        }

        // rm with recursive-force targeting root or home root.
        if prog == "rm" || prog.ends_with("/rm") {
            let has_rf = tokens.iter().any(|t| {
                let t = t.trim_start_matches('-');
                t.contains('r') && t.contains('f')
            });
            if has_rf {
                // Find the path target (first non-flag token after flags).
                let target = tokens.iter().skip(1).find(|t| !t.starts_with('-'));
                if let Some(t) = target {
                    let t = t.trim_matches('"').trim_matches('\'');
                    // Block wiping root, home root, or system dirs.
                    let dangerous = t == "/" || t == "~" || t == "/*"
                        || t == "/home" || t == "/usr" || t == "/bin"
                        || t == "/etc" || t == "/var" || t == "/boot"
                        || t.starts_with("/dev/sd") || t.starts_with("/dev/nvme");
                    if dangerous {
                        return Some(BlockReason {
                            hook: "command_guard".into(),
                            message: format!("blocked rm -rf on system path: {t}"),
                            severity: Severity::Block,
                        });
                    }
                }
            }
        }

        // Fork bomb variants.
        if joined.contains(":(){") || joined.contains(": () {") {
            return Some(BlockReason {
                hook: "command_guard".into(),
                message: "blocked fork bomb".into(),
                severity: Severity::Block,
            });
        }

        // Filesystem format / disk wipe.
        if prog == "mkfs" || joined.contains("dd if=/dev/zero of=/dev/")
            || joined.contains("dd if=/dev/urandom of=/dev/")
        {
            return Some(BlockReason {
                hook: "command_guard".into(),
                message: "blocked filesystem format / disk wipe".into(),
                severity: Severity::Block,
            });
        }

        // Shutdown / reboot.
        if prog == "shutdown" || prog == "halt" || prog == "poweroff" {
            return Some(BlockReason {
                hook: "command_guard".into(),
                message: "blocked shutdown/poweroff".into(),
                severity: Severity::Block,
            });
        }

        None
    }
}

#[async_trait]
impl Hook for CommandGuardHook {
    fn name(&self) -> &str {
        "command_guard"
    }
    async fn before(&self, action: &Action) -> Result<(), BlockReason> {
        if let Action::RunCommand { command } = action {
            if let Some(reason) = self.classify(command) {
                return Err(reason);
            }
        }
        Ok(())
    }
}

/// Scan file-write diffs for assertion weakening (the Forge assertion-check +
/// HonestyVerifier analog). Runs in `after`, surfaces findings rather than
/// blocking (a weakening is reported, not reverted — revert is a separate step).
pub struct AssertionGuardHook;

#[async_trait]
impl Hook for AssertionGuardHook {
    fn name(&self) -> &str {
        "assertion_guard"
    }
    async fn after(&self, outcome: &ActionOutcome) -> Vec<HonestyWarning> {
        match &outcome.diff {
            Some(diff_text) if !diff_text.is_empty() => {
                let diff = parse_diff(diff_text);
                check_assertion_weakening(&diff)
            }
            _ => Vec::new(),
        }
    }
}

/// Require an active task before allowing file writes (the Forge task-guard
/// analog — no edits without a tracked task).
pub struct TaskGuardHook {
    has_active_task: std::sync::Mutex<bool>,
}

impl TaskGuardHook {
    pub fn new() -> Self {
        Self {
            has_active_task: std::sync::Mutex::new(false),
        }
    }
    pub fn set_active(&self, active: bool) {
        if let Ok(mut g) = self.has_active_task.lock() {
            *g = active;
        }
    }
}

impl Default for TaskGuardHook {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Hook for TaskGuardHook {
    fn name(&self) -> &str {
        "task_guard"
    }
    async fn before(&self, action: &Action) -> Result<(), BlockReason> {
        if let Action::WriteFile { .. } = action {
            let active = self.has_active_task.lock().map(|g| *g).unwrap_or(false);
            if !active {
                return Err(BlockReason {
                    hook: self.name().into(),
                    message: "file write blocked: no active task (start one via forge task start)".into(),
                    severity: Severity::Block,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(c: &str) -> Action {
        Action::RunCommand { command: c.into() }
    }

    #[tokio::test]
    async fn command_guard_blocks_rm_rf_root() {
        let h = CommandGuardHook::default();
        let err = h.before(&cmd("rm -rf /")).await.unwrap_err();
        assert_eq!(err.severity, Severity::Block);
        assert!(err.message.contains("system path"), "got: {err:?}");
    }

    #[tokio::test]
    async fn command_guard_allows_safe_command() {
        let h = CommandGuardHook::default();
        assert!(h.before(&cmd("cargo build")).await.is_ok());
    }

    #[tokio::test]
    async fn assertion_guard_reports_weakening_from_diff() {
        let h = AssertionGuardHook;
        let diff = "--- a/t.rs\n+++ b/t.rs\n-func()\n-t.Fatal(\"x\")\n+func()\n+t.Log(\"x\")\n";
        let outcome = ActionOutcome {
            action: Action::WriteFile { path: "t.rs".into(), content_preview: "".into() },
            ok: true,
            diff: Some(diff.into()),
            error: None,
        };
        let findings = h.after(&outcome).await;
        assert!(findings.iter().any(|f| f.rule == "fatal_to_log"));
    }

    #[tokio::test]
    async fn task_guard_blocks_write_without_active_task() {
        let h = TaskGuardHook::new();
        let action = Action::WriteFile { path: "x.rs".into(), content_preview: "".into() };
        let err = h.before(&action).await.unwrap_err();
        assert_eq!(err.severity, Severity::Block);
    }

    #[tokio::test]
    async fn task_guard_allows_write_with_active_task() {
        let h = TaskGuardHook::new();
        h.set_active(true);
        let action = Action::WriteFile { path: "x.rs".into(), content_preview: "".into() };
        assert!(h.before(&action).await.is_ok());
    }

    #[tokio::test]
    async fn hook_manager_dispatches_before_and_blocks() {
        let mut mgr = HookManager::new();
        mgr.register(Box::new(CommandGuardHook::default()));
        let err = mgr.before(&cmd("rm -rf /home")).await;
        // "rm -rf /" is a substring of "rm -rf /home" → blocked.
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn hook_manager_collects_after_findings() {
        let mut mgr = HookManager::new();
        mgr.register(Box::new(AssertionGuardHook));
        let outcome = ActionOutcome {
            action: Action::WriteFile { path: "t.rs".into(), content_preview: "".into() },
            ok: true,
            diff: Some("--- a/t.rs\n+++ b/t.rs\n-x\n-t.Fatal(\"x\")\n+x\n+t.Log(\"x\")\n".into()),
            error: None,
        };
        let findings = mgr.after(&outcome).await;
        assert!(!findings.is_empty());
    }
}
