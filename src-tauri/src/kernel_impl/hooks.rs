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
    WriteFile {
        path: String,
        content_preview: String,
    },
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
    let args_val = || -> Option<serde_json::Value> { serde_json::from_str(arguments).ok() };
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
    /// Mission Phase 2 — controller-only execution (D4). The agent delegates
    /// ALL coding to sub-agents; its ToolRegistry is the read-only +
    /// `dispatch_subagent` subset (see `build_react_agent`), so unlike `Plan`
    /// this mode does NOT block writes at the gate — the restriction is
    /// structural (the tool set), not a permission veto. The master still
    /// needs to write `prd.json` (flip `passes`) and dispatch workers, mirroring
    /// QwenPaw's Phase 2 keeping `write_file`/`execute_shell_command` available.
    Executing,
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

    /// Is this Mission Phase 2 controller-execution mode (D4)? The tool set
    /// is already restricted to read-only + `dispatch_subagent`, so the gate
    /// does not veto writes here — this flag lets the run loop + UI mark the
    /// current phase distinctly from `Default`.
    pub fn is_executing(self) -> bool {
        matches!(self, PermissionMode::Executing)
    }
}

/// Lifecycle events hooks may observe via [`Hook::on_event`]. The dispatch
/// points cover the full claude-code lifecycle: `UserPromptSubmit` (a turn
/// starting), `PreToolUse`/`PostToolUse` (around each tool invocation), and
/// `Stop` (the run finishing). Built-in hooks default to no-op; the trait
/// method has a default impl so existing hooks stay source-compatible.
#[derive(Debug, Clone)]
pub enum HookEvent {
    /// A new user prompt is about to be sent to the model.
    UserPromptSubmit { prompt: String },
    /// A tool is about to be invoked. `tool` is the tool name; `arguments` is
    /// its raw JSON arguments string. Returning `Err` (a user hook's exit 2)
    /// BLOCKS the invocation — the tool does not run and the block reason is
    /// surfaced as the tool result.
    PreToolUse { tool: String, arguments: String },
    /// A tool has just returned. `result` is the tool's output string. This is
    /// observation-only — a returned `Err` is logged but cannot retroactively
    /// block an already-executed tool.
    PostToolUse {
        tool: String,
        arguments: String,
        result: String,
    },
    /// The agent run is stopping (completed / failed / aborted).
    Stop { summary: String },
}

/// A hook. `before` may veto; `after` observes completed actions; `on_event`
/// observes run-lifecycle events (turn start / tool call / run stop) and may
/// RETURN additional context strings (e.g. a `UserPromptSubmit` hook that emits
/// project conventions on stdout) OR refuse the event with `Err` (a user hook's
/// exit 2): on `UserPromptSubmit`/`PreToolUse` the refusal BLOCKS (the turn / the
/// tool call is refused); on `Stop`/`PostToolUse` it is logged but has no effect
/// (you can't retroactively stop a run or un-execute a tool).
#[async_trait]
pub trait Hook: Send + Sync {
    fn name(&self) -> &str;
    async fn before(&self, _action: &Action) -> Result<(), BlockReason> {
        Ok(())
    }
    async fn after(&self, _outcome: &ActionOutcome) -> Vec<HonestyWarning> {
        Vec::new()
    }
    /// Observe a lifecycle event. Returns additional context strings to inject
    /// into the run (only meaningful for `UserPromptSubmit`), or `Err` to refuse
    /// the event — the caller decides whether the refusal blocks (Submit /
    /// PreToolUse) or is ignored (Stop / PostToolUse). Default `Ok(empty)` so
    /// built-in hooks stay source-compatible; the D2 `UserCommandHook` overrides
    /// this, mapping the command's exit code (0→context, 2→block, else→warn).
    async fn on_event(&self, _ev: &HookEvent) -> Result<Vec<String>, BlockReason> {
        Ok(Vec::new())
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
        // Catastrophe floor — irreversible system-destruction runs in ALL modes,
        // including SkipPermissions (yolo). `rm -rf /`, `mkfs`, `dd` to a block
        // device, fork bombs, and shutdown/poweroff are not "permissions" a yolo
        // mode can waive, so this runs BEFORE the per-mode short-circuit below
        // (mirrors `sudo --no-preserve-root`: bypass modes keep a hard floor on
        // irrevocable ops). Normal command guards + the allowlist still only
        // apply in non-yolo modes via the registered CommandGuardHook loop.
        match action {
            Action::RunCommand { command } => {
                if let Some(reason) = classify_catastrophe(command) {
                    return Err(reason);
                }
            }
            Action::CallTool { tool, arguments } => {
                // M7: also guard tool calls whose name suggests shell execution
                // (same surface as CommandGuardHook, so a `bash` tool call can't
                // smuggle `mkfs` past yolo either).
                let dangerous_tools = ["exec", "shell", "bash", "cmd", "powershell", "sh"];
                if dangerous_tools
                    .iter()
                    .any(|dt| tool.to_lowercase().contains(dt))
                {
                    if let Some(cmd) = extract_command_from_args(arguments) {
                        if let Some(reason) = classify_catastrophe(&cmd) {
                            return Err(reason);
                        }
                    }
                }
            }
            _ => {}
        }
        // Permission-mode short-circuit (runs before per-hook dispatch):
        // skip-permissions bypasses everything [else]; plan mode blocks writes
        // and command execution until the user confirms the plan.
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

    /// Dispatch a lifecycle event to every hook's `on_event`, collecting the
    /// additional-context strings each hook returns. Returns `Err` on the FIRST
    /// hook that refuses (a user hook's exit 2) — short-circuiting so a block is
    /// honored immediately and no later hook runs for that event. The caller
    /// decides whether the refusal blocks (Submit / PreToolUse) or is ignored
    /// (Stop / PostToolUse). A hook that merely warns (non-2 exit) logs inside
    /// `on_event` and returns `Ok(empty)`, so it never short-circuits.
    pub async fn dispatch_event(&self, ev: &HookEvent) -> Result<Vec<String>, BlockReason> {
        let mut contexts = Vec::new();
        for h in &self.hooks {
            match h.on_event(ev).await {
                Ok(ctx) => contexts.extend(ctx),
                Err(reason) => return Err(reason),
            }
        }
        Ok(contexts)
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

/// Irreversible system-destruction check — the hard floor that runs in ALL
/// permission modes, including [`PermissionMode::SkipPermissions`] (yolo).
/// `rm -rf /`, `mkfs`, `dd` to a block device, fork bombs, and
/// shutdown/poweroff are not "permissions" a yolo mode can waive: they destroy
/// the user's system or data irrecoverably. This mirrors the consensus behind
/// `sudo --no-preserve-root` — bypass modes still keep a hard floor on
/// irrevocable ops.
///
/// Unlike [`CommandGuardHook::classify`], this does NOT consult the allowlist:
/// the catastrophe floor is not admin-bypassable.
/// [`HookManager::before`] calls this BEFORE the per-mode `skips_guards`
/// short-circuit, so it applies even under SkipPermissions.
fn classify_catastrophe(command: &str) -> Option<BlockReason> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    let prog = tokens[0].to_lowercase();
    let joined = command.to_lowercase();

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
                let dangerous = t == "/"
                    || t == "~"
                    || t == "/*"
                    || t == "/home"
                    || t == "/usr"
                    || t == "/bin"
                    || t == "/etc"
                    || t == "/var"
                    || t == "/boot"
                    || t.starts_with("/dev/sd")
                    || t.starts_with("/dev/nvme");
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

    // Filesystem format / disk wipe. `mkfs` and its typed variants
    // (mkfs.ext4 / mkfs.ntfs / mkfs.vfat / ...) all unconditionally format.
    if prog == "mkfs"
        || prog.starts_with("mkfs.")
        || joined.contains("dd if=/dev/zero of=/dev/")
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

/// Reject shell commands that are genuinely destructive. Uses token-based
/// detection (the Forge bash-guard analog) instead of naive substring matching,
/// so `rm -rf /home/user/old-build` (legitimate) is NOT blocked while
/// `rm -rf /` (wipe root) IS.
#[derive(Default)]
pub struct CommandGuardHook {
    /// User-configurable allowlist of command prefixes that bypass the guard.
    allowlist: Vec<String>,
}

impl CommandGuardHook {
    pub fn with_allowlist(allowlist: Vec<String>) -> Self {
        Self { allowlist }
    }

    /// Returns a BlockReason if the command is dangerous, else None.
    /// Token-based: splits on whitespace, inspects the program + flags + target.
    fn classify(&self, command: &str) -> Option<BlockReason> {
        let joined = command.to_lowercase();
        // Allowlist bypass (does NOT apply to the catastrophe floor — an admin
        // allowlist cannot rescue a system wipe; see classify_catastrophe,
        // which HookManager runs unconditionally in ALL modes before this
        // registered hook). This path only runs in non-yolo modes.
        for allowed in &self.allowlist {
            if joined.starts_with(&allowed.to_lowercase()) {
                return None;
            }
        }
        classify_catastrophe(command)
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

/// Gate file writes by Forge-task boundary (the Forge task-guard analog). A
/// session bound to a task (`task_ref`) may write inside its `working_dir`;
/// writes OUTSIDE that dir are blocked (the agent escaped its task scope); a
/// session with NO task only warns — it never bricks the agent's own
/// file-writing tools. Previously deferred because the chat path carried no
/// task state, so a hard Block on every taskless WriteFile would have refused
/// all writes; the v11→v12 migration + task_ref threading now let the bound
/// case enforce scope while the unbound case degrades to a warning.
pub struct TaskGuardHook {
    task_ref: Option<String>,
    working_dir: Option<std::path::PathBuf>,
}

impl TaskGuardHook {
    pub fn new(task_ref: Option<String>, working_dir: Option<std::path::PathBuf>) -> Self {
        Self {
            task_ref,
            working_dir,
        }
    }
}

impl Default for TaskGuardHook {
    fn default() -> Self {
        Self::new(None, None)
    }
}

#[async_trait]
impl Hook for TaskGuardHook {
    fn name(&self) -> &str {
        "task_guard"
    }
    async fn before(&self, action: &Action) -> Result<(), BlockReason> {
        let Action::WriteFile { path, .. } = action else {
            return Ok(());
        };
        match (&self.task_ref, &self.working_dir) {
            // No task bound: WARN but allow. Severity::Warn is logged by
            // HookManager::before and does NOT block — the agent keeps working.
            (None, _) => Err(BlockReason {
                hook: self.name().into(),
                message: "file write without an active task (start one via `forge task start`)"
                    .into(),
                severity: Severity::Warn,
            }),
            // Task bound but working_dir unknown: can't bound-check, allow.
            (Some(_), None) => Ok(()),
            // Task bound + working_dir: writes INSIDE pass, OUTSIDE are blocked.
            (Some(_), Some(dir)) => {
                if is_within(path, dir) {
                    Ok(())
                } else {
                    Err(BlockReason {
                        hook: self.name().into(),
                        message: format!(
                            "file write outside task scope: {path} is not within {}",
                            dir.display()
                        ),
                        severity: Severity::Block,
                    })
                }
            }
        }
    }
}

/// Is `path` inside `dir`? Two strategies, authoritative first:
///
/// 1. If the target EXISTS on disk, `canonicalize` both sides and compare the
///    canonical forms. This resolves `.`, `..`, AND symlinks — the only way to
///    defeat `/proj/./../../etc/passwd` escapes and symlink redirects whose
///    target leaves the project.
/// 2. If the target does NOT exist (the common case: the agent is writing a NEW
///    file whose path isn't on disk yet), canonicalize can't help, so we
///    LEXICALLY normalize both paths — collapse `.` and `..` components WITHOUT
///    touching the filesystem — then prefix-check. This closes the old hole
///    where a raw `strip_prefix` saw `/proj/../../etc` as "starting with /proj"
///    and returned `true`, letting the agent write outside the project by
///    embedding `..`. Parent-dir symlinks are NOT resolved in this branch (an
///    accepted limitation: a symlink inside the project whose target escapes is
///    caught once the file exists and path 1 runs on a later call).
///
/// An absolute `path` is checked directly; a relative one is joined onto `dir`.
fn is_within(path: &str, dir: &std::path::Path) -> bool {
    let p = std::path::Path::new(path);
    let target = if p.is_absolute() {
        p.to_path_buf()
    } else {
        dir.join(p)
    };
    // 1. Authoritative: both sides exist → canonicalize resolves .. and symlinks.
    if let (Ok(t), Ok(d)) = (std::fs::canonicalize(&target), std::fs::canonicalize(dir)) {
        return t.strip_prefix(&d).is_ok() || t == d;
    }
    // 2. Fallback: target doesn't exist yet → lexical normalize, then prefix-check.
    let norm_target = lexical_normalize(&target);
    let norm_dir = lexical_normalize(dir);
    norm_target.strip_prefix(&norm_dir).is_ok() || norm_target == norm_dir
}

/// Lexically normalize a path: resolve `.` (drop) and `..` (pop the last
/// component) WITHOUT touching the filesystem, so `/proj/../../etc` collapses
/// to `/etc`. Used by [`is_within`] when the target doesn't exist yet
/// (canonicalize fails). A `..` that would escape above the root is a no-op
/// (`pop` on the root stays at the root) — correct for confinement: `/../etc`
/// normalizes to `/etc`, never above.
fn lexical_normalize(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::{Component, PathBuf};
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
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
            action: Action::WriteFile {
                path: "t.rs".into(),
                content_preview: "".into(),
            },
            ok: true,
            diff: Some(diff.into()),
            error: None,
        };
        let findings = h.after(&outcome).await;
        assert!(findings.iter().any(|f| f.rule == "fatal_to_log"));
    }

    fn write(path: &str) -> Action {
        Action::WriteFile {
            path: path.into(),
            content_preview: "".into(),
        }
    }

    #[tokio::test]
    async fn task_guard_warns_write_without_task() {
        // No task_ref → Severity::Warn (logged, does NOT block). This is the
        // "never brick" guarantee: a taskless session still writes files.
        let h = TaskGuardHook::new(None, None);
        let err = h.before(&write("/proj/x.rs")).await.unwrap_err();
        assert_eq!(
            err.severity,
            Severity::Warn,
            "taskless write must warn, not block"
        );
    }

    #[tokio::test]
    async fn task_guard_allows_write_inside_working_dir() {
        let h = TaskGuardHook::new(
            Some("feat/x".into()),
            Some(std::path::PathBuf::from("/proj")),
        );
        assert!(h.before(&write("/proj/src/main.rs")).await.is_ok());
    }

    #[tokio::test]
    async fn task_guard_blocks_write_outside_working_dir() {
        // Task bound to /proj but writing /etc/passwd → Block (escaped scope).
        let h = TaskGuardHook::new(
            Some("feat/x".into()),
            Some(std::path::PathBuf::from("/proj")),
        );
        let err = h.before(&write("/etc/passwd")).await.unwrap_err();
        assert_eq!(err.severity, Severity::Block);
        assert!(err.message.contains("outside task scope"), "got: {err:?}");
    }

    #[tokio::test]
    async fn task_guard_allows_when_working_dir_unknown() {
        // Task bound but no working_dir → can't bound-check, allow.
        let h = TaskGuardHook::new(Some("feat/x".into()), None);
        assert!(h.before(&write("/anywhere/x.rs")).await.is_ok());
    }

    #[tokio::test]
    async fn task_guard_ignores_non_write_actions() {
        let h = TaskGuardHook::new(None, None);
        assert!(h.before(&cmd("cargo build")).await.is_ok());
    }

    #[tokio::test]
    async fn task_guard_blocks_dotdot_path_escape() {
        // Regression: `/proj/../../etc/passwd` lexically STARTS WITH `/proj`, so
        // the OLD is_within (raw strip_prefix short-circuit) returned true → the
        // agent could write anywhere by embedding `..`. lexical_normalize must
        // collapse it to `/etc/passwd` → outside /proj → blocked.
        let h = TaskGuardHook::new(
            Some("feat/x".into()),
            Some(std::path::PathBuf::from("/proj")),
        );
        let err = h
            .before(&write("/proj/../../etc/passwd"))
            .await
            .unwrap_err();
        assert_eq!(
            err.severity,
            Severity::Block,
            "dotdot escape must be blocked: {err:?}"
        );
    }

    #[tokio::test]
    async fn task_guard_allows_legitimate_dotdot_within_scope() {
        // A `..` that STAYS inside the project (`/proj/sub/../file.rs` collapses
        // to `/proj/file.rs`) must remain allowed — the fix must not over-block
        // ordinary relative navigation within the working dir.
        let h = TaskGuardHook::new(
            Some("feat/x".into()),
            Some(std::path::PathBuf::from("/proj")),
        );
        assert!(
            h.before(&write("/proj/sub/../file.rs")).await.is_ok(),
            "in-scope dotdot must be allowed"
        );
    }

    #[tokio::test]
    async fn task_guard_blocks_dotdot_with_dot_segments() {
        // `/proj/./../../etc` mixes `.` and `..` — both must collapse so the
        // escape is caught (the old code's lexical `strip_prefix` saw the `/proj`
        // prefix and returned true before canonicalize ever ran).
        let h = TaskGuardHook::new(
            Some("feat/x".into()),
            Some(std::path::PathBuf::from("/proj")),
        );
        let err = h
            .before(&write("/proj/./../../etc/shadow"))
            .await
            .unwrap_err();
        assert_eq!(
            err.severity,
            Severity::Block,
            "mixed ./.. escape must be blocked: {err:?}"
        );
    }

    #[test]
    fn lexical_normalize_collapses_dot_and_dotdot() {
        // `/proj/../../etc/passwd` must collapse BELOW /proj (escaped).
        let escaped = lexical_normalize(std::path::Path::new("/proj/../../etc/passwd"));
        assert!(
            !escaped.starts_with("/proj"),
            "escaped path {escaped:?} must not remain under /proj"
        );
        // `/proj/./sub/../file.rs` must collapse to inside /proj.
        let inside = lexical_normalize(std::path::Path::new("/proj/./sub/../file.rs"));
        assert!(
            inside.starts_with("/proj"),
            "in-scope path {inside:?} must remain under /proj"
        );
    }

    #[tokio::test]
    async fn dispatch_event_reaches_hook_on_event() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        struct EventSpy {
            submits: Arc<AtomicUsize>,
            stops: Arc<AtomicUsize>,
        }
        #[async_trait]
        impl Hook for EventSpy {
            fn name(&self) -> &str {
                "event_spy"
            }
            async fn on_event(&self, ev: &HookEvent) -> Result<Vec<String>, BlockReason> {
                match ev {
                    HookEvent::UserPromptSubmit { .. } => {
                        self.submits.fetch_add(1, Ordering::SeqCst);
                    }
                    HookEvent::PreToolUse { .. } | HookEvent::PostToolUse { .. } => {}
                    HookEvent::Stop { .. } => {
                        self.stops.fetch_add(1, Ordering::SeqCst);
                    }
                }
                Ok(Vec::new())
            }
        }
        let submits = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let mut mgr = HookManager::new();
        mgr.register(Box::new(EventSpy {
            submits: submits.clone(),
            stops: stops.clone(),
        }));
        let _ = mgr
            .dispatch_event(&HookEvent::UserPromptSubmit {
                prompt: "hi".into(),
            })
            .await;
        let _ = mgr
            .dispatch_event(&HookEvent::UserPromptSubmit {
                prompt: "again".into(),
            })
            .await;
        let _ = mgr
            .dispatch_event(&HookEvent::Stop {
                summary: "done".into(),
            })
            .await;
        assert_eq!(submits.load(Ordering::SeqCst), 2);
        assert_eq!(stops.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dispatch_event_collects_contexts_from_hooks() {
        // A hook returning Vec<String> on UserPromptSubmit → dispatch_event
        // must gather them so the run loop can inject the joined context.
        struct ContextHook {
            ctx: String,
        }
        #[async_trait]
        impl Hook for ContextHook {
            fn name(&self) -> &str {
                "context_hook"
            }
            async fn on_event(&self, ev: &HookEvent) -> Result<Vec<String>, BlockReason> {
                Ok(if let HookEvent::UserPromptSubmit { .. } = ev {
                    vec![self.ctx.clone()]
                } else {
                    Vec::new()
                })
            }
        }
        let mut mgr = HookManager::new();
        mgr.register(Box::new(ContextHook {
            ctx: "rule-A".into(),
        }));
        mgr.register(Box::new(ContextHook {
            ctx: "rule-B".into(),
        }));
        // Stop dispatch must yield nothing (neither hook returns on Stop).
        let stop_ctxs = mgr
            .dispatch_event(&HookEvent::Stop {
                summary: "x".into(),
            })
            .await
            .unwrap();
        assert!(
            stop_ctxs.is_empty(),
            "Stop yields no context: {stop_ctxs:?}"
        );
        // Submit dispatch must gather BOTH hooks' contexts in registration order.
        let submit_ctxs = mgr
            .dispatch_event(&HookEvent::UserPromptSubmit { prompt: "p".into() })
            .await
            .unwrap();
        assert_eq!(
            submit_ctxs,
            vec!["rule-A".to_string(), "rule-B".to_string()]
        );
    }

    #[tokio::test]
    async fn dispatch_event_short_circuits_on_block() {
        // v2: a hook returning Err (user-hook exit 2) must short-circuit — the
        // FIRST blocking hook ends dispatch, later hooks do NOT run for that
        // event, and the caller receives Err to honor the block.
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        struct BlockHook;
        #[async_trait]
        impl Hook for BlockHook {
            fn name(&self) -> &str {
                "block_hook"
            }
            async fn on_event(&self, ev: &HookEvent) -> Result<Vec<String>, BlockReason> {
                if let HookEvent::UserPromptSubmit { prompt } = ev {
                    if prompt.contains("forbidden") {
                        return Err(BlockReason {
                            hook: "block_hook".into(),
                            message: "prompt contained forbidden token".into(),
                            severity: Severity::Block,
                        });
                    }
                }
                Ok(Vec::new())
            }
        }
        let ran = Arc::new(AtomicUsize::new(0));
        struct Counter(Arc<AtomicUsize>);
        #[async_trait]
        impl Hook for Counter {
            fn name(&self) -> &str {
                "counter"
            }
            async fn on_event(&self, _ev: &HookEvent) -> Result<Vec<String>, BlockReason> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(Vec::new())
            }
        }
        let mut mgr = HookManager::new();
        mgr.register(Box::new(BlockHook));
        mgr.register(Box::new(Counter(ran.clone())));
        // A forbidden prompt: BlockHook short-circuits BEFORE Counter runs.
        let res = mgr
            .dispatch_event(&HookEvent::UserPromptSubmit {
                prompt: "forbidden thing".into(),
            })
            .await;
        let reason = res.expect_err("block must surface as Err");
        assert_eq!(reason.severity, Severity::Block);
        assert_eq!(
            ran.load(Ordering::SeqCst),
            0,
            "later hook must not run after a block"
        );
        // A clean prompt: no block, Counter runs.
        let res = mgr
            .dispatch_event(&HookEvent::UserPromptSubmit {
                prompt: "ok".into(),
            })
            .await
            .unwrap();
        assert!(res.is_empty());
        assert_eq!(
            ran.load(Ordering::SeqCst),
            1,
            "counter runs on a clean prompt"
        );
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
            action: Action::WriteFile {
                path: "t.rs".into(),
                content_preview: "".into(),
            },
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
        let write = Action::WriteFile {
            path: "x.rs".into(),
            content_preview: "".into(),
        };
        let run = Action::RunCommand {
            command: "echo hi".into(),
        };
        let tool = Action::CallTool {
            tool: "read".into(),
            arguments: "{}".into(),
        };
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
        let write = Action::WriteFile {
            path: "x.rs".into(),
            content_preview: "".into(),
        };
        let run = Action::RunCommand {
            command: "rm -rf x".into(),
        };
        let tool = Action::CallTool {
            tool: "write".into(),
            arguments: "{}".into(),
        };
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
        let write = Action::WriteFile {
            path: "x.rs".into(),
            content_preview: "".into(),
        };
        assert!(mgr.before(&write).await.is_ok());
    }

    #[tokio::test]
    async fn executing_mode_lets_writes_through_and_is_detectable() {
        // Executing (Mission Phase 2) does NOT gate writes at the hook layer —
        // the restriction is structural (read-only + dispatch_subagent tool
        // subset), not a permission veto. The master needs write_file to flip
        // prd.json `passes`, mirroring QwenPaw keeping write_file available in
        // Phase 2. is_executing flags the phase distinctly from Default/Plan.
        let mgr = HookManager::new().with_mode(PermissionMode::Executing);
        let write = Action::WriteFile {
            path: "prd.json".into(),
            content_preview: "".into(),
        };
        assert!(mgr.before(&write).await.is_ok());
        assert!(PermissionMode::Executing.is_executing());
        assert!(!PermissionMode::Plan.is_executing());
        assert!(!PermissionMode::Default.is_executing());
    }

    #[test]
    fn non_plan_modes_do_not_block_writes() {
        let write = Action::WriteFile {
            path: "x.rs".into(),
            content_preview: "".into(),
        };
        for m in [
            PermissionMode::Default,
            PermissionMode::AutoEdit,
            PermissionMode::Executing,
            PermissionMode::DryRun,
            PermissionMode::Silent,
            PermissionMode::SkipPermissions,
        ] {
            assert!(
                m.blocks_action(&write).is_none(),
                "{m:?} should not block writes"
            );
        }
    }

    #[test]
    fn skips_guards_only_for_skip_permissions() {
        assert!(PermissionMode::SkipPermissions.skips_guards());
        for m in [
            PermissionMode::Default,
            PermissionMode::AutoEdit,
            PermissionMode::Plan,
            PermissionMode::Executing,
            PermissionMode::DryRun,
            PermissionMode::Silent,
        ] {
            assert!(!m.skips_guards(), "{m:?} should not skip guards");
        }
    }

    #[tokio::test]
    async fn plan_mode_manager_blocks_write_before_hooks() {
        let mgr = HookManager::new().with_mode(PermissionMode::Plan);
        let write = Action::WriteFile {
            path: "x.rs".into(),
            content_preview: "".into(),
        };
        let err = mgr.before(&write).await.unwrap_err();
        assert_eq!(err.severity, Severity::Block);
        assert_eq!(err.hook, "permission_mode");
    }

    #[tokio::test]
    async fn skip_permissions_bypasses_normal_command_guard() {
        // yolo still skips the NORMAL command guard: a non-catastrophic command
        // (legitimate `rm -rf` target, not a system path) passes without prompt.
        let mut mgr = HookManager::new().with_mode(PermissionMode::SkipPermissions);
        mgr.register(Box::new(CommandGuardHook::default()));
        assert!(mgr
            .before(&cmd("rm -rf /home/user/old-build"))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn skip_permissions_still_blocks_catastrophe() {
        // The catastrophe floor runs even under yolo: `rm -rf /` is blocked
        // regardless of mode. "skip permissions" ≠ "allow system wipe".
        let mut mgr = HookManager::new().with_mode(PermissionMode::SkipPermissions);
        mgr.register(Box::new(CommandGuardHook::default()));
        let err = mgr.before(&cmd("rm -rf /")).await.unwrap_err();
        assert_eq!(err.severity, Severity::Block);
        assert!(err.message.contains("system path"));
    }

    #[tokio::test]
    async fn skip_permissions_blocks_catastrophe_via_bash_tool() {
        // A `bash` tool call smuggling `mkfs` cannot escape the catastrophe floor
        // under yolo either (CallTool surface, not just RunCommand).
        let mut mgr = HookManager::new().with_mode(PermissionMode::SkipPermissions);
        mgr.register(Box::new(CommandGuardHook::default()));
        let action = Action::CallTool {
            tool: "bash".into(),
            arguments: r#"{"command":"mkfs.ext4 /dev/sda1"}"#.into(),
        };
        assert!(mgr.before(&action).await.is_err());
    }

    #[tokio::test]
    async fn catastrophe_floor_blocks_in_all_modes() {
        // The hard floor is mode-independent: every permission mode blocks
        // `rm -rf /`. yolo does not weaken irrevocable-op prevention.
        for mode in [
            PermissionMode::Default,
            PermissionMode::AutoEdit,
            PermissionMode::Plan,
            PermissionMode::Executing,
            PermissionMode::DryRun,
            PermissionMode::Silent,
            PermissionMode::SkipPermissions,
        ] {
            let mut mgr = HookManager::new().with_mode(mode);
            mgr.register(Box::new(CommandGuardHook::default()));
            assert!(
                mgr.before(&cmd("rm -rf /")).await.is_err(),
                "rm -rf / must be blocked in {mode:?}"
            );
        }
    }

    #[tokio::test]
    async fn allowlist_cannot_rescue_catastrophe() {
        // The catastrophe floor ignores the allowlist — an admin allowlist of
        // `rm -rf /` cannot rescue a system wipe (hard floor, not a permission).
        let mut mgr = HookManager::new().with_mode(PermissionMode::Default);
        mgr.register(Box::new(CommandGuardHook::with_allowlist(vec![
            "rm -rf /".into()
        ])));
        assert!(
            mgr.before(&cmd("rm -rf /")).await.is_err(),
            "allowlist must not bypass the catastrophe floor"
        );
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
            ("executing", PermissionMode::Executing),
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
