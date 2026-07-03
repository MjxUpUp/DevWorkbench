use crate::agents::discovery::{discover_agents, recommend_agent, AgentInfo};
use crate::agents::kernel_tasks::KernelTasks;
use crate::agents::pty;
use crate::agents::react_chat;
use crate::agents::session;
use crate::db::DbState;
use crate::error::AppError;
use crate::kernel_impl::executor;
use crate::mcp::registry::McpRegistry;
use crate::models::SessionStatus;
use crate::models::{AgentType, Conversation, Session};
use kernel_core::{Agent, AgentEvent, AgentInput, AgentRunStatus};
use std::sync::Arc;
use tauri::{Emitter, Manager, State};

/// Tauri managed state wrapping AgentProcesses (PTY-based)
pub struct AgentState(pub Arc<pty::AgentProcesses>);

/// Managed registry of in-flight Human-Gate approvals. Thin wrapper around the
/// kernel-side [`ApprovalMap`](crate::kernel_impl::human_gate::ApprovalMap) so
/// `resolve_human_gate_cmd` and the driver (which clones an owned handle into
/// the spawned run) share one registry. Defined here (not in `human_gate`)
/// because Tauri managed state is a command-layer concern.
#[derive(Default)]
pub struct AgentApprovalState(pub crate::kernel_impl::human_gate::ApprovalMap);

impl AgentApprovalState {
    /// Drop every pending approval for a session — called from
    /// `stop_agent_session` on abort so a cancelled run doesn't leak Senders.
    pub fn clear_session(&self, session_id: &str) {
        crate::kernel_impl::human_gate::clear_session_approvals(&self.0, session_id);
    }
}

/// Resolve a Human-Gate-suspended tool call. `action` is `"approve"`,
/// `"reject"`, or `"retry"` (with `feedback`); `resume_token` must match the
/// `approval_required` event the frontend received. A token that already timed
/// out (auto-Reject after 300s) or whose session was aborted returns NotFound —
/// the UI treats that as "no longer relevant".
///
/// Each resolution is persisted to the L1 verdict ledger as a `gate =
/// "human-gate"` row (verdict APPROVE/REJECT/RETRY) tied to the session, so the
/// P6 reliability rubric's `manual_intervention` hard gate can be scored from
/// fact rather than hardcoded — a run that needed a human nudge reliably reads
/// `had_human_intervention = true` even after the in-memory approval map is
/// cleared on session end. Best-effort: a DB write failure is logged, never
/// blocks the resolution. The session id is recovered from the resume token
/// (`approve__{sid}__{seq}`); a malformed token skips the write but still
/// resolves.
#[tauri::command]
pub async fn resolve_human_gate_cmd(
    approval_state: State<'_, AgentApprovalState>,
    db: State<'_, DbState>,
    resume_token: String,
    action: String,
    feedback: Option<String>,
) -> Result<(), AppError> {
    use crate::kernel_impl::human_gate::{resolve_approval, HumanGateDecision};
    let decision = match action.as_str() {
        "approve" => HumanGateDecision::Approve,
        "reject" => HumanGateDecision::Reject,
        "retry" => HumanGateDecision::Retry {
            feedback: feedback.unwrap_or_default(),
        },
        other => {
            return Err(AppError::Internal(format!(
                "unknown approval action '{other}' (want approve|reject|retry)"
            )));
        }
    };
    match resolve_approval(&approval_state.0, &resume_token, decision) {
        Ok(()) => {
            // Persist the intervention so the rubric hard-gate is scoreable from
            // fact. `had_human_intervention` counts ANY decision (approve/reject/
            // retry) — each is a human nudge the run needed.
            if let Some(sid) = session_of_resume_token(&resume_token) {
                let row = crate::eval::verdicts::NewVerdict {
                    id: uuid::Uuid::new_v4().to_string(),
                    session_id: Some(sid.to_string()),
                    case_id: None,
                    gate: "human-gate".to_string(),
                    verdict: action.to_uppercase(),
                    // A human nudge is a recorded fact, not a gain to attribute.
                    attribution: None,
                    report: serde_json::to_string(&serde_json::json!({
                        "action": action,
                        "resume_token": resume_token,
                    }))
                    .ok(),
                    commit_sha: None,
                    created_at: chrono::Utc::now().to_rfc3339(),
                };
                if let Ok(conn) = db.get() {
                    if let Err(e) = crate::eval::verdicts::insert_verdict(&conn, &row) {
                        log::warn!("[human-gate] verdict persist failed for {sid}: {e}");
                    }
                }
            }
            Ok(())
        }
        Err(_) => Err(AppError::NotFound(format!(
            "no active approval for {resume_token} (timed out or session ended?)"
        ))),
    }
}

/// Recover the session id from a Human-Gate resume token
/// (`approve__{session_id}__{seq}`). None if the token isn't that shape — a
/// malformed token simply skips the ledger write, it doesn't fail the resolve.
fn session_of_resume_token(token: &str) -> Option<&str> {
    let rest = token.strip_prefix("approve__")?;
    let (sid, _seq) = rest.rsplit_once("__")?;
    if sid.is_empty() {
        None
    } else {
        Some(sid)
    }
}

// Agent discovery commands
#[tauri::command]
pub fn discover_agents_cmd(db: State<'_, DbState>) -> Result<Vec<AgentInfo>, AppError> {
    let conn = db.get()?;
    Ok(discover_agents(Some(&conn)))
}

#[tauri::command]
pub fn recommend_agent_for_project(tags: Vec<String>) -> Result<Option<AgentType>, AppError> {
    Ok(recommend_agent(&tags))
}

// Session commands
#[tauri::command]
pub fn load_sessions(db: State<'_, DbState>) -> Result<Vec<Session>, AppError> {
    let conn = db.get()?;
    crate::agents::session::load_sessions_from_db(&conn)
}

/// Read the FULL (ANSI-stripped) output for a session, for the completed-session terminal view.
/// Unlike the stored `outputSummary` (tail-truncated to OUTPUT_SUMMARY_MAX_CHARS), this returns
/// the complete text so the completed session isn't cut off mid-reply.
#[tauri::command]
pub fn read_session_output_cmd(session_id: String) -> Result<Option<String>, AppError> {
    Ok(pty::read_full_session_output(&session_id))
}

/// Read all archived compaction chunks for a session (v1.3 C2). Each chunk is
/// one compaction pass (micro-clear or summarize) holding the dropped messages
/// verbatim — the expand view behind a "context compacted" summary card.
/// Returns `null` when no archive exists (session never compacted). Mirrors
/// [`read_session_output_cmd`] but for the compact archive.
#[tauri::command]
pub fn read_compact_archive_cmd(
    session_id: String,
) -> Result<Option<Vec<serde_json::Value>>, AppError> {
    Ok(pty::read_compact_archive(&session_id))
}

// Agent process lifecycle commands (PTY-based for CLI agents + kernel for the
// self-hosted ReactAgent).

/// Expand a leading `/name args` into the matched slash command's template
/// (claude-code argumentSubstitution). Unknown commands pass through. This is
/// the "submit-time render" seam: the frontend inserts `/plan fix X`, and
/// spawn renders it to the template with `fix X` in `$ARGUMENTS` BEFORE the
/// kernel sees it — slash commands are a prompt-template layer over the
/// kernel, not a kernel feature.
fn expand_slash_command(db: &crate::db::DbState, prompt: String) -> Result<String, AppError> {
    let (name, args) = match crate::slash_commands::registry::parse_command(&prompt) {
        Some(x) => x,
        None => return Ok(prompt),
    };
    let conn = db
        .get()
        .map_err(|e| AppError::Config(format!("Lock error: {e}")))?;
    match crate::slash_commands::registry::find_by_name(&conn, &name)? {
        Some(cmd) => Ok(crate::slash_commands::registry::render_template(
            &cmd.template,
            &args,
        )),
        None => Ok(prompt),
    }
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn spawn_agent_session(
    app: tauri::AppHandle,
    db: State<'_, DbState>,
    kernel_tasks: State<'_, KernelTasks>,
    project_path: String,
    agent_type: AgentType,
    prompt: String,
    model: Option<String>,
    linked_requirement_id: Option<String>,
    parent_session_id: Option<String>,
    conversation_id: Option<String>,
    mode: Option<crate::kernel_impl::hooks::PermissionMode>,
    task_ref: Option<String>,
) -> Result<Session, AppError> {
    // MUST be `async`: react_chat_driver calls `tokio::spawn`, which requires a
    // current Tokio runtime context. A sync Tauri command runs on the main
    // thread (NO runtime context) → `tokio::spawn` panics with "there is no
    // reactor running", and because that panic is on the main thread it can't
    // unwind across the Tauri/webview FFI boundary → process aborts (闪退). The
    // async command body is driven on Tauri's tokio runtime, so the runtime
    // context is present and `tokio::spawn` succeeds. The CLI path
    // (spawn_pty_agent) uses `std::thread::spawn` and never had this issue.
    let prompt = expand_slash_command(db.inner(), prompt)?;
    // 砍 CLI（用户决定 1）：chat 唯一执行路径 = 自研 ReactKernel。原 kernel=false
    // 的 pty::spawn_pty_agent 分支退役——CLI agent 选项已从 ChatHeader 移除，
    // pty::spawn_pty_agent 仅由 OpaqueAgent（工作流节点桥接外部 CLI）调用。
    // 多模型通过协议层（Anthropic/OpenAI，de-glm 已落地）支撑，不靠 CLI 壳。
    Ok(react_chat_driver(
        &app,
        db.inner().clone(),
        kernel_tasks.inner(),
        &project_path,
        &agent_type,
        &prompt,
        model.as_deref(),
        linked_requirement_id.as_deref(),
        parent_session_id.as_deref(),
        conversation_id.as_deref(),
        mode.unwrap_or_default(),
        task_ref.as_deref(),
    )?)
}

/// Drive a self-hosted ReactAgent on a background tokio task, mirroring the pty
/// spawn shape: build the Running session row synchronously (so the UI gets a
/// session id + `agent:started` immediately), then stream AgentEvents →
/// ChatStreamEvents → `agent:event`, and finalize on completion/error.
///
/// The driver task is registered in `KernelTasks` so `stop_agent_session` can
/// abort it. Aborting drops the future — the driver's own finalize does NOT run
/// — so stop must write the failed status itself, which it does.
///
/// MVP constraints (explicitly deferred, see plan §"阶段 E 后续"):
/// empty ToolRegistry (no real tools yet), non-streaming `generate()` (Token is
/// the whole message, not chunked), single-turn (resume_from ignored).
#[allow(clippy::too_many_arguments)]
fn react_chat_driver(
    app: &tauri::AppHandle,
    db_conn: crate::db::DbState,
    kernel_tasks: &KernelTasks,
    project_path: &str,
    agent_type: &AgentType,
    prompt: &str,
    model: Option<&str>,
    linked_requirement_id: Option<&str>,
    parent_session_id: Option<&str>,
    conversation_id: Option<&str>,
    mode: crate::kernel_impl::hooks::PermissionMode,
    task_ref: Option<&str>,
) -> Result<Session, String> {
    let session_id = uuid::Uuid::new_v4().to_string();
    log::info!(
        "[react_chat] driver START sid={session_id} agent={agent_type:?} model={model:?} conv={conversation_id:?}"
    );
    let resolved_conv_id = pty::resolve_or_create_conversation(
        &db_conn,
        conversation_id,
        project_path,
        prompt,
        agent_type,
    )?;
    let session = pty::build_running_session_row(
        &session_id,
        project_path,
        agent_type,
        prompt,
        model,
        &resolved_conv_id,
        linked_requirement_id,
        parent_session_id,
        task_ref,
    );
    pty::register_running_session(
        &db_conn,
        app,
        &session,
        conversation_id,
        &resolved_conv_id,
        project_path,
        agent_type,
    )?;

    // Shadow-git checkpoint at session start: snapshot the working tree so the
    // user can roll this agent's changes back later. Best-effort — a non-repo
    // path or missing git only disables rollback for this session, it never
    // blocks the run (checkpoint is an enhancement, not a gate).
    if let Err(e) = crate::kernel_impl::checkpoint::create_at_session_start(
        project_path,
        &session_id,
        "session_start",
    ) {
        log::warn!("[checkpoint] create failed for {session_id}: {e} (rollback disabled)");
    }

    // T8 experience flywheel replay: pull Forge's pending *mandatory* reviews
    // (low-score, unresolved) into the knowledge base as quality_failure lessons,
    // which build_react_agent → experience_prompt_suffix (T7) surfaces in THIS
    // session's system prompt. Best-effort + dedup → idempotent and non-blocking
    // (forge missing / no pending reviews = no-op). Same blocking-subprocess-at-
    // session-start pattern as the checkpoint above.
    match crate::quality::experience::list_forge_reviews(std::path::Path::new(project_path)) {
        Ok(reviews) => {
            let pending = crate::quality::experience::pending_mandatory(&reviews);
            let accepted = crate::quality::experience::accepted_only(&reviews);
            let resolved = crate::quality::experience::resolved_not_accepted(&reviews);
            if !pending.is_empty() || !accepted.is_empty() || !resolved.is_empty() {
                if let Ok(conn) = db_conn.get() {
                    let hash = crate::activity::hash_project_path(project_path);
                    // Flywheel BOTH ways, with a split exit: replay pending
                    // lessons INTO the store (project-local + one global per
                    // dimension); ACCEPTED reviews purge their lessons OUT (full
                    // exit — the user signed off); RESOLVED-only reviews DECAY
                    // confidence (soft exit — improvement stays on record for
                    // tracking). Global lessons are cross-project aggregates and
                    // are never purged/decayed by a single project's resolve.
                    let res = crate::quality::experience::replay_to_knowledge(
                        &conn,
                        project_path,
                        &pending,
                        agent_type,
                    );
                    let purged = crate::quality::experience::purge_lessons_for_resolved_reviews(
                        &conn, &hash, &accepted,
                    );
                    let decayed = crate::quality::experience::decay_confidence_for_resolved_reviews(
                        &conn, &hash, &resolved,
                    );
                    log::info!(
                        "[experience] replayed {} lessons ({} skipped, {} global), purged {} accepted, decayed {} resolved for {session_id}",
                        res.replayed, res.skipped, res.promoted_global, purged, decayed
                    );
                }
            }
        }
        Err(crate::error::AppError::ForgeNotInstalled) => {} // forge absent → nothing new
        Err(e) => log::warn!("[experience] replay failed for {session_id}: {e}"),
    }

    // Prior conversation turns → structured Message history. This is the
    // ReactAgent analog of the CLI path's inject_conversation_context, but built
    // from persisted blocks (real user/assistant/tool turns) instead of a flat
    // output_summary string. Computed before spawn so it can be moved into the
    // task. load_prior_turns returns ASC; the current turn (just registered as
    // Running) is excluded by turns_to_history's Running filter.
    let prior_turns = pty::load_prior_turns(&db_conn, &resolved_conv_id, parent_session_id);
    let history_drv = react_chat::turns_to_history(
        &prior_turns,
        react_chat::REACT_HISTORY_TURN_TEXT_CAP,
        react_chat::REACT_HISTORY_TOTAL_TEXT_CAP,
    );

    let started = std::time::Instant::now();
    let app_drv = app.clone();
    let sid_drv = session_id.clone();
    let db_drv = db_conn.clone();
    let pp_drv = project_path.to_string();
    let at_drv = agent_type.clone();
    let model_drv = model.map(|m| m.to_string());
    let prompt_drv = prompt.to_string();
    let conv_drv = resolved_conv_id.clone();
    let task_ref_drv = task_ref.map(|s| s.to_string());
    // v1.3 C2: shared buffer between the ReactAgent's compaction path (which
    // pushes Compact meta-events here — they bypass the AgentEvent stream, so
    // the loop below can't collect them) and this driver (which splices them
    // into final_blocks at stream end so the summary card persists + replays).
    // Cloned into build_react_agent below.
    let compaction_blocks: std::sync::Arc<std::sync::Mutex<Vec<pty::ChatStreamEvent>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    // v2 Human Gate: clone the shared approval map so the ReactAgent can
    // suspend destructive calls and resolve_human_gate_cmd (managed state) can
    // deliver the decision. Only actually consulted when mode == HumanGate
    // (build_react_agent gates the wiring); cloning unconditionally is cheap
    // (Arc<Mutex>) and keeps the driver signature uniform across modes.
    let approval_map = app
        .try_state::<AgentApprovalState>()
        .map(|s| std::sync::Arc::clone(&s.0));

    let handle = tokio::spawn(async move {
        log::info!("[react_chat] task ENTERED sid={sid_drv}");
        let mcp = app_drv.try_state::<McpRegistry>();
        let agent = match executor::build_react_agent(
            model_drv.as_deref(),
            mcp.as_deref(),
            &pp_drv,
            Some(conv_drv.as_str()),
            history_drv,
            Some(db_drv.clone()),
            mode,
            task_ref_drv.as_deref(),
            // Trace attribution: this turn's LLM calls land under sid_drv so a
            // failed session's real req/resp body is one query away.
            Some(sid_drv.as_str()),
            None, // skill_filter
            None, // mcp_filter
            None, // knowledge_ids
            // Main orchestrator agent — give it WorkflowTool so it can
            // self-plan a DAG for complex multi-step tasks.
            Some(app_drv.clone()),
            Some(std::sync::Arc::clone(&compaction_blocks)),
            approval_map,
        ) {
            Ok(a) => a,
            Err(e) => {
                log::error!("[react_chat] build_react_agent failed for {}: {e}", sid_drv);
                pty::finalize_session(
                    &db_drv,
                    &app_drv,
                    &sid_drv,
                    &pp_drv,
                    &at_drv,
                    SessionStatus::Failed,
                    None,
                    Some(format!("Agent init failed: {e}")),
                    None,
                    None,
                );
                return;
            }
        };
        log::info!("[react_chat] build_react_agent OK sid={sid_drv}");
        let input = AgentInput {
            prompt: prompt_drv.clone(),
            working_dir: Some(pp_drv.clone()),
            model: None,
            resume_from: None,
        };
        let mut stream = match agent.run(input) {
            Ok(s) => s,
            Err(e) => {
                log::error!("[react_chat] agent.run failed for {}: {e}", sid_drv);
                pty::finalize_session(
                    &db_drv,
                    &app_drv,
                    &sid_drv,
                    &pp_drv,
                    &at_drv,
                    SessionStatus::Failed,
                    None,
                    Some(format!("Agent run failed: {e}")),
                    None,
                    None,
                );
                return;
            }
        };
        log::info!("[react_chat] agent.run OK, streaming sid={sid_drv}");

        use futures::StreamExt;
        let mut final_status = SessionStatus::Completed;
        let mut final_exit: Option<i32> = Some(0);
        let mut final_output = String::new();
        // Accumulate the wire events emitted to agent:event so the completed
        // session can be persisted and replayed via BlocksView. Mirrors the
        // pipe path's Arc<Mutex<Vec>> accumulation.
        let mut final_blocks: Vec<pty::ChatStreamEvent> = Vec::new();
        while let Some(ev_res) = stream.next().await {
            let secs = started.elapsed().as_secs();
            let ev = match ev_res {
                Ok(e) => e,
                Err(e) => {
                    log::error!("[react_chat] stream error for {}: {e}", sid_drv);
                    let summary = if final_output.is_empty() {
                        Some(format!("Agent error: {e}"))
                    } else {
                        Some(final_output.clone())
                    };
                    pty::finalize_session(
                        &db_drv,
                        &app_drv,
                        &sid_drv,
                        &pp_drv,
                        &at_drv,
                        SessionStatus::Failed,
                        None,
                        summary,
                        None,
                        None,
                    );
                    return;
                }
            };
            // Track terminal status/exit/output from Done; accumulate Token text
            // as a fallback summary (Done.output_summary is authoritative when set).
            match &ev {
                AgentEvent::Token(t) => final_output.push_str(t),
                AgentEvent::Done(outcome) => {
                    final_status = match outcome.status {
                        AgentRunStatus::Completed => SessionStatus::Completed,
                        _ => SessionStatus::Failed,
                    };
                    final_exit = outcome.exit_code;
                    if let Some(s) = &outcome.output_summary {
                        final_output = s.clone();
                    }
                }
                _ => {}
            }
            let wires = react_chat::map_agent_event(ev, secs);
            for wire in &wires {
                let _ = app_drv.emit(
                    "agent:event",
                    serde_json::json!({ "sessionId": &sid_drv, "event": wire }),
                );
            }
            final_blocks.extend(wires);
        }

        // Stream ended (ReactAgent always yields Done before ending — this is the
        // normal completion path). Remove from the stop table so a later stop on
        // this completed session doesn't touch a dead handle, then persist state.
        // v1.3 C2: splice the compaction sink's Compact events into final_blocks
        // so the summary card survives the live→persisted handoff + replays on
        // reload. The LIVE path (agentStore.appendBlock via the emitted
        // agent:event) already placed them in real-time order during the run;
        // here we append them at the tail of the persisted transcript (perfect
        // time-interleaving isn't recoverable without per-event timestamps, and
        // a tail marker "compacted N× this session" is honest for the replay).
        let compact_count = compaction_blocks.lock().map(|mut b| {
            let n = b.len();
            final_blocks.append(&mut b);
            n
        }).unwrap_or(0);
        log::info!(
            "[react_chat] stream ENDED sid={sid_drv} status={final_status:?} blocks={} ({} compact)",
            final_blocks.len(),
            compact_count
        );
        if let Some(kt) = app_drv.try_state::<KernelTasks>() {
            kt.remove(&sid_drv);
        }
        let summary = if final_output.is_empty() {
            None
        } else {
            Some(final_output)
        };

        // v1.3 T2 + D6: close the long-term-memory loop. A Completed kernel-
        // agent session has no CLI log, so write its knowledge contributions
        // directly — the natural-language `react_session` memory (what it SAID)
        // AND the structured `react_reflection` companion (what it DID). The
        // write core is factored into session_reflection::persist_completion_
        // memory so it's unit-testable over a plain Connection; this surrounding
        // closure holds a Tauri AppHandle + live ReactAgent stream and can't be
        // driven from `cargo test`. Best-effort (a DB failure just logs). Only
        // on Completed so a Failed/degraded run doesn't pollute memory.
        if final_status == SessionStatus::Completed {
            if let Ok(conn) = db_drv.get() {
                let hash = crate::activity::hash_project_path(&pp_drv);
                let written = crate::kernel_impl::session_reflection::persist_completion_memory(
                    &conn,
                    &hash,
                    &sid_drv,
                    &prompt_drv,
                    summary.as_deref(),
                    &final_blocks,
                    &at_drv,
                );
                if written > 0 {
                    log::info!("[react_chat] {written} knowledge entries recorded for {sid_drv}");
                }
            }
        }

        pty::finalize_session(
            &db_drv,
            &app_drv,
            &sid_drv,
            &pp_drv,
            &at_drv,
            final_status,
            final_exit,
            summary,
            None,
            Some(final_blocks),
        );
    });

    kernel_tasks.insert(&session_id, handle);
    Ok(session)
}

// ---- Conversation commands ----
//
// A conversation is the multi-turn container. spawn_agent_session attaches a
// turn (creating the conversation when conversation_id is None); these commands
// cover listing / renaming / archiving for the sidebar.

#[tauri::command]
pub fn list_conversations(
    db: State<'_, DbState>,
    project_path: String,
    include_archived: Option<bool>,
) -> Result<Vec<Conversation>, AppError> {
    let conn = db.get()?;
    // include_archived defaults to false: the sidebar shows only active
    // conversations; archived/deleted are soft-hidden until explicitly requested.
    session::load_conversations_for_project_db(
        &conn,
        &project_path,
        include_archived.unwrap_or(false),
    )
}

#[tauri::command]
pub fn update_conversation(
    db: State<'_, DbState>,
    id: String,
    patch: serde_json::Value,
) -> Result<(), AppError> {
    let conn = db.get()?;
    session::update_conversation_db(&conn, &id, patch)
}

/// Archive a conversation (soft-hide from the sidebar, undoable). Sets
/// status='archived'; load_conversations filters it out unless include_archived.
#[tauri::command]
pub fn archive_conversation(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db.get()?;
    session::set_conversation_status_db(&conn, &id, "archived")
}

/// Delete a conversation (soft-delete, undoable within the frontend undo
/// window). Sets status='deleted'; the row stays so the undo can restore it to
/// 'active' via set_conversation_status_db.
#[tauri::command]
pub fn delete_conversation(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db.get()?;
    session::set_conversation_status_db(&conn, &id, "deleted")
}

/// Restore an archived/deleted conversation back to the sidebar (undo path for
/// both archive and delete). Sets status='active'.
#[tauri::command]
pub fn restore_conversation(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db.get()?;
    session::set_conversation_status_db(&conn, &id, "active")
}

/// 编辑某条 turn 的 prompt 并重新生成。语义 = 从该 turn 的**父节点** fork
/// 一个新兄弟 turn(parent = 被编辑 turn 的 parent_session_id,prompt = 新内容,
/// 同 conversation),然后重跑 agent。新 turn 与被编辑 turn 成兄弟(共享
/// parent),构成可切换分支;旧 turn 保留,前端分支切换器在兄弟间切换。
///
/// Claude Code 的 "edit a message and regenerate" 等价物——但模型化在 turn 级别
/// (一次 agent 运行)而非单条 chat message,贴合本项目 conversation→turn
/// (parent_session_id 链)的持久化结构,无需引入独立 messages 表。
#[tauri::command]
pub async fn edit_and_regenerate(
    app: tauri::AppHandle,
    db: State<'_, DbState>,
    kernel_tasks: State<'_, KernelTasks>,
    session_id: String,
    new_prompt: String,
) -> Result<Session, AppError> {
    let edited = {
        let conn = db.get()?;
        session::get_session_by_id_db(&conn, &session_id)?
            .ok_or_else(|| AppError::NotFound(format!("Session {session_id} 不存在")))?
    };
    let conversation_id = edited.conversation_id.clone().ok_or_else(|| {
        AppError::Internal(format!("Session {session_id} 无 conversation_id,无法 fork"))
    })?;
    // Fork 点 = 被编辑 turn 的 parent(不是被编辑 turn 本身),所以重生成的
    // turn 是从同一分叉点长出的兄弟。新 turn 的 prior-turn history 随之只走该
    // parent 的祖先链(load_turn_chain_db),绝不混入被编辑 turn 那条已被替换
    // 的分支——这是避免分支污染的关键。
    spawn_agent_session(
        app,
        db,
        kernel_tasks,
        edited.project_path,
        edited.agent_type,
        new_prompt,
        edited.model,
        None, // linked_requirement_id 不继承
        edited.parent_session_id, // ← fork 点
        Some(conversation_id),    // ← 同 conversation
        None,              // mode: 默认权限态
        edited.task_ref,   // 复用原任务上下文
    )
    .await
}

/// 返回一个 conversation 的扁平分支节点(turn + parent 指针,oldest-first)。
/// 前端按 parent_session_id 分组渲染分支切换器。
#[tauri::command]
pub fn get_conversation_branches(
    db: State<'_, DbState>,
    conversation_id: String,
) -> Result<Vec<session::BranchNode>, AppError> {
    let conn = db.get()?;
    session::load_conversation_branches_db(&conn, &conversation_id)
}

#[tauri::command]
pub fn stop_agent_session(
    app: tauri::AppHandle,
    state: State<'_, AgentState>,
    db: State<'_, DbState>,
    kernel_tasks: State<'_, KernelTasks>,
    approval_state: State<'_, AgentApprovalState>,
    session_id: String,
) -> Result<(), AppError> {
    // Kernel agents have no PID — abort their driver task. Returns true iff this
    // session was a kernel task, in which case we skip the pty/PID kill below.
    let was_kernel = kernel_tasks.abort(&session_id);
    // v2 Human Gate: reclaim any pending approval for this session. Aborting the
    // driver task drops its future, which drops the Receiver of a suspended
    // check() — but the Sender stays in the map until cleared, leaking entries.
    // Clearing here also lets a still-live check() auto-reject promptly (its
    // Receiver gets None → Reject) instead of waiting the full 300s timeout.
    approval_state.clear_session(&session_id);
    if !was_kernel {
        // Best-effort PID kill; process may already be dead (stale session)
        let _ = pty::stop_agent(&state.0, &session_id);
    }
    // Aborting a kernel task drops its future — the driver's own finalize does
    // NOT run. So we always write the failed status + emit agent:completed
    // here (same as the pty path), regardless of agent kind.

    // Always update session status so UI reflects the stop immediately.
    // "cancelled" (not "failed") — the user deliberately stopped it, so the UI
    // renders "已取消" rather than "失败". AgentRunStatus::Cancelled exists for
    // this distinction but was never wired until now.
    let patch = serde_json::json!({
        "status": "cancelled",
        "finishedAt": chrono::Utc::now().to_rfc3339(),
        "exitCode": 0,
        "outputSummary": "Session cancelled by user"
    });
    let won_race = {
        let conn = db.get()?;
        crate::agents::session::update_session_db(&conn, &session_id, patch)? > 0
    };

    // Only emit if this stop won the running→terminal race. If the agent had
    // already finalized naturally (status no longer 'running'), update_session_db
    // flipped 0 rows (CAS) and finalize_session already emitted agent:completed —
    // emitting again would double-fire the event (duplicate notification + a
    // non-deterministic cancelled↔completed flip).
    if won_race {
        let _ = app.emit(
            "agent:completed",
            serde_json::json!({
                "sessionId": session_id,
                "status": "cancelled",
                "exitCode": 0
            }),
        );
    }

    Ok(())
}

#[tauri::command]
pub fn pty_write_cmd(
    state: State<'_, AgentState>,
    session_id: String,
    data: String,
) -> Result<(), AppError> {
    pty::pty_write(&state.0, &session_id, &data).map_err(AppError::from)
}

#[tauri::command]
pub fn pty_resize_cmd(
    state: State<'_, AgentState>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), AppError> {
    pty::pty_resize(&state.0, &session_id, cols, rows).map_err(AppError::from)
}

// Activity commands
#[tauri::command]
pub fn get_project_activity(
    db: State<'_, DbState>,
    project_path: String,
) -> Result<Vec<crate::models::ActivityEvent>, AppError> {
    let conn = db.get()?;
    crate::activity::get_events_for_project(&conn, &project_path)
}

#[tauri::command]
pub fn get_recent_activity(
    db: State<'_, DbState>,
    limit: Option<usize>,
) -> Result<Vec<crate::models::ActivityEvent>, AppError> {
    let conn = db.get()?;
    crate::activity::get_recent_events(&conn, limit.unwrap_or(50))
}

// Knowledge commands
#[tauri::command]
pub fn search_knowledge(
    db: State<'_, DbState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<crate::models::KnowledgeEntry>, AppError> {
    let conn = db.get()?;
    crate::knowledge::store::search_entries(&conn, &query, limit.unwrap_or(20))
}

#[tauri::command]
pub fn get_knowledge_for_project(
    db: State<'_, DbState>,
    project_path: String,
) -> Result<Vec<crate::models::KnowledgeEntry>, AppError> {
    let conn = db.get()?;
    let hash = crate::activity::hash_project_path(&project_path);
    crate::knowledge::store::get_entries_for_project(&conn, &hash)
}

#[tauri::command]
pub fn delete_knowledge_entry(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db.get()?;
    crate::knowledge::store::delete_entry(&conn, &id)
}

#[tauri::command]
pub fn update_knowledge_entry(
    db: State<'_, DbState>,
    id: String,
    title: String,
    content: String,
) -> Result<(), AppError> {
    let conn = db.get()?;
    crate::knowledge::store::update_entry(&conn, &id, &title, &content)
}

// Config commands
#[tauri::command]
pub fn load_mcp_config(project_path: String) -> Result<crate::models::McpConfigFile, AppError> {
    let path = std::path::Path::new(&project_path).join("mcp-servers.toml");
    if !path.exists() {
        return Ok(crate::models::McpConfigFile { servers: vec![] });
    }
    crate::config::mcp::load_mcp_config(&path)
}

#[tauri::command]
pub fn save_mcp_config(
    project_path: String,
    config: crate::models::McpConfigFile,
) -> Result<(), AppError> {
    let path = std::path::Path::new(&project_path).join("mcp-servers.toml");
    crate::config::mcp::save_mcp_config(&config, &path)
}

#[tauri::command]
pub fn apply_mcp_config(
    project_path: String,
    config: crate::models::McpConfigFile,
) -> Result<Vec<String>, AppError> {
    let path = std::path::Path::new(&project_path);
    crate::config::adapters::apply_translations(&config, path)
}

// Quality commands
#[tauri::command]
pub fn get_quality_reports(
    db: State<'_, DbState>,
) -> Result<Vec<crate::models::QualityReport>, AppError> {
    let conn = db.get()?;
    crate::quality::report::get_all_reports(&conn)
}

#[tauri::command]
pub fn get_quality_report_for_session(
    db: State<'_, DbState>,
    session_id: String,
) -> Result<Option<crate::models::QualityReport>, AppError> {
    let conn = db.get()?;
    crate::quality::report::get_report_for_session(&conn, &session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resume token embeds the session id as `approve__{sid}__{seq}` — the
    /// human-gate ledger write recovers the session from it. Pin the shape so
    /// the rubric's `had_human_intervention` query keys on the right id.
    #[test]
    fn session_of_resume_token_parses_sid() {
        assert_eq!(
            session_of_resume_token("approve__sess-abc__0"),
            Some("sess-abc")
        );
        assert_eq!(
            session_of_resume_token("approve__550e8400-e29b__12"),
            Some("550e8400-e29b")
        );
    }

    #[test]
    fn session_of_resume_token_rejects_malformed() {
        // Wrong prefix / missing seq / empty sid → None (write skipped, resolve
        // still succeeds — best-effort persistence).
        assert_eq!(session_of_resume_token("approve__sess-abc"), None);
        assert_eq!(session_of_resume_token("other__sess__0"), None);
        assert_eq!(session_of_resume_token("approve____0"), None);
    }
}
