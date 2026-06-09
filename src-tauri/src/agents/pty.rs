use crate::models::{AgentType, ContextSnapshot, Session, SessionStatus};
use std::collections::HashMap;
use std::io::Read;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tauri::Emitter;

/// Max chars kept as output summary (truncated from tail of output)
const OUTPUT_SUMMARY_MAX_CHARS: usize = 2000;

/// Active agent processes tracked by session ID.
pub struct AgentProcesses {
    processes: Mutex<HashMap<String, u32>>,
}

impl AgentProcesses {
    pub fn new() -> Self {
        Self {
            processes: Mutex::new(HashMap::new()),
        }
    }
}

/// On Windows, resolve the real .exe for npm-installed CLI tools.
/// `which::which` may return a shell script wrapper without .exe extension,
/// which `std::process::Command` can handle but we prefer the real exe.
#[cfg(target_os = "windows")]
fn resolve_agent_exe(agent_type: &AgentType) -> Result<std::path::PathBuf, String> {
    let path = crate::commands::tools::which_expanded(agent_type.command_name())
        .ok_or_else(|| format!("{} 未安装", agent_type.display_name()))?;

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext.eq_ignore_ascii_case("exe") {
        return Ok(path);
    }

    // Try common locations for the actual exe
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

    // Fallback: try with .exe extension in same directory
    let with_exe = path.with_extension("exe");
    if with_exe.exists() {
        return Ok(with_exe);
    }

    Ok(path)
}

#[cfg(not(target_os = "windows"))]
fn resolve_agent_exe(agent_type: &AgentType) -> Result<std::path::PathBuf, String> {
    crate::commands::tools::which_expanded(agent_type.command_name())
        .ok_or_else(|| format!("{} 未安装", agent_type.display_name()))
}

/// Build spawn command with agent-specific flags.
fn build_spawn_command(
    agent_type: &AgentType,
    project_path: &str,
    prompt: &str,
    model: Option<&str>,
    parent_session_id: Option<&str>,
) -> Result<std::process::Command, String> {
    let exe = resolve_agent_exe(agent_type)?;
    let mut cmd = std::process::Command::new(exe);

    cmd.current_dir(project_path);

    let is_continue = parent_session_id.is_some();

    match agent_type {
        AgentType::ClaudeCode => {
            cmd.arg("--print");
            if is_continue {
                cmd.arg("--continue");
            }
            cmd.arg(prompt);
            if let Some(m) = model {
                cmd.arg("--model").arg(m);
            }
        }
        AgentType::Codex => {
            cmd.arg("exec").arg(prompt);
            if let Some(m) = model {
                cmd.arg("--model").arg(m);
            }
        }
        AgentType::GeminiCli => {
            cmd.arg("--prompt").arg(prompt);
        }
        AgentType::CursorAgent => {
            cmd.arg("agent").arg("--prompt").arg(prompt);
        }
        AgentType::Copilot => {
            cmd.arg("copilot").arg("suggest").arg("--prompt").arg(prompt);
        }
        AgentType::QwenCode => {
            cmd.arg("--prompt").arg(prompt);
        }
        AgentType::Pi => {
            cmd.arg("--prompt").arg(prompt);
        }
    }

    Ok(cmd)
}

/// Spawn an agent process with piped I/O and stream output via Tauri events.
/// Uses `std::process::Command` with raw byte streaming (no PTY needed).
/// ANSI sequences are preserved for xterm.js rendering in the frontend.
pub fn spawn_pty_agent(
    app: &tauri::AppHandle,
    processes: Arc<AgentProcesses>,
    project_path: &str,
    agent_type: AgentType,
    prompt: &str,
    model: Option<&str>,
    linked_requirement_id: Option<&str>,
    parent_session_id: Option<&str>,
) -> Result<Session, String> {
    let session_id = uuid::Uuid::new_v4().to_string();

    let mut cmd = build_spawn_command(&agent_type, project_path, prompt, model, parent_session_id)?;

    // Configure stdio — pipe stdout/stderr for capture, no stdin
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    // On Windows, prevent console window from appearing
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动 {} 失败: {}", agent_type.display_name(), e))?;
    let pid = child.id();

    // Track the process
    processes
        .processes
        .lock()
        .unwrap()
        .insert(session_id.clone(), pid);

    // Create session record
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

    crate::agents::session::add_session(session.clone())?;
    let _ = app.emit("agent:started", &session);

    // --- Reader thread: stream raw bytes via Tauri events + write log ---
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let app_reader = app.clone();
    let sid_reader = session_id.clone();
    let processes_reader = processes.clone();
    std::thread::spawn(move || {
        let mut output_log: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];

        // Read stdout
        if let Some(mut stdout) = stdout {
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = &buf[..n];
                        output_log.extend_from_slice(data);
                        let bytes_vec: Vec<u8> = data.to_vec();
                        let _ = app_reader.emit(
                            "pty:output",
                            serde_json::json!({
                                "sessionId": sid_reader,
                                "data": bytes_vec,
                            }),
                        );
                    }
                    Err(_) => break,
                }
            }
        }

        // Read stderr
        if let Some(mut stderr) = stderr {
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = &buf[..n];
                        output_log.extend_from_slice(data);
                        let bytes_vec: Vec<u8> = data.to_vec();
                        let _ = app_reader.emit(
                            "pty:output",
                            serde_json::json!({
                                "sessionId": sid_reader,
                                "data": bytes_vec,
                            }),
                        );
                    }
                    Err(_) => break,
                }
            }
        }

        // Save raw output log (for outputSummary)
        if let Ok(agents_dir) = crate::agents::session::agents_dir() {
            let log_path = agents_dir.join("outputs");
            let _ = std::fs::create_dir_all(&log_path);
            let _ = std::fs::write(log_path.join(format!("{}.log", sid_reader)), &output_log);
        }

        // Remove from active processes
        if let Ok(mut map) = processes_reader.processes.lock() {
            map.remove(&sid_reader);
        }
    });

    // --- Wait thread: detect process exit ---
    let app_exit = app.clone();
    let sid_exit = session_id.clone();
    let project_path_exit = project_path.to_string();
    std::thread::spawn(move || {
        let status = child.wait();
        let (exit_code, session_status) = match status {
            Ok(s) => (
                s.code(),
                if s.success() {
                    SessionStatus::Completed
                } else {
                    SessionStatus::Failed
                },
            ),
            Err(_) => (None, SessionStatus::Failed),
        };

        // Give reader thread time to finish writing the log file
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Extract context snapshot from git diff
        let snapshot = extract_context_snapshot(&project_path_exit);

        // Read output summary from log
        let output_summary = read_output_summary(&sid_exit);

        // Update session record
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

        let _ = crate::agents::session::update_session(&sid_exit, patch.clone());
        let _ = app_exit.emit(
            "agent:completed",
            serde_json::json!({
                "sessionId": sid_exit,
                "status": session_status.as_str(),
                "exitCode": exit_code,
            }),
        );
    });

    Ok(session)
}

/// Write data to stdin (best-effort for pipe-based spawning; typically unused).
pub fn pty_write(
    _processes: &Arc<AgentProcesses>,
    _session_id: &str,
    _data: &str,
) -> Result<(), String> {
    // Pipe-based spawning doesn't support stdin input.
    // Agents run in non-interactive mode (--print, exec, etc.)
    Ok(())
}

/// Resize (no-op for pipe-based spawning).
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
    let map = processes.processes.lock().unwrap();
    if let Some(&pid) = map.get(session_id) {
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output();
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = std::process::Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .output();
        }
        Ok(())
    } else {
        Err(format!("Session {} 不在运行中", session_id))
    }
}

/// Read the output log for a session and return a truncated summary.
/// Strips ANSI escape sequences for a clean text summary.
fn read_output_summary(session_id: &str) -> Option<String> {
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
    if text.len() > OUTPUT_SUMMARY_MAX_CHARS {
        Some(format!("...{}", &text[text.len() - OUTPUT_SUMMARY_MAX_CHARS..]))
    } else {
        Some(text)
    }
}

/// Strip ANSI escape sequences from raw bytes, return clean UTF-8 string.
fn strip_ansi(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    while let Some(&ch) = chars.peek() {
                        chars.next();
                        if ('\x40'..='\x7e').contains(&ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(&ch) = chars.peek() {
                        chars.next();
                        if ch == '\x07' || ch == '\\' {
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn extract_context_snapshot(project_path: &str) -> Option<ContextSnapshot> {
    let output = std::process::Command::new("git")
        .args(["diff", "--stat", "--name-only"])
        .current_dir(project_path)
        .output()
        .ok()?;

    let files = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect::<Vec<_>>();

    Some(ContextSnapshot {
        files_changed: files,
        key_output: String::new(),
    })
}
