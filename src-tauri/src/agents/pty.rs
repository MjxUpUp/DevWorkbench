//! Chat wire-event schema (`ChatStreamEvent`) + ReactKernel session lifecycle
//! helpers (conversation/session row setup, finalize, compaction archive) and
//! block persistence transforms (merge / cap).
//!
//! The legacy "external CLI agent via pty" execution chain was retired — this
//! module now hosts only the wire schema and the shared session-management
//! helpers the ReactKernel driver (`react_chat_driver`) and the compaction sink
//! call. `ChatStreamEvent` is the UI-facing schema rendered into block cards.

use crate::models::{AgentType, ContextSnapshot, Session, SessionStatus};
use tauri::Emitter;

/// Wire-level structured event for the `agent:event` channel — what the chat
/// frontend renders into block cards. Decoupled from kernel-core's `AgentEvent`
/// (which has no serde derives) so this schema can evolve with the UI without
/// touching the kernel trait layer. Serialized with `kind` as the discriminator
/// tag so the TS union narrows on it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum ChatStreamEvent {
    #[serde(rename = "text")]
    Text { content: String },
    /// Reasoning/thinking trace (GLM Interleaved Thinking, claude extended
    /// thinking). Rendered as a collapsible thinking block, separate from the
    /// answer text. Streamed chunk-by-chunk by the transparent ReactAgent.
    #[serde(rename = "thinking")]
    Thinking { content: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        /// tool_call_id pairing key. Populated end-to-end: ReactKernel
        /// (`ToolCall.id` — the LLM-issued correlation id — forwarded into
        /// `ToolCallEvent.id` by react_agent, so DB replay pairs by id instead
        /// of degrading to FIFO). `Option` + `skip_serializing_if` keeps the
        /// wire clean and lets pre-id session blocks deserialize unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
        content: String,
        is_error: bool,
    },
    #[serde(rename = "result")]
    Result { is_error: bool, secs: u64 },
    /// A file was changed on disk by the agent (a write_file/patch tool landed).
    /// Surfaced as a lightweight path line so the user sees per-write mutations
    /// as they happen — distinct from the aggregated `Done.files_changed` list
    /// (a git-diff snapshot taken once at run end) and from a tool_result card
    /// (which shows tool output, not which path was touched). Maps from
    /// kernel-core `AgentEvent::FileChanged`.
    #[serde(rename = "file_changed")]
    FileChanged { path: String },
    /// Context auto-compaction meta-event (v1.3 C2). NOT produced by a model
    /// turn — emitted by the compaction sink when `maybe_compact` replaces part
    /// of the history. A meta-event: it never enters the model's history
    /// (dropped in turns_to_history / blocks_to_history), it only tells the UI
    /// to render a "context compacted" summary card. Expand the card to read the
    /// archived原文 via `read_compact_archive_cmd`. `is_error` marks a breaker
    /// trip (summarizer failed repeatedly; compaction suspended for the rest of
    /// the run — the run continues, just without further compression).
    #[serde(rename = "compact")]
    Compact {
        summary: String,
        archived_at: Option<String>,
        dropped_count: usize,
        is_error: bool,
    },
    /// Compact-boundary marker (B-plan §4.2 缺项3, CCB parity
    /// `SystemCompactBoundaryMessage`). A META event emitted by the compaction
    /// path alongside [`Compact`](Self::Compact): records WHERE a compaction
    /// happened so that on resume, `blocks_to_history` reconstructs a boundary
    /// `Message` and `maybe_compact` summarizes only what comes AFTER the last
    /// boundary — avoiding re-compaction of already-summarized history (the
    /// "summary of summary" drift). Like `Compact` it never enters the model's
    /// history (filtered out in `blocks_to_history`, like the other meta-events).
    /// `preserved_count` = how many trailing messages were kept verbatim (CCB
    /// `preservedSegment`; DW records a count, not a uuid range).
    #[serde(rename = "compact_boundary")]
    CompactBoundary {
        /// `"auto"` | `"manual"` — what triggered the compaction.
        trigger: String,
        /// Estimated tokens just before compaction ran.
        pre_tokens: usize,
        /// Trailing messages preserved verbatim across this compaction.
        preserved_count: usize,
    },
    /// Human-Gate approval request (Clutch #3). NOT a chat block — a control
    /// signal: emitted when a destructive action is about to land in
    /// `PermissionMode::HumanGate`, telling the UI to open an approval modal.
    /// The agent SUSPENDS until `resolve_human_gate_cmd` delivers a decision
    /// (or 300s auto-rejects). Never persisted into session.blocks and never
    /// enters model history (`react_chat` filters it out, like `Compact`).
    #[serde(rename = "approval_required")]
    ApprovalRequired {
        /// Tool name about to run (e.g. `bash`, `write_file`).
        tool: String,
        /// Raw JSON arguments string — the modal previews these so the user
        /// sees exactly what would execute.
        arguments: String,
        /// `approve__{session_id}__{seq}` — the UI returns this verbatim in
        /// `resolve_human_gate_cmd` to resume the right suspended call.
        resume_token: String,
        /// One-line "why this is destructive" summary (modal title).
        summary: String,
    },
}

/// Fold consecutive `Text`/`Thinking` events into one run each (same semantics
/// as the frontend's `appendBlock` merge). Persisted blocks should match what
/// the live in-memory Map held — not one entry per streaming token delta —
/// otherwise a reloaded session renders N tiny text/thinking cards instead of
/// the single merged paragraph.
///
/// Thinking must fold too: GLM Interleaved Thinking streams chunk-by-chunk, and
/// the old Text-only fold left thinking as one block per token. Session 82e56ebe
/// (4 min, glm-5.2) persisted 1681 single-token thinking blocks into a 128 KB
/// `sessions.blocks` row — the frontend merges for LIVE render
/// (`BlocksView::normalizeEvents`), but the persisted replica did not, so
/// history replay / direct DB reads saw the碎片. Folding both kinds here makes
/// the persisted copy match the live view.
pub(crate) fn merge_consecutive_runs(events: Vec<ChatStreamEvent>) -> Vec<ChatStreamEvent> {
    let mut out: Vec<ChatStreamEvent> = Vec::with_capacity(events.len());
    for ev in events {
        match (&ev, out.last_mut()) {
            (
                ChatStreamEvent::Text { content: incoming },
                Some(ChatStreamEvent::Text { content: acc }),
            ) => acc.push_str(incoming),
            (
                ChatStreamEvent::Thinking { content: incoming },
                Some(ChatStreamEvent::Thinking { content: acc }),
            ) => acc.push_str(incoming),
            _ => out.push(ev),
        }
    }
    out
}

/// Cap every string value nested inside a JSON value to `max_chars` (appending
/// "…") — recursing through objects and arrays. Used to shrink ToolUse.input
/// for the persisted copy while keeping the JSON structure intact so the
/// frontend still renders it.
fn cap_json_string_values(value: serde_json::Value, max_chars: usize) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            if s.chars().count() > max_chars {
                let capped: String = s.chars().take(max_chars).collect();
                serde_json::Value::String(format!("{}…", capped))
            } else {
                serde_json::Value::String(s)
            }
        }
        serde_json::Value::Array(arr) => serde_json::Value::Array(
            arr.into_iter()
                .map(|v| cap_json_string_values(v, max_chars))
                .collect(),
        ),
        serde_json::Value::Object(obj) => serde_json::Value::Object(
            obj.into_iter()
                .map(|(k, v)| (k, cap_json_string_values(v, max_chars)))
                .collect(),
        ),
        other => other,
    }
}

/// Apply persistence caps to a block list: only `ToolUse.input` strings are
/// capped. Live emit is NOT capped — only the DB-bound replica. Prevents a
/// giant Edit `new_string` from ballooning the row. Text is the user-facing
/// answer (left whole), Result carries no payload, and ToolResult.content was
/// already preview-capped at emit time.
pub(crate) fn cap_blocks_for_persist(
    events: Vec<ChatStreamEvent>,
    max_chars: usize,
) -> Vec<ChatStreamEvent> {
    events
        .into_iter()
        .map(|ev| match ev {
            ChatStreamEvent::ToolUse { id, name, input } => ChatStreamEvent::ToolUse {
                id,
                name,
                input: cap_json_string_values(input, max_chars),
            },
            other => other,
        })
        .collect()
}

/// Load prior turns for a conversation so `react_chat_driver` can rebuild
/// multi-turn history.
///
/// `parent_session_id` set (continuation / edit-and-regenerate fork): the
/// parent's full ancestor chain including the parent itself
/// (`load_prior_turn_chain`). `parent_session_id` None (first turn / linear
/// continue with no explicit parent): all turns of the conversation
/// (`load_turns_for_conversation_db`).
pub(crate) fn load_prior_turns(
    db_conn: &crate::db::DbState,
    conversation_id: &str,
    parent_session_id: Option<&str>,
) -> Vec<crate::models::Session> {
    let Ok(conn) = db_conn.get() else {
        return Vec::new();
    };
    match parent_session_id {
        // Parent-keyed history: the parent's full chain (its ancestors + the
        // parent itself). Two callers share this path:
        //  • continuation (ChatView sets parent = last completed turn) — the
        //    parent IS the latest round, so it must be rebuilt into history or
        //    model-switch/continue loses it (defect ⑤).
        //  • edit-and-regenerate fork (parent = edited turn's own parent = the
        //    fork point) — the fork point's content must be inherited too.
        // load_prior_turn_chain appends the parent itself to load_turn_chain_db's
        // ancestor-only result. The sibling branch being replaced is still
        // excluded: it is neither an ancestor of, nor equal to, the fork point.
        Some(pid) => crate::agents::session::load_prior_turn_chain(&conn, pid).unwrap_or_default(),
        // Flat: first turn, or a linear continue with no explicit parent (the
        // pipe path derives a parent afterwards). A linear conversation is its
        // own single branch, so conversation-wide loading == the ancestor chain.
        None => crate::agents::session::load_turns_for_conversation_db(&conn, conversation_id)
            .unwrap_or_default(),
    }
}

/// Resolve an existing conversation id, or insert a new conversation row and
/// return its id. The new conversation's title is the prompt's first 40 chars.
pub(crate) fn resolve_or_create_conversation(
    db_conn: &crate::db::DbState,
    conversation_id: Option<&str>,
    project_path: &str,
    prompt: &str,
    agent_type: &AgentType,
) -> Result<String, String> {
    let conn = db_conn.get().map_err(|e| e.to_string())?;
    let resolved_conv_id: String = match conversation_id {
        Some(id) => id.to_string(),
        None => {
            let new_id = uuid::Uuid::new_v4().to_string();
            let now = chrono::Local::now().to_rfc3339();
            let title: String = prompt.chars().take(40).collect();
            let conv = crate::models::Conversation {
                id: new_id.clone(),
                project_path: project_path.to_string(),
                title,
                last_agent: Some(agent_type.clone()),
                status: "active".to_string(),
                started_at: now.clone(),
                last_activity_at: now,
                pinned: false,
            };
            crate::agents::session::insert_conversation_db(&conn, &conv)
                .map_err(|e| e.to_string())?;
            new_id
        }
    };
    Ok(resolved_conv_id)
}

/// Build a `Running` Session row ready for `insert_session_db`. Does not write.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_running_session_row(
    session_id: &str,
    project_path: &str,
    agent_type: &AgentType,
    prompt: &str,
    model: Option<&str>,
    conversation_id: &str,
    linked_requirement_id: Option<&str>,
    parent_session_id: Option<&str>,
    task_ref: Option<&str>,
) -> Session {
    Session {
        id: session_id.to_string(),
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
        conversation_id: Some(conversation_id.to_string()),
        blocks: None,
        task_ref: task_ref.map(|s| s.to_string()),
    }
}

/// Insert the session row, touch the conversation's last_activity (when
/// attaching), record a `session_started` activity event, and emit
/// `agent:started`. This is the synchronous setup half of spawn — it must
/// complete before the caller hands the session back to the UI.
pub(crate) fn register_running_session(
    db_conn: &crate::db::DbState,
    app: &tauri::AppHandle,
    session: &Session,
    conversation_id: Option<&str>,
    resolved_conv_id: &str,
    project_path: &str,
    agent_type: &AgentType,
) -> Result<(), String> {
    let conn = db_conn.get().map_err(|e| e.to_string())?;
    crate::agents::session::insert_session_db(&conn, session).map_err(|e| e.to_string())?;

    if conversation_id.is_some() {
        let now = chrono::Local::now().to_rfc3339();
        let patch = serde_json::json!({
            "lastAgent": serde_json::to_string(agent_type).unwrap_or_default().trim_matches('"'),
            "lastActivityAt": now,
        });
        let _ = crate::agents::session::update_conversation_db(&conn, resolved_conv_id, patch);
    }

    let _ = crate::activity::record_event(
        &conn,
        &crate::activity::make_activity_event(
            &session.id,
            project_path,
            agent_type,
            "session_started",
            &format!("{} session started", agent_type.display_name()),
            None,
            None,
        ),
    );
    let _ = app.emit("agent:started", session);
    Ok(())
}

/// Write the final session state: status/finishedAt/exit/context/summary patch,
/// a `session_completed`/`session_failed` activity event (carrying the changed
/// file list), and emit `agent:completed`. Then kick off the post-session hook
/// (forge quality gate) on a background thread. The caller prepares
/// `output_summary` + `context_snapshot`; this fn only persists the terminal
/// state — so the ReactAgent driver can call it with the same shape any wait
/// thread does.
///
/// `finalize_reason` controls how `blocks_integrity::finalize_for_storage`
/// repairs the blocks before persisting. This is the SINGLE chokepoint that
/// guarantees persisted sessions always satisfy the pairing invariant — the
/// root cause of cfa53764 continuation HTTP 400 failures (orphan ToolUse on
/// disk → next turn replays malformed history → upstream API rejects).
#[allow(clippy::too_many_arguments)]
pub(crate) fn finalize_session(
    db_conn: &crate::db::DbState,
    app: &tauri::AppHandle,
    session_id: &str,
    project_path: &str,
    agent_type: &AgentType,
    session_status: SessionStatus,
    exit_code: Option<i32>,
    output_summary: Option<String>,
    context_snapshot: Option<ContextSnapshot>,
    blocks: Option<Vec<ChatStreamEvent>>,
    finalize_reason: crate::agents::blocks_integrity::FinalizeReason,
) {
    let files_for_activity = context_snapshot.as_ref().map(|s| s.files_changed.clone());

    let mut patch = serde_json::json!({
        "status": session_status.as_str(),
        "finishedAt": chrono::Local::now().to_rfc3339(),
    });
    if let Some(code) = exit_code {
        patch["exitCode"] = code.into();
    }
    if let Some(snap) = context_snapshot {
        patch["contextSnapshot"] = serde_json::to_value(snap).unwrap();
    }
    if let Some(summary) = output_summary {
        patch["outputSummary"] = serde_json::Value::String(summary);
    }
    if let Some(blocks) = blocks {
        // ── P0: pairing-invariant repair BEFORE serialization ──
        // This is the chokepoint that makes orphan ToolUse blocks impossible to
        // persist. Continuation sessions no longer replay malformed history →
        // no more HTTP 400 "tool call result does not follow tool call".
        let (repaired, stats) = crate::agents::blocks_integrity::finalize_for_storage(
            blocks,
            finalize_reason,
        );
        if !stats.was_clean {
            log::warn!(
                "[blocks_integrity] session {} repaired: reason={:?} in={} out={} stripped_use={} synth_result={} drop_dangling={}",
                session_id,
                finalize_reason,
                stats.input_blocks,
                stats.output_blocks,
                stats.stripped_orphan_use,
                stats.synthesized_result,
                stats.dropped_dangling_result,
            );
        }
        // Persist the chat blocks so a finalized session replays via BlocksView
        // instead of falling back to the raw terminal log. Merge consecutive
        // text deltas (match the live Map's shape) and cap giant ToolUse inputs
        // before serializing — live emit is untouched.
        let persisted = cap_blocks_for_persist(merge_consecutive_runs(repaired), 8000);
        if let Ok(val) = serde_json::to_value(persisted) {
            patch["blocks"] = val;
        }
        // Audit log — best-effort, never blocks the write.
        if let Ok(conn) = db_conn.get() {
            crate::agents::blocks_integrity::write_finalize_log(
                &conn,
                session_id,
                finalize_reason,
                &stats,
            );
        }
    }

    log::info!(
        "[completion] Session {} locking DB for completion update...",
        session_id
    );
    // won_race == false only when update_session_db returned Ok(0): the session
    // was already terminal (a racing stop_agent_session won). In that case skip
    // BOTH the activity record and the agent:completed emit — finalize already
    // lost, and re-emitting would double-fire / log the wrong terminal status.
    // On a DB write Err (rare) keep the prior best-effort behavior: still record
    // + emit so the UI spinner clears instead of hanging.
    let mut won_race = true;
    if let Ok(conn) = db_conn.get() {
        log::info!(
            "[completion] Session {} DB locked, writing completion...",
            session_id
        );
        match crate::agents::session::update_session_db(&conn, session_id, patch) {
            Ok(rows) => won_race = rows > 0,
            Err(e) => log::error!("[finalize] status update failed for {}: {e}", session_id),
        }
        if won_race {
            let event_type = match session_status {
                SessionStatus::Completed => "session_completed",
                _ => "session_failed",
            };
            let _ = crate::activity::record_event(
                &conn,
                &crate::activity::make_activity_event(
                    session_id,
                    project_path,
                    agent_type,
                    event_type,
                    &format!(
                        "{} session {}",
                        agent_type.display_name(),
                        session_status.as_str()
                    ),
                    None,
                    files_for_activity,
                ),
            );
        }
    } else {
        log::error!(
            "[finalize] Failed to lock DB for session {} completion update",
            session_id
        );
    }
    if won_race {
        log::info!(
            "[finalize] Emitting agent:completed for session {}",
            session_id
        );
        let _ = app.emit(
            "agent:completed",
            serde_json::json!({
                "sessionId": session_id,
                "status": session_status.as_str(),
                "exitCode": exit_code,
            }),
        );
    } else {
        log::info!(
            "[finalize] Session {} already terminal — skipping agent:completed emit",
            session_id
        );
    }

    run_post_session_hooks(
        db_conn.clone(),
        project_path.to_string(),
        session_id.to_string(),
        agent_type.clone(),
    );
}

/// Run the forge quality gate in a background thread.
fn run_post_session_hooks(
    db: crate::db::DbState,
    project_path: String,
    session_id: String,
    agent_type: AgentType,
) {
    let sid_for_log = session_id.clone();
    let result = std::thread::Builder::new()
        .name("post-session-hooks".into())
        .spawn(move || {
            // Quality gate — run subprocess
            let forge_result =
                crate::quality::forge::run_forge_gate(std::path::Path::new(&project_path));
            match forge_result {
                Ok(report) => {
                    if let Ok(conn) = db.get() {
                        let _ = crate::quality::report::save_report(&conn, &report);
                        let _ = crate::quality::feedback::create_feedback(
                            &conn,
                            &report,
                            &project_path,
                            &agent_type,
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
            sid_for_log,
            e
        );
    }
}

/// Append one compaction chunk to the session's JSONL archive (oldest first).
/// Each line records the summary + the dropped messages, so the summary card's
/// expand view can replay them. Returns the archive path on success.
pub(crate) fn append_compact_archive(
    session_id: &str,
    chunk: &crate::kernel_impl::context_compact::ArchivedChunk,
) -> Option<String> {
    let dir = crate::agents::session::agents_dir().ok()?.join("compact");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("[compact] create archive dir failed for {session_id}: {e}");
        return None;
    }
    let path = dir.join(format!("{session_id}.jsonl"));
    let line = serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "kind": chunk.kind,
        "summary": chunk.summary,
        "dropped_count": chunk.dropped_messages.len(),
        "dropped_messages": chunk.dropped_messages,
    });
    let mut serialized = match serde_json::to_string(&line) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[compact] serialize archive line failed for {session_id}: {e}");
            return None;
        }
    };
    serialized.push('\n');
    if let Err(e) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, serialized.as_bytes()))
    {
        log::warn!("[compact] write archive failed for {session_id}: {e}");
        return None;
    }
    Some(path.display().to_string())
}

/// Read all archived compaction chunks for a session (JSONL, oldest first) —
/// the expand view behind a summary card. Each line mirrors one
/// [`append_compact_archive`] write. Returns `None` when no archive exists.
pub(crate) fn read_compact_archive(session_id: &str) -> Option<Vec<serde_json::Value>> {
    let path = crate::agents::session::agents_dir()
        .ok()?
        .join("compact")
        .join(format!("{session_id}.jsonl"));
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => out.push(v),
            Err(e) => log::warn!("[compact] skip malformed archive line for {session_id}: {e}"),
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_consecutive_runs_folds_runs_and_leaves_others() {
        let evs = vec![
            ChatStreamEvent::Text {
                content: "a".into(),
            },
            ChatStreamEvent::Text {
                content: "b".into(),
            },
            ChatStreamEvent::ToolUse {
                id: None,
                name: "Read".into(),
                input: serde_json::json!({"file_path": "/x"}),
            },
            ChatStreamEvent::Text {
                content: "c".into(),
            },
        ];
        let merged = merge_consecutive_runs(evs);
        assert_eq!(merged.len(), 3);
        assert_eq!(
            merged[0],
            ChatStreamEvent::Text {
                content: "ab".into()
            }
        );
        assert_eq!(
            merged[2],
            ChatStreamEvent::Text {
                content: "c".into()
            }
        );
    }

    #[test]
    fn merge_consecutive_runs_empty_and_single() {
        assert!(merge_consecutive_runs(vec![]).is_empty());
        let one = vec![ChatStreamEvent::Text {
            content: "solo".into(),
        }];
        assert_eq!(
            merge_consecutive_runs(one),
            vec![ChatStreamEvent::Text {
                content: "solo".into()
            }]
        );
    }

    #[test]
    fn merge_consecutive_runs_all_tool_use_unchanged() {
        let evs = vec![
            ChatStreamEvent::ToolUse {
                id: None,
                name: "A".into(),
                input: serde_json::Value::Null,
            },
            ChatStreamEvent::ToolUse {
                id: None,
                name: "B".into(),
                input: serde_json::Value::Null,
            },
        ];
        let merged = merge_consecutive_runs(evs.clone());
        assert_eq!(merged, evs);
    }

    #[test]
    fn merge_consecutive_runs_folds_thinking_too() {
        // Regression (session 82e56ebe): GLM streams thinking chunk-by-chunk;
        // the persisted row held 1681 single-token thinking blocks (128 KB).
        // Folding must collapse a run of Thinking into one exactly like Text —
        // and must NOT merge across a different-kind block in between.
        let evs = vec![
            ChatStreamEvent::Thinking { content: "The".into() },
            ChatStreamEvent::Thinking { content: " user".into() },
            ChatStreamEvent::Thinking { content: " wants".into() },
            ChatStreamEvent::Text { content: "answer".into() },
            ChatStreamEvent::Thinking { content: "more".into() },
        ];
        let merged = merge_consecutive_runs(evs);
        assert_eq!(merged.len(), 3, "two thinking runs + one text in between");
        assert_eq!(
            merged[0],
            ChatStreamEvent::Thinking { content: "The user wants".into() }
        );
        assert_eq!(merged[1], ChatStreamEvent::Text { content: "answer".into() });
        assert_eq!(merged[2], ChatStreamEvent::Thinking { content: "more".into() });
    }

    #[test]
    fn cap_blocks_for_persist_truncates_long_tool_use_input_strings() {
        let big = "x".repeat(10_000);
        let evs = vec![ChatStreamEvent::ToolUse {
            id: None,
            name: "Edit".into(),
            input: serde_json::json!({ "file_path": "/p", "new_string": big }),
        }];
        let capped = cap_blocks_for_persist(evs, 8000);
        match &capped[0] {
            ChatStreamEvent::ToolUse { input, .. } => {
                let new_string = input.get("new_string").unwrap().as_str().unwrap();
                assert!(new_string.ends_with('…'));
                // 8000 kept chars + 1 ellipsis char.
                assert_eq!(new_string.chars().count(), 8001);
                // Short sibling field untouched.
                assert_eq!(input.get("file_path").unwrap().as_str(), Some("/p"));
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn cap_blocks_for_persist_leaves_text_result_and_short_input() {
        let evs = vec![
            ChatStreamEvent::Text {
                content: "answer".into(),
            },
            ChatStreamEvent::ToolUse {
                id: None,
                name: "Read".into(),
                input: serde_json::json!({"file_path": "/short"}),
            },
            ChatStreamEvent::ToolResult {
                tool_use_id: None,
                content: "ok".into(),
                is_error: false,
            },
            ChatStreamEvent::Result {
                is_error: false,
                secs: 1,
            },
        ];
        let capped = cap_blocks_for_persist(evs.clone(), 8000);
        assert_eq!(capped, evs);
    }
}
