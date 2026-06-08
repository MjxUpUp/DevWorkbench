use crate::models::{AgentType, ContextSnapshot, Session, SessionStatus};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tauri::Emitter;

/// Active agent processes tracked by session ID.
/// Wrapped in Arc for sharing across threads.
pub struct AgentProcesses {
    /// session_id -> child PID
    processes: Mutex<HashMap<String, u32>>,
}

impl AgentProcesses {
    pub fn new() -> Self {
        Self {
            processes: Mutex::new(HashMap::new()),
        }
    }
}

/// Spawn an agent process and stream output via Tauri events
pub fn spawn_agent(
    app: &tauri::AppHandle,
    processes: Arc<AgentProcesses>,
    project_path: &str,
    agent_type: AgentType,
    prompt: &str,
    model: Option<&str>,
    linked_requirement_id: Option<&str>,
) -> Result<Session, String> {
    let session_id = uuid::Uuid::new_v4().to_string();

    // Build command based on agent type
    let mut cmd = build_spawn_command(&agent_type, project_path, prompt, model)?;

    // Configure stdio for output capture
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    let working_dir = Path::new(project_path);
    if working_dir.exists() {
        cmd.current_dir(working_dir);
    }

    // Spawn the process
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
        parent_session_id: None,
    };

    // Save session
    crate::agents::session::add_session(session.clone())?;

    // Emit started event
    let _ = app.emit("agent:started", &session);

    // Spawn background thread to read output and emit events
    let app_clone = app.clone();
    let session_id_clone = session_id.clone();
    let processes_clone = processes.clone();

    // Read stdout in background
    if let Some(stdout) = child.stdout.take() {
        let sid_out = session_id_clone.clone();
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            let reader = BufReader::new(stdout);
            let mut output_log = Vec::new();
            for line in reader.lines() {
                match line {
                    Ok(text) => {
                        output_log.push(text.clone());
                        let _ = app_clone.emit(
                            "agent:output",
                            serde_json::json!({
                                "sessionId": sid_out,
                                "line": text,
                            }),
                        );
                    }
                    Err(_) => break,
                }
            }
            // Save output to log file
            if let Ok(agents_dir) = crate::agents::session::agents_dir() {
                let log_path = agents_dir.join("outputs");
                let _ = std::fs::create_dir_all(&log_path);
                let _ = std::fs::write(
                    log_path.join(format!("{}.log", sid_out)),
                    output_log.join("\n"),
                );
            }
        });
    }

    // Spawn a thread to wait for exit
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

        // Extract context snapshot from git diff
        let snapshot = extract_context_snapshot(&project_path_exit);

        // Update session
        let mut patch = serde_json::json!({
            "status": match session_status {
                SessionStatus::Completed => "completed",
                SessionStatus::Failed => "failed",
                SessionStatus::Running => "running",
            },
            "finishedAt": chrono::Local::now().to_rfc3339(),
        });
        if let Some(code) = exit_code {
            patch["exitCode"] = code.into();
        }
        if let Some(snap) = snapshot {
            patch["contextSnapshot"] = serde_json::to_value(snap).unwrap();
        }

        let _ = crate::agents::session::update_session(&sid_exit, patch.clone());
        let _ = app_exit.emit(
            "agent:completed",
            serde_json::json!({
                "sessionId": sid_exit,
                "status": match session_status {
                    SessionStatus::Completed => "completed",
                    SessionStatus::Failed => "failed",
                    SessionStatus::Running => "running",
                },
                "exitCode": exit_code,
            }),
        );

        // Remove from active processes
        if let Ok(mut proc_map) = processes_clone.processes.lock() {
            proc_map.remove(&sid_exit);
        }
    });

    Ok(session)
}

fn build_spawn_command(
    agent_type: &AgentType,
    _project_path: &str,
    prompt: &str,
    model: Option<&str>,
) -> Result<std::process::Command, String> {
    let exe = crate::commands::tools::which_expanded(agent_type.command_name())
        .ok_or_else(|| format!("{} 未安装", agent_type.display_name()))?
        .to_string_lossy()
        .to_string();

    let mut cmd = std::process::Command::new(&exe);

    match agent_type {
        AgentType::ClaudeCode => {
            cmd.arg("--print").arg(prompt);
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
            cmd.arg("copilot")
                .arg("suggest")
                .arg("--prompt")
                .arg(prompt);
        }
        AgentType::QwenCode => {
            cmd.arg("--prompt").arg(prompt);
        }
    }

    Ok(cmd)
}

/// Stop a running agent session
pub fn stop_agent(processes: &Arc<AgentProcesses>, session_id: &str) -> Result<(), String> {
    let map = processes.processes.lock().unwrap();
    if let Some(&pid) = map.get(session_id) {
        // Kill the process — cross-platform via subprocess command
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
