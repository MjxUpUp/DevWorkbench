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

/// Map a tool call name+arguments into an [`Action`] variant. WriteFile and
/// RunCommand are classified by tool name so Plan mode and AssertionGuard can
/// act on them; everything else stays as CallTool (the historic single-channel).
///
/// Built-in tool names: `write_file` / `WriteFile` / `write` → WriteFile;
/// `bash` / `Bash` / `exec` / `shell` / `cmd` → RunCommand.
pub fn classify_action(tool_name: &str, arguments: &str) -> Action {
    let args_val = || -> Option<serde_json::Value> {
        serde_json::from_str(arguments).ok()
    };
    if matches!(
        tool_name,
        "write_file" | "WriteFile" | "write" | "Write" | "patch"
    ) {
        let (path, preview) = args_val()
            .as_ref()
            .map(|v| {
                let p = v
                    .get("file_path")
                    .or_else(|| v.get("path"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let c = v
                    .get("content")
                    .or_else(|| v.get("text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                (p.to_string(), c.chars().take(200).collect::<String>())
            })
            .unwrap_or_default();
        Action::WriteFile {
            path,
            content_preview: preview,
        }
    } else if matches!(
        tool_name,
        "bash" | "Bash" | "exec" | "shell" | "cmd" | "sh" | "run"
    ) {
        let command = args_val()
            .as_ref()
            .and_then(|v| {
                v.get("command")
                    .and_then(|c| c.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| arguments.to_string());
        Action::RunCommand { command }
    } else {
        Action::CallTool {
            tool: tool_name.to_string(),
            arguments: arguments.to_string(),
        }
    }
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

/// Permission mode — mirrors the frontend `AgentMode` selector and shapes how
/// [`HookManager::before`] gates actions. This is the backend half of
/// "permission mode 贯通": the run loop carries the selected mode into
/// `before`, which short-circuits before the per-hook dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    /// Interactive execution (P0: existing guard behaviour).
    #[default]
    Default,
    /// Auto-accept file edits without asking (P0: same as Default — the
    /// confirm-on-write interaction lands later).
    AutoEdit,
    /// Read-only planning — block writes and command execution until the user
    /// confirms the plan.
    Plan,
    /// Dry-run / preview (v2.0 C6) — let the agent reason over a COMPLETE plan
    /// with zero side effects. Read-only tools (search/read) run for real so the
    /// agent plans against actual file contents; every side-effecting tool is
    /// intercepted in `execute_tool_call` and returns a simulated result instead
    /// of landing. Unlike `Plan` (which *blocks* writes and halts the loop),
    /// DryRun keeps the loop running so the agent emits a full execution plan the
    /// user can review before re-running in a real mode. `blocks_action` returns
    /// `None` for it — the simulation happens one layer down, not at the gate.
    DryRun,
    /// Minimal output (P0: same guard behaviour; event throttling is later).
    Silent,
    /// Skip every permission check — bypass all before-hooks.
    SkipPermissions,
}

impl PermissionMode {
    /// Returns a block reason if this mode forbids the action outright (plan
    /// mode blocks file writes and command execution). Pure predicate so it's
    /// unit-testable in isolation, away from the hook dispatch.
    pub fn blocks_action(self, action: &Action) -> Option<&'static str> {
        match self {
            PermissionMode::Plan => match action {
                Action::WriteFile { .. } => {
                    Some("plan mode is read-only: confirm the plan before writing files")
                }
                Action::RunCommand { .. } => {
                    Some("plan mode is read-only: confirm the plan before running commands")
                }
                Action::CallTool { .. } => None,
            },
            _ => None,
        }
    }

    /// Does this mode skip all before-hooks? (skip-permissions.)
    pub fn skips_guards(self) -> bool {
        matches!(self, PermissionMode::SkipPermissions)
    }

    /// Is this dry-run / preview mode (v2.0 C6)? Side-effecting tools are
    /// simulated, not landed — see `execute_tool_call`. `blocks_action` returns
    /// `None` for it; the interception happens one layer down so read-only tools
    /// still run for real and the loop keeps producing a plan.
    pub fn is_dry_run(self) -> bool {
        matches!(self, PermissionMode::DryRun)
    }
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
    mode: PermissionMode,
}

impl Default for HookManager {
    fn default() -> Self {
        Self::new()
    }
}

impl HookManager {
    pub fn new() -> Self {
        Self {
            hooks: Vec::new(),
            mode: PermissionMode::Default,
        }
    }

    /// Set the permission mode shaping how `before` gates actions.
    pub fn with_mode(mut self, mode: PermissionMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    pub fn register(&mut self, hook: Box<dyn Hook>) {
        self.hooks.push(hook);
    }

    /// Run all `before` hooks. Returns the first BlockReason, or Ok.
    /// (A BlockReason stops the action; warnings are logged but don't block.)
    pub async fn before(&self, action: &Action) -> Result<(), BlockReason> {
        // Permission-mode short-circuit (runs before per-hook dispatch):
        // skip-permissions bypasses everything; plan mode blocks writes and
        // command execution until the user confirms the plan.
        if self.mode.skips_guards() {
            return Ok(());
        }
        if let Some(msg) = self.mode.blocks_action(action) {
            return Err(BlockReason {
                hook: "permission_mode".into(),
                message: msg.into(),
                severity: Severity::Block,
            });
        }
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


/// Best-effort: extract a shell command string from a tool's JSON arguments
/// (looks for "command"/"cmd"/"script" keys).
fn extract_command_from_args(args: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(args).ok()?;
    for key in &["command", "cmd", "script", "shell_command"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

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
        match action {
            Action::RunCommand { command } => {
                if let Some(reason) = self.classify(command) {
                    return Err(reason);
                }
            }
            Action::CallTool { tool, arguments } => {
                // M7: also guard tool calls whose name suggests shell execution.
                let dangerous_tools = ["exec", "shell", "bash", "cmd", "powershell", "sh"];
                let lower = tool.to_lowercase();
                if dangerous_tools.iter().any(|dt| lower.contains(dt)) {
                    // Inspect arguments for dangerous commands.
                    if let Some(cmd) = extract_command_from_args(arguments) {
                        if let Some(reason) = self.classify(&cmd) {
                            return Err(reason);
                        }
                    }
                }
            }
            _ => {}
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

    // --- v1.1 Task 5: permission mode gating ---

    #[test]
    fn plan_mode_blocks_writes_and_commands_only() {
        let write = Action::WriteFile { path: "x.rs".into(), content_preview: "".into() };
        let run = Action::RunCommand { command: "echo hi".into() };
        let tool = Action::CallTool { tool: "read".into(), arguments: "{}".into() };
        assert!(PermissionMode::Plan.blocks_action(&write).is_some());
        assert!(PermissionMode::Plan.blocks_action(&run).is_some());
        // Plan still allows read-style tool calls — only writes/commands stop.
        assert!(PermissionMode::Plan.blocks_action(&tool).is_none());
    }

    // --- v2.0 C6: dry-run execution mode layering ---

    #[test]
    fn dry_run_blocks_nothing_and_is_detectable() {
        // DryRun must NOT gate at the hook layer — its interception lives in
        // execute_tool_call. Every action passes `blocks_action` unchanged, and
        // `is_dry_run` flags it so the tool-execution layer can simulate.
        let write = Action::WriteFile { path: "x.rs".into(), content_preview: "".into() };
        let run = Action::RunCommand { command: "rm -rf x".into() };
        let tool = Action::CallTool { tool: "write".into(), arguments: "{}".into() };
        assert!(PermissionMode::DryRun.blocks_action(&write).is_none());
        assert!(PermissionMode::DryRun.blocks_action(&run).is_none());
        assert!(PermissionMode::DryRun.blocks_action(&tool).is_none());
        assert!(PermissionMode::DryRun.is_dry_run());
        assert!(!PermissionMode::Default.is_dry_run());
        assert!(!PermissionMode::Plan.is_dry_run());
    }

    #[tokio::test]
    async fn dry_run_manager_lets_side_effects_through_gate() {
        // The gate must pass a write action in dry-run (unlike Plan, which
        // blocks). The simulation is the tool layer's job, not the hook's.
        let mgr = HookManager::new().with_mode(PermissionMode::DryRun);
        let write = Action::WriteFile { path: "x.rs".into(), content_preview: "".into() };
        assert!(mgr.before(&write).await.is_ok());
    }

    #[test]
    fn non_plan_modes_do_not_block_writes() {
        let write = Action::WriteFile { path: "x.rs".into(), content_preview: "".into() };
        for m in [
            PermissionMode::Default,
            PermissionMode::AutoEdit,
            PermissionMode::DryRun,
            PermissionMode::Silent,
            PermissionMode::SkipPermissions,
        ] {
            assert!(m.blocks_action(&write).is_none(), "{m:?} should not block writes");
        }
    }

    #[test]
    fn skips_guards_only_for_skip_permissions() {
        assert!(PermissionMode::SkipPermissions.skips_guards());
        for m in [
            PermissionMode::Default,
            PermissionMode::AutoEdit,
            PermissionMode::Plan,
            PermissionMode::DryRun,
            PermissionMode::Silent,
        ] {
            assert!(!m.skips_guards(), "{m:?} should not skip guards");
        }
    }

    #[tokio::test]
    async fn plan_mode_manager_blocks_write_before_hooks() {
        let mgr = HookManager::new().with_mode(PermissionMode::Plan);
        let write = Action::WriteFile { path: "x.rs".into(), content_preview: "".into() };
        let err = mgr.before(&write).await.unwrap_err();
        assert_eq!(err.severity, Severity::Block);
        assert_eq!(err.hook, "permission_mode");
    }

    #[tokio::test]
    async fn skip_permissions_bypasses_command_guard() {
        // Even a destructive `rm -rf /` is allowed under skip-permissions — the
        // mode short-circuits before the command guard runs.
        let mut mgr = HookManager::new().with_mode(PermissionMode::SkipPermissions);
        mgr.register(Box::new(CommandGuardHook::default()));
        assert!(mgr.before(&cmd("rm -rf /")).await.is_ok());
    }

    #[tokio::test]
    async fn default_mode_still_enforces_command_guard() {
        let mut mgr = HookManager::new();
        mgr.register(Box::new(CommandGuardHook::default()));
        assert!(mgr.before(&cmd("rm -rf /")).await.is_err());
    }

    #[test]
    fn permission_mode_serde_matches_frontend_kebab() {
        // Frontend AgentMode uses kebab-case ids; the backend must round-trip
        // the exact same strings so the wire value passes through unchanged.
        let cases = [
            ("default", PermissionMode::Default),
            ("auto-edit", PermissionMode::AutoEdit),
            ("plan", PermissionMode::Plan),
            ("dry-run", PermissionMode::DryRun),
            ("silent", PermissionMode::Silent),
            ("skip-permissions", PermissionMode::SkipPermissions),
        ];
        for (s, mode) in cases {
            let ser = serde_json::to_string(&mode).unwrap();
            assert_eq!(ser, format!("\"{s}\""), "serialize {mode:?}");
            let de: PermissionMode = serde_json::from_str(&format!("\"{s}\"")).unwrap();
            assert_eq!(de, mode, "deserialize {s}");
        }
    }
}
