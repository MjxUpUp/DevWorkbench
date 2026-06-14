//! `Executor` implementation routing graph nodes to DevWorkbench subsystems.
//!
//! - Agent nodes → `spawn_pty_agent` (opaque CLI), then await completion by
//!   polling the session row in SQLite (the existing wait-thread updates it).
//! - Gate nodes → `quality::forge::run_forge_gate` (or HonestyVerifier for the
//!   "honesty" gate).

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use kernel_compose::graph::{AgentChunk, AgentNodeSpec, Executor, GateNode};
use kernel_core::AgentInput;
use serde_json::{json, Value};

use crate::agents::pty::AgentProcesses;
use crate::db::DbState;
use crate::error::AppError;
use crate::kernel_impl::opaque_agent::OpaqueAgent;
use crate::kernel_impl::react_agent::{GlmChatModel, ReactAgent, ToolRegistry};
use crate::models::AgentType;
use crate::quality;

/// The host Executor — bridges kernel-compose graph nodes to real subsystems.
pub struct KernelExecutor {
    app: tauri::AppHandle,
    processes: Arc<AgentProcesses>,
    db: DbState,
}

impl KernelExecutor {
    pub fn new(app: tauri::AppHandle, processes: Arc<AgentProcesses>, db: DbState) -> Self {
        Self { app, processes, db }
    }
}

#[async_trait]
impl Executor for KernelExecutor {
    fn run_agent(
        &self,
        spec: &AgentNodeSpec,
        input: Value,
        working_dir: Option<String>,
    ) -> Result<BoxStream<'static, Result<AgentChunk, String>>, String> {
        let prompt = spec
            .prompt
            .clone()
            .or_else(|| input.as_str().map(String::from))
            .ok_or_else(|| "agent node has no prompt".to_string())?;

        let project_path = working_dir.clone().unwrap_or_else(|| ".".into());
        let agent_input = AgentInput {
            prompt,
            working_dir: Some(project_path),
            model: spec.model.clone(),
            resume_from: spec.resume_from.clone(),
        };

        // Dual-mode dispatch: a known CLI spec resolves to an opaque agent
        // (claude/codex/gemini/…); anything else is a transparent ReactAgent
        // (self-built, kernel-controlled LLM + tool loop). Both implement
        // kernel_core::Agent and produce the same AgentEvent stream, which we
        // map onto AgentChunk for the graph runner. This closes the gap where
        // the executor bypassed OpaqueAgent/ReactAgent entirely and spawned
        // the CLI directly + polled the DB.
        let agent: Box<dyn kernel_core::Agent> = match AgentType::from_spec(&spec.agent) {
            Some(at) => Box::new(OpaqueAgent::new(
                self.app.clone(),
                self.processes.clone(),
                self.db.clone(),
                at,
            )),
            None => Box::new(build_react_agent(spec.model.as_deref())?),
        };

        let event_stream = agent.run(agent_input).map_err(|e| e.to_string())?;
        Ok(Box::pin(map_agent_to_chunks(event_stream)))
    }

    async fn run_gate(
        &self,
        gate: &GateNode,
        input: Value,
        working_dir: Option<String>,
    ) -> Result<Value, String> {
        let project = working_dir.unwrap_or_else(|| ".".into());
        let path = std::path::Path::new(&project);

        match gate.gate.as_str() {
            "forge" => {
                let path_clone = path.to_path_buf();
                let result = tokio::task::spawn_blocking(move || {
                    quality::forge::run_forge_gate(&path_clone)
                })
                .await
                .map_err(|e| format!("forge join: {e}"))?;
                let json_val = match result {
                    Ok(report) => serde_json::to_value(&report)
                        .unwrap_or_else(|_| json!({"status": "unknown"})),
                    Err(AppError::ForgeNotInstalled) => {
                        // Graceful skip — forge missing is not a graph failure.
                        json!({"gate": "forge", "status": "skipped", "note": "forge not installed"})
                    }
                    Err(e) => return Err(e.to_string()),
                };
                Ok(json_val)
            }
            "honesty" => {
                // Real HonestyVerifier wiring (previously a hardcoded "passed"):
                // scan uncommitted git diff for assertion weakening, sanity-check
                // the compile environment (Rust projects), and cross-check the
                // agent's completion claim against the captured proof.
                let claim = input.as_str().unwrap_or("").to_string();
                let project_path = path.to_path_buf();
                let result = tokio::task::spawn_blocking(move || {
                    run_honesty_gate(&project_path, &claim)
                })
                .await
                .map_err(|e| format!("honesty join: {e}"))?;
                Ok(result)
            }
            other => Err(format!("unknown gate '{other}'")),
        }
    }
}

// ---------------------------------------------------------------------------
// Dual-mode dispatch helpers
// ---------------------------------------------------------------------------

/// Build a transparent ReactAgent. Task 3 (`fix/providers-glm46`) wires the real
/// providers.toml api_key plumbing; for now the key is empty (GLM calls fail at
/// request time until the config is filled). The default model is `glm-4.6` —
/// the strongest tool-calling GLM on the Anthropic-compatible endpoint —
/// overridable via `spec.model`. Flagship models stay on the opaque path
/// (claude/codex/gemini) where the user selects them.
fn build_react_agent(model: Option<&str>) -> Result<ReactAgent, String> {
    let model_id = model.unwrap_or("glm-4.6").to_string();
    let chat = GlmChatModel::bigmodel(String::new(), model_id);
    Ok(ReactAgent::new(
        chat,
        ToolRegistry::new(),
        "You are a Dev Workbench kernel agent. Complete the task concisely.",
    ))
}

/// Map a kernel-core `AgentEvent` stream onto graph `AgentChunk`s:
/// - `Token` → `Delta` (forwarded to the frontend as `NodeOutput`)
/// - `Done`  → `Final` (becomes the node's output value, propagated to successors)
/// - `ToolCall`/`FileChanged`/`TurnBoundary` → dropped (the runner only
///   forwards textual deltas; these are agent-internal observations)
/// - `Err`   → stream error (fails the node)
fn map_agent_to_chunks(
    events: BoxStream<'static, Result<kernel_core::AgentEvent, kernel_core::Error>>,
) -> impl futures::Stream<Item = Result<AgentChunk, String>> {
    use futures::StreamExt;
    events.filter_map(|ev_res| async move {
        match ev_res {
            Ok(kernel_core::AgentEvent::Token(t)) => {
                Some(Ok(AgentChunk::Delta(Value::String(t))))
            }
            Ok(kernel_core::AgentEvent::Done(outcome)) => {
                Some(Ok(AgentChunk::Final(outcome_to_value(outcome))))
            }
            Ok(_) => None,
            Err(e) => Some(Err(e.to_string())),
        }
    })
}

/// Serialize an `AgentOutcome` as the node's final JSON value.
fn outcome_to_value(o: kernel_core::AgentOutcome) -> Value {
    json!({
        "status": format!("{:?}", o.status).to_lowercase(),
        "output": o.output_summary,
        "files_changed": o.files_changed,
        "exit_code": o.exit_code,
    })
}

// ---------------------------------------------------------------------------
// Honesty gate — real HonestyVerifier wiring
// ---------------------------------------------------------------------------

/// Run the honesty gate against a project directory.
///
/// Three checks (each carrying real evidence, not a paraphrase):
/// 1. `check_assertion_weakening` over the uncommitted `git diff HEAD`
///    (universal — works for any language whose assertions match the rules).
/// 2. `verify_env_sane` over `cargo check` output (Rust projects only).
/// 3. `require_proof_of_completion` cross-checking the agent's `claim` against
///    the captured compile output (Rust projects only).
///
/// `status` is `"failed"` if any Error-severity finding surfaces, else `"passed"`.
fn run_honesty_gate(project: &std::path::Path, claim: &str) -> Value {
    use crate::kernel_impl::honesty::{
        check_assertion_weakening, parse_diff, require_proof_of_completion, verify_env_sane,
        Severity as HonestySeverity,
    };

    let mut findings = Vec::new();

    // 1. Assertion weakening from uncommitted changes.
    let diff_text = git_diff_text(project);
    if !diff_text.is_empty() {
        findings.extend(check_assertion_weakening(&parse_diff(&diff_text)));
    }

    // 2 & 3. Env sanity + claim-vs-proof (Rust projects only — non-Rust dirs
    //    have no `cargo check` to run; skipping is honest, not a free pass).
    if project.join("Cargo.toml").exists() {
        let check_out = cargo_check_text(project);
        if let Err(w) = verify_env_sane(&check_out) {
            findings.push(w);
        }
        if !claim.is_empty() {
            if let Err(w) = require_proof_of_completion(claim, &check_out) {
                findings.push(w);
            }
        }
    }

    let has_error = findings
        .iter()
        .any(|f| f.severity == HonestySeverity::Error);
    json!({
        "gate": "honesty",
        "status": if has_error { "failed" } else { "passed" },
        "findings": findings,
        "finding_count": findings.len(),
    })
}

/// Capture `git diff HEAD` (staged + unstaged vs HEAD) as unified-diff text.
/// Returns empty on any failure (non-repo, git missing) — treated as "no
/// changes to inspect" rather than a false positive.
fn git_diff_text(project: &std::path::Path) -> String {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("diff").arg("HEAD").current_dir(project);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Run `cargo check` (short format) and return combined stdout+stderr for
/// honesty inspection. Empty on failure to invoke cargo.
fn cargo_check_text(project: &std::path::Path) -> String {
    let mut cmd = std::process::Command::new("cargo");
    cmd.arg("check")
        .arg("--message-format=short")
        .current_dir(project);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = match cmd.output() {
        Ok(o) => o,
        Err(_) => return String::new(),
    };
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    s
}

#[cfg(test)]
mod honesty_gate_tests {
    use super::*;

    /// A non-git, non-Rust directory has nothing to inspect → passed, no findings.
    #[test]
    fn clean_dir_passes_honesty_gate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let res = run_honesty_gate(tmp.path(), "done");
        assert_eq!(res["status"], "passed");
        assert_eq!(res["finding_count"].as_u64(), Some(0));
    }

    /// A real assertion weakening (`t.Fatal` → `t.Log`) in an uncommitted diff
    /// must flip the gate to `failed` with a non-zero finding count.
    #[test]
    fn assertion_weakening_fails_honesty_gate() {
        use std::process::Command;
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        // Bootstrap a git repo (user config so `commit` works headlessly).
        let setups: &[&[&str]] = &[
            &["init"],
            &["config", "user.email", "t@t.t"],
            &["config", "user.name", "t"],
        ];
        for args in setups {
            let mut c = Command::new("git");
            c.args(*args).current_dir(root);
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                c.creation_flags(0x0800_0000);
            }
            assert!(c.status().map(|s| s.success()).unwrap_or(false), "git {args:?} failed");
        }

        // Baseline committed file with a strong assertion.
        let f = root.join("t_test.rs");
        std::fs::write(&f, "func()\nt.Fatal(\"x\")\n").unwrap();
        let add = Command::new("git").args(["add", "."]).current_dir(root).status();
        let commit = Command::new("git")
            .args(["commit", "-m", "base", "--allow-empty"])
            .current_dir(root)
            .status();
        assert!(add.map(|s| s.success()).unwrap_or(false), "git add failed");
        assert!(commit.map(|s| s.success()).unwrap_or(false), "git commit failed");

        // Weakening change, left uncommitted → `git diff HEAD` sees it.
        std::fs::write(&f, "func()\nt.Log(\"x\")\n").unwrap();

        let res = run_honesty_gate(root, "all tests pass");
        assert_eq!(
            res["status"], "failed",
            "t.Fatal→t.Log weakening must fail honesty gate: {res}"
        );
        assert!(
            res["finding_count"].as_u64().unwrap_or(0) > 0,
            "expected at least one finding: {res}"
        );
    }
}
