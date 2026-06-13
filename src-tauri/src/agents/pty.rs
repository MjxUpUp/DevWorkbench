use crate::models::{AgentType, ContextSnapshot, Session, SessionStatus};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::sync::LazyLock;
use tauri::Emitter;

/// Max chars kept as output summary (truncated from tail of output)
const OUTPUT_SUMMARY_MAX_CHARS: usize = 2000;

/// Maximum time a pipe session may run before being force-killed.
const PIPE_SESSION_TIMEOUT_SECS: u64 = 600; // 10 minutes

/// Cache resolved agent exe paths to avoid repeated PATH scanning.
static EXE_CACHE: LazyLock<Mutex<HashMap<String, PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ---------------------------------------------------------------------------
// Tracked process types
// ---------------------------------------------------------------------------

/// Handles for a PTY-backed session: master (resize), writer (input), killer (stop).
struct PtyHandles {
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
}

/// A tracked process — either a real PTY or a pipe-based fallback.
enum TrackedProcess {
    Pty(PtyHandles),
    Pipe(u32),
}

/// Active agent processes tracked by session ID.
pub struct AgentProcesses {
    processes: Mutex<HashMap<String, TrackedProcess>>,
}

impl AgentProcesses {
    pub fn new() -> Self {
        Self {
            processes: Mutex::new(HashMap::new()),
        }
    }
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

/// Resolved spawn parameters for an agent.
struct SpawnConfig {
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
}

/// Cached version of `resolve_agent_exe`. Avoids PATH scanning on every spawn.
fn resolve_agent_exe_cached(agent_type: &AgentType) -> Result<PathBuf, String> {
    let key = agent_type.command_name().to_string();
    {
        let cache = EXE_CACHE.lock().map_err(|e| format!("EXE 缓存锁失败: {}", e))?;
        if let Some(path) = cache.get(&key) {
            return Ok(path.clone());
        }
    }
    let path = resolve_agent_exe(agent_type)?;
    let mut cache = EXE_CACHE.lock().map_err(|e| format!("EXE 缓存锁失败: {}", e))?;
    cache.insert(key, path.clone());
    Ok(path)
}

/// Build the program + args for an agent invocation (shared by PTY and pipe paths).
fn build_spawn_config(
    agent_type: &AgentType,
    project_path: &str,
    prompt: &str,
    model: Option<&str>,
    parent_session_id: Option<&str>,
) -> Result<SpawnConfig, String> {
    let exe = resolve_agent_exe_cached(agent_type)?;
    let is_continue = parent_session_id.is_some();
    let mut args: Vec<String> = Vec::new();

    match agent_type {
        AgentType::ClaudeCode => {
            args.push("--print".to_string());
            if is_continue {
                args.push("--continue".to_string());
            }
            args.push(prompt.to_string());
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
        }
        AgentType::Pi => {
            args.push("--prompt".to_string());
            args.push(prompt.to_string());
        }
    }

    Ok(SpawnConfig {
        program: exe,
        args,
        cwd: PathBuf::from(project_path),
    })
}

// ---------------------------------------------------------------------------
// Main spawn entry point
// ---------------------------------------------------------------------------

/// Spawn an agent process. Tries real PTY first, falls back to pipe I/O.
pub fn spawn_pty_agent(
    app: &tauri::AppHandle,
    processes: Arc<AgentProcesses>,
    db_conn: Arc<Mutex<rusqlite::Connection>>,
    project_path: &str,
    agent_type: AgentType,
    prompt: &str,
    model: Option<&str>,
    linked_requirement_id: Option<&str>,
    parent_session_id: Option<&str>,
) -> Result<Session, String> {
    // Inject project knowledge context into the prompt — run in background thread
    // with a 2-second timeout to avoid blocking the UI on slow DB queries.
    let injected_prompt = inject_knowledge_with_timeout(
        &db_conn, &agent_type, project_path, prompt,
    );
    // Inject @file references with actual file content
    let injected_prompt = inject_file_references(project_path, &injected_prompt);
    let config = build_spawn_config(&agent_type, project_path, &injected_prompt, model, parent_session_id)?;

    // On Windows, portable_pty's read() blocks until the master is closed,
    // making real-time streaming impossible. Use pipe mode as the primary path.
    // On other platforms, try PTY first for full terminal emulation.
    #[cfg(target_os = "windows")]
    {
        spawn_pipe_fallback(&app, &processes, &db_conn, &config, &agent_type, project_path, linked_requirement_id, parent_session_id, prompt, model)
    }
    #[cfg(not(target_os = "windows"))]
    {
        match try_spawn_pty(&app, &processes, &db_conn, &config, &agent_type, project_path, linked_requirement_id, parent_session_id, prompt, model) {
            Ok(session) => Ok(session),
            Err(pty_err) => {
                log::warn!("PTY creation failed ({}), falling back to pipe mode", pty_err);
                spawn_pipe_fallback(&app, &processes, &db_conn, &config, &agent_type, project_path, linked_requirement_id, parent_session_id, prompt, model)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PTY path
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn try_spawn_pty(
    app: &tauri::AppHandle,
    processes: &Arc<AgentProcesses>,
    db_conn: &Arc<Mutex<rusqlite::Connection>>,
    config: &SpawnConfig,
    agent_type: &AgentType,
    project_path: &str,
    linked_requirement_id: Option<&str>,
    parent_session_id: Option<&str>,
    prompt: &str,
    model: Option<&str>,
) -> Result<Session, String> {
    log::info!(
        "[PTY spawn] program={}, args={:?}, cwd={}",
        config.program.display(),
        config.args,
        config.cwd.display()
    );
    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("PTY openpty 失败: {}", e))?;

    let mut cmd = portable_pty::CommandBuilder::new(&config.program);
    cmd.args(config.args.iter().map(|s| s.as_str()));
    cmd.cwd(&config.cwd);

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("PTY spawn 失败: {}", e))?;

    let _pid = child.process_id().unwrap_or(0);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("PTY clone reader 失败: {}", e))?;

    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("PTY take writer 失败: {}", e))?;

    let killer = child.clone_killer();

    let session_id = uuid::Uuid::new_v4().to_string();
    processes
        .processes
        .lock()
        .unwrap()
        .insert(
            session_id.clone(),
            TrackedProcess::Pty(PtyHandles {
                master: pair.master,
                writer,
                killer,
            }),
        );

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
                    let _ = std::fs::write(dir.join(format!("{}.pre-diff", bg_sid)), pre_diff.join("\n"));
                }
            })
            .ok();
    }

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

    {
        let conn = db_conn.lock().map_err(|e| e.to_string())?;
        crate::agents::session::insert_session_db(&conn, &session)
            .map_err(|e| e.to_string())?;
        let _ = crate::activity::record_event(&conn, &crate::activity::make_activity_event(
            &session_id,
            project_path,
            agent_type,
            "session_started",
            &format!("{} session started", agent_type.display_name()),
            None,
            None,
        ));
    }
    let _ = app.emit("agent:started", &session);

    let child_exited = Arc::new(AtomicBool::new(false));

    // Reader thread
    let app_reader = app.clone();
    let sid_reader = session_id.clone();
    let processes_reader = processes.clone();
    let reader_exit_signal = child_exited.clone();
    std::thread::spawn(move || {
        let mut output_log: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        let mut reader = reader;
        let mut total_bytes: usize = 0;
        let mut emit_count: usize = 0;

        log::info!("[PTY reader] Started for session {}", sid_reader);

        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    log::info!("[PTY reader] EOF for session {}, total_bytes={}, emit_count={}", sid_reader, total_bytes, emit_count);
                    break;
                }
                Ok(n) => {
                    total_bytes += n;
                    let data = &buf[..n];
                    output_log.extend_from_slice(data);
                    let bytes_vec: Vec<u8> = data.to_vec();
                    emit_count += 1;
                    if let Err(e) = app_reader.emit(
                        "pty:output",
                        serde_json::json!({
                            "sessionId": sid_reader,
                            "data": bytes_vec,
                        }),
                    ) {
                        log::error!("[PTY reader] emit failed for session {}: {}", sid_reader, e);
                    }
                }
                Err(e) => {
                    if reader_exit_signal.load(Ordering::Acquire) {
                        log::info!("[PTY reader] Read interrupted after child exit for session {}, total_bytes={}, emit_count={}", sid_reader, total_bytes, emit_count);
                        break;
                    }
                    log::error!("[PTY reader] read error for session {}: {}", sid_reader, e);
                    break;
                }
            }
        }

        log::info!("[PTY reader] Finished for session {}, total_bytes={}, chunks={}, log_size={}", sid_reader, total_bytes, emit_count, output_log.len());

        if let Ok(agents_dir) = crate::agents::session::agents_dir() {
            let log_path = agents_dir.join("outputs");
            let _ = std::fs::create_dir_all(&log_path);
            let _ = std::fs::write(log_path.join(format!("{}.log", sid_reader)), &output_log);
        }

        if let Ok(mut map) = processes_reader.processes.lock() {
            map.remove(&sid_reader);
        }
    });

    // Wait thread
    let app_exit = app.clone();
    let sid_exit = session_id.clone();
    let project_path_exit = project_path.to_string();
    let db_conn_exit = db_conn.clone();
    let agent_type_exit = agent_type.clone();
    let processes_wait = processes.clone();
    let wait_exit_signal = child_exited.clone();
    std::thread::spawn(move || {
        log::info!("[PTY wait] Waiting for session {} to exit", sid_exit);
        let status = child.wait();
        log::info!("[PTY wait] Session {} exited: {:?}", sid_exit, status.as_ref().map(|s| s.exit_code()));

        wait_exit_signal.store(true, Ordering::Release);

        if let Ok(mut map) = processes_wait.processes.lock() {
            if map.remove(&sid_exit).is_some() {
                log::info!("[PTY wait] Dropped PTY master for session {}, reader should unblock", sid_exit);
            }
        }

        let (exit_code, session_status): (Option<i32>, SessionStatus) = match status {
            Ok(s) => (
                Some(s.exit_code() as i32),
                if s.success() { SessionStatus::Completed } else { SessionStatus::Failed },
            ),
            Err(_) => (None, SessionStatus::Failed),
        };

        std::thread::sleep(std::time::Duration::from_millis(500));

        let snapshot = extract_context_snapshot(&project_path_exit, &sid_exit);
        let output_summary = read_output_summary(&sid_exit).or_else(|| {
            if exit_code.unwrap_or(-1) == 0 {
                Some("(Agent completed with no text output)".to_string())
            } else {
                Some(format!("(Process exited with code {:?})", exit_code))
            }
        });
        let files_for_activity = snapshot.as_ref().map(|s| s.files_changed.clone());

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

        if let Ok(conn) = db_conn_exit.lock() {
            let _ = crate::agents::session::update_session_db(&conn, &sid_exit, patch);
            let event_type = match session_status {
                SessionStatus::Completed => "session_completed",
                _ => "session_failed",
            };
            let _ = crate::activity::record_event(&conn, &crate::activity::make_activity_event(
                &sid_exit,
                &project_path_exit,
                &agent_type_exit,
                event_type,
                &format!("{} session {}", agent_type_exit.display_name(), session_status.as_str()),
                None,
                files_for_activity,
            ));
        }
        let _ = app_exit.emit(
            "agent:completed",
            serde_json::json!({
                "sessionId": sid_exit,
                "status": session_status.as_str(),
                "exitCode": exit_code,
            }),
        );

        run_post_session_hooks(
            db_conn_exit.clone(),
            project_path_exit.clone(),
            sid_exit.to_string(),
            agent_type_exit.clone(),
            session_status,
        );
    });

    Ok(session)
}

// ---------------------------------------------------------------------------
// Pipe fallback path
// ---------------------------------------------------------------------------

fn spawn_pipe_fallback(
    app: &tauri::AppHandle,
    processes: &Arc<AgentProcesses>,
    db_conn: &Arc<Mutex<rusqlite::Connection>>,
    config: &SpawnConfig,
    agent_type: &AgentType,
    project_path: &str,
    linked_requirement_id: Option<&str>,
    parent_session_id: Option<&str>,
    prompt: &str,
    model: Option<&str>,
) -> Result<Session, String> {
    log::info!("[PIPE spawn] program={}, args={:?}, cwd={}", config.program.display(), config.args, config.cwd.display());
    let mut cmd = std::process::Command::new(&config.program);
    cmd.args(&config.args)
        .current_dir(&config.cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动 {} 失败: {}", agent_type.display_name(), e))?;
    let pid = child.id();

    let session_id = uuid::Uuid::new_v4().to_string();

    processes
        .processes
        .lock()
        .unwrap()
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
                    let _ = std::fs::write(dir.join(format!("{}.pre-diff", bg_sid)), pre_diff.join("\n"));
                }
            })
            .ok();
    }

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

    {
        let conn = db_conn.lock().map_err(|e| e.to_string())?;
        crate::agents::session::insert_session_db(&conn, &session)
            .map_err(|e| e.to_string())?;
        let _ = crate::activity::record_event(&conn, &crate::activity::make_activity_event(
            &session_id,
            project_path,
            agent_type,
            "session_started",
            &format!("{} session started", agent_type.display_name()),
            None,
            None,
        ));
    }
    let _ = app.emit("agent:started", &session);

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    drop(child.stdin.take());

    // Reader thread
    let app_reader = app.clone();
    let sid_reader = session_id.clone();
    let processes_reader = processes.clone();
    std::thread::spawn(move || {
        let mut output_log: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];

        if let Some(mut out) = stdout {
            loop {
                match out.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = &buf[..n];
                        output_log.extend_from_slice(data);
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
        if let Some(mut err) = stderr {
            loop {
                match err.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let data = &buf[..n];
                        output_log.extend_from_slice(data);
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

    // Wait thread — with timeout to prevent hung processes from blocking forever
    let app_exit = app.clone();
    let sid_exit = session_id.clone();
    let project_path_exit = project_path.to_string();
    let db_conn_exit = db_conn.clone();
    let agent_type_exit = agent_type.clone();
    let processes_kill = processes.clone();
    std::thread::spawn(move || {
        log::info!("[PIPE wait] Waiting for session {} to exit (timeout={}s)", sid_exit, PIPE_SESSION_TIMEOUT_SECS);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(PIPE_SESSION_TIMEOUT_SECS);
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
                    if std::time::Instant::now() > deadline {
                        log::warn!("[PIPE wait] Session {} timed out after {}s, killing", sid_exit, PIPE_SESSION_TIMEOUT_SECS);
                        timed_out = true;
                        // Force-kill the process tree
                        let _ = stop_agent(&processes_kill, &sid_exit);
                        break (None, SessionStatus::Failed);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                Err(_) => {
                    break (None, SessionStatus::Failed);
                }
            }
        };

        log::info!("[PIPE wait] Session {} exited: code={:?}, status={}, timed_out={}", sid_exit, exit_code, session_status.as_str(), timed_out);

        std::thread::sleep(std::time::Duration::from_millis(500));

        let snapshot = extract_context_snapshot(&project_path_exit, &sid_exit);
        let files_for_activity = snapshot.as_ref().map(|s| s.files_changed.clone());
        let output_summary = read_output_summary(&sid_exit).or_else(|| {
            if timed_out {
                Some(format!("(Session timed out after {}s and was killed)", PIPE_SESSION_TIMEOUT_SECS))
            } else if exit_code.unwrap_or(-1) == 0 {
                Some("(Agent completed with no text output)".to_string())
            } else {
                Some(format!("(Process exited with code {:?})", exit_code))
            }
        });

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

        if let Ok(conn) = db_conn_exit.lock() {
            let _ = crate::agents::session::update_session_db(&conn, &sid_exit, patch);
            let event_type = match session_status {
                SessionStatus::Completed => "session_completed",
                _ => "session_failed",
            };
            let _ = crate::activity::record_event(&conn, &crate::activity::make_activity_event(
                &sid_exit,
                &project_path_exit,
                &agent_type_exit,
                event_type,
                &format!("{} session {}", agent_type_exit.display_name(), session_status.as_str()),
                None,
                files_for_activity,
            ));
        } else {
            log::error!("[PIPE wait] Failed to lock DB for session {} completion update", sid_exit);
        }
        log::info!("[PIPE wait] Emitting agent:completed for session {}", sid_exit);
        let _ = app_exit.emit(
            "agent:completed",
            serde_json::json!({
                "sessionId": sid_exit,
                "status": session_status.as_str(),
                "exitCode": exit_code,
            }),
        );

        run_post_session_hooks(
            db_conn_exit.clone(),
            project_path_exit.clone(),
            sid_exit.to_string(),
            agent_type_exit.clone(),
            session_status,
        );
    });

    Ok(session)
}

// ---------------------------------------------------------------------------
// Interactive I/O (real for PTY, no-op for pipe)
// ---------------------------------------------------------------------------

/// Write data to the agent's stdin (via PTY writer). No-op for pipe sessions.
pub fn pty_write(
    processes: &Arc<AgentProcesses>,
    session_id: &str,
    data: &str,
) -> Result<(), String> {
    let mut map = processes.processes.lock().map_err(|e| format!("进程表锁失败: {}", e))?;
    if let Some(TrackedProcess::Pty(ref mut handles)) = map.get_mut(session_id) {
        handles
            .writer
            .write_all(data.as_bytes())
            .map_err(|e| format!("PTY write 失败: {}", e))?;
        handles
            .writer
            .flush()
            .map_err(|e| format!("PTY flush 失败: {}", e))?;
    }
    Ok(())
}

/// Resize the PTY terminal. No-op for pipe sessions.
pub fn pty_resize(
    processes: &Arc<AgentProcesses>,
    session_id: &str,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let map = processes.processes.lock().map_err(|e| format!("进程表锁失败: {}", e))?;
    if let Some(TrackedProcess::Pty(ref handles)) = map.get(session_id) {
        handles
            .master
            .resize(portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("PTY resize 失败: {}", e))?;
    }
    Ok(())
}

/// Stop a running agent session.
pub fn stop_agent(processes: &Arc<AgentProcesses>, session_id: &str) -> Result<(), String> {
    let tracked = {
        let mut map = processes.processes.lock().map_err(|e| format!("进程表锁失败: {}", e))?;
        map.remove(session_id)
    };

    match tracked {
        Some(TrackedProcess::Pty(mut handles)) => {
            handles
                .killer
                .kill()
                .map_err(|e| format!("PTY kill 失败: {}", e))?;
        }
        Some(TrackedProcess::Pipe(pid)) => {
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                    .output();
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = std::process::Command::new("kill")
                    .args(["-TERM", &pid.to_string()])
                    .output();
            }
        }
        None => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read the output log for a session and return a truncated summary.
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

fn extract_context_snapshot(project_path: &str, session_id: &str) -> Option<ContextSnapshot> {
    let post_diff = capture_git_diff_names(project_path);
    let pre_diff = read_pre_diff(session_id).unwrap_or_default();
    let agent_files: Vec<String> = post_diff
        .into_iter()
        .filter(|f| !pre_diff.contains(f))
        .collect();

    Some(ContextSnapshot {
        files_changed: agent_files,
        key_output: String::new(),
    })
}

fn capture_git_diff_names(project_path: &str) -> Vec<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(["diff", "--name-only"])
        .current_dir(project_path);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn read_pre_diff(session_id: &str) -> Option<Vec<String>> {
    let path = crate::agents::session::agents_dir().ok()?
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
    db_conn: &Arc<Mutex<rusqlite::Connection>>,
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
            let result = match conn.lock() {
                Ok(conn) => crate::knowledge::injector::inject_for_agent(&conn, &at, &pp, &p),
                Err(_) => p,
            };
            let _ = tx.send(result);
        })
        .ok();

    match rx.recv_timeout(std::time::Duration::from_secs(2)) {
        Ok(injected) => injected,
        Err(_) => {
            log::warn!("Knowledge injection timed out for project {}, using original prompt", project_path);
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

/// Parse `@/path/to/file` references from the prompt and replace them with
/// the actual file content wrapped in markers. Paths may be absolute or
/// relative to `project_path`.
fn inject_file_references(project_path: &str, prompt: &str) -> String {
    let mut result = prompt.to_string();
    let mut total_injected: usize = 0;
    let mut replacements: Vec<(String, String)> = Vec::new();

    // Manual scan for @/path or @X:\path patterns
    let chars: Vec<char> = prompt.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '@' {
            i += 1;
            continue;
        }

        // Look ahead: must be '/' or '\' or a Windows drive letter like 'C:\'
        let path_start = if i + 1 < chars.len() && (chars[i + 1] == '/' || chars[i + 1] == '\\') {
            i + 1
        } else if i + 3 < chars.len()
            && chars[i + 1].is_ascii_alphabetic()
            && chars[i + 2] == ':'
            && (chars[i + 3] == '\\' || chars[i + 3] == '/')
        {
            i + 1
        } else {
            i += 1;
            continue;
        };

        // Collect path chars until whitespace or end
        let mut path_end = path_start;
        while path_end < chars.len()
            && !chars[path_end].is_whitespace()
            && chars[path_end] != '@'
        {
            path_end += 1;
        }

        let file_path: String = chars[path_start..path_end].iter().collect();
        let full_match: String = chars[i..path_end].iter().collect();

        // Skip if already processed
        if replacements.iter().any(|(m, _)| m == &full_match) {
            i = path_end;
            continue;
        }

        let path = if std::path::Path::new(&file_path).is_absolute() {
            std::path::PathBuf::from(&file_path)
        } else {
            // Relative path like /src/main.rs → project_path/src/main.rs
            let relative = file_path.trim_start_matches('/').trim_start_matches('\\');
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
                format!("[File {} skipped: total injection limit reached]", path.display()),
            ));
            continue;
        }
        total_injected += injected_len;

        let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let wrapped = format!(
            "--- BEGIN FILE: {} ({}) ---\n{}\n--- END FILE: {} ---",
            file_name, path.display(), content, file_name
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
    db: Arc<Mutex<rusqlite::Connection>>,
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
                if let Ok(conn) = db.lock() {
                    let _ = crate::knowledge::collector::collect_from_session(
                        &conn, &project_path, &session_id, &agent_type,
                    );
                }
            }
            // 2. Quality gate — run subprocess
            let forge_result = crate::quality::forge::run_forge_gate(std::path::Path::new(&project_path));
            match forge_result {
                Ok(report) => {
                    if let Ok(conn) = db.lock() {
                        let _ = crate::quality::report::save_report(&conn, &report);
                        let _ = crate::quality::feedback::create_feedback(
                            &conn, &report, &project_path, &agent_type,
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
            sid_for_log, e
        );
    }
}
