//! Human Gate (Clutch #3) — synchronous human approval for destructive agent
//! actions in [`PermissionMode::HumanGate`].
//!
//! Unlike [`super::hooks`] (which veto statically via `before`), the Human Gate
//! SUSPENDS a tool call interactively: when [`is_destructive`] matches, the
//! agent emits an `approval_required` meta-event and awaits a one-shot decision
//! from `resolve_human_gate_cmd`. Approve → the tool runs; Reject → the tool
//! result is `[blocked: 用户拒绝]` and the agent adapts; Retry → the feedback is
//! fed back as the tool result so the agent can correct course.
//!
//! This module owns the approval types so both `commands::agents` (managed
//! state + resolve command) and `react_agent` (the interception point) import
//! from here without a circular `commands ↔ kernel_impl` dependency.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::oneshot;

use tauri::Emitter;

use crate::agents::pty::ChatStreamEvent;
use crate::kernel_impl::hooks::{is_destructive, Action};

/// How long the agent waits for a decision before auto-rejecting. Matches the
/// workflow runner's human-node timeout. Long enough to read a destructive-op
/// preview + type Retry feedback; short enough that a forgotten modal doesn't
/// wedge the run indefinitely.
pub const HUMAN_GATE_TIMEOUT: Duration = Duration::from_secs(300);

/// A user's decision on a suspended tool call, carried over a one-shot channel.
#[derive(Debug, Clone)]
pub enum HumanGateDecision {
    Approve,
    Reject,
    Retry { feedback: String },
}

/// Shared registry of in-flight approvals, keyed by `resume_token`
/// (`approve__{session_id}__{seq}`). Each value is the Sender half of a one-shot
/// channel whose Receiver is held by the suspended tool-call future. Wrapped in
/// `Arc<Mutex>` so the driver task can clone an owned handle into the spawned
/// run. The managed-state wrapper in `commands::agents` is
/// [`AgentApprovalState`](super) around this type.
pub type ApprovalMap = Arc<Mutex<HashMap<String, oneshot::Sender<HumanGateDecision>>>>;

/// The outcome of a [`HumanGateCtx::check`] — what the caller does next.
pub enum HumanGateOutcome {
    /// Not destructive, or the user approved — proceed to invoke the tool.
    Allow,
    /// User rejected (or timed out / session aborted) — the tool does NOT run;
    /// the caller returns this as the tool result so the agent adapts.
    Reject,
    /// User asked to retry with extra guidance — feed `feedback` back as the
    /// tool result (consumes a max-steps iteration).
    Retry(String),
}

/// Per-run context the tool-execution path holds when the agent is in
/// HumanGate mode. Cloned cheaply (AppHandle is `Arc`-inner, approvals is a
/// shared `Arc<Mutex>`); the `seq` counter is shared via `Arc<AtomicU64>` so
/// concurrent tool calls in one turn get distinct resume tokens.
pub struct HumanGateCtx {
    app: tauri::AppHandle,
    session_id: String,
    approvals: ApprovalMap,
    seq: AtomicU64,
}

impl HumanGateCtx {
    pub fn new(app: tauri::AppHandle, session_id: String, approvals: ApprovalMap) -> Self {
        Self {
            app,
            session_id,
            approvals,
            seq: AtomicU64::new(0),
        }
    }

    /// Gate a tool call. If `is_destructive(action, working_dir)`, emit
    /// `approval_required` on `agent:event` and suspend until the user decides
    /// (or [`HUMAN_GATE_TIMEOUT`] auto-rejects). Non-destructive actions return
    /// [`HumanGateOutcome::Allow`] immediately with no emit. Every failure path
    /// is FAIL-SAFE (the destructive op does NOT run): a dropped channel (Sender
    /// reclaimed by abort/clear) → Reject; the 300s timeout → Reject + reclaim;
    /// a best-effort emit glitch → the call still suspends and times out to
    /// Reject. So a UI/IPC glitch never wedges the agent AND never lets a
    /// destructive op slip through unapproved — the catastrophe floor and normal
    /// command guards still apply upstream on top.
    pub async fn check(
        &self,
        action: &Action,
        tool: &str,
        arguments: &str,
        working_dir: &Path,
    ) -> HumanGateOutcome {
        if !is_destructive(action, working_dir) {
            return HumanGateOutcome::Allow;
        }
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let resume_token = format!("approve__{}__{}", self.session_id, seq);
        let rx = {
            let (tx, rx) = oneshot::channel();
            if let Ok(mut g) = self.approvals.lock() {
                g.insert(resume_token.clone(), tx);
            }
            rx
        };
        let summary = describe_destructive(action);
        // Emit the control meta-event. This is NOT a chat block — the frontend
        // listener short-circuits on `approval_required` to open a modal rather
        // than render a card, and it is never persisted into session.blocks.
        let wire = ChatStreamEvent::ApprovalRequired {
            tool: tool.to_string(),
            arguments: arguments.to_string(),
            resume_token: resume_token.clone(),
            summary,
        };
        let _ = self.app.emit(
            "agent:event",
            serde_json::json!({ "sessionId": &self.session_id, "event": &wire }),
        );
        match tokio::time::timeout(HUMAN_GATE_TIMEOUT, rx).await {
            Ok(Ok(HumanGateDecision::Approve)) => HumanGateOutcome::Allow,
            Ok(Ok(HumanGateDecision::Reject)) => HumanGateOutcome::Reject,
            Ok(Ok(HumanGateDecision::Retry { feedback })) => HumanGateOutcome::Retry(feedback),
            // Sender dropped (session aborted / cleared) → auto-reject.
            Ok(Err(_)) => HumanGateOutcome::Reject,
            // Timed out — auto-reject and reclaim the stale Sender.
            Err(_) => {
                if let Ok(mut g) = self.approvals.lock() {
                    g.remove(&resume_token);
                }
                HumanGateOutcome::Reject
            }
        }
    }
}

/// Free-function variant for the resolve command + tests: deliver a decision to
/// the suspended future registered under `resume_token`. `Ok(())` = delivered;
/// `Err(())` = token unknown, already resolved, or the receiver was dropped (the
/// driver future died with a stopped run). The command maps `Err` to NotFound,
/// so a plain unit error suffices — there's nothing to distinguish at the call
/// site (every failure is "no active approval to resolve").
pub fn resolve_approval(
    map: &ApprovalMap,
    resume_token: &str,
    decision: HumanGateDecision,
) -> Result<(), ()> {
    // Hand-rolled: remove then send so a failed send (dropped receiver) still
    // cleans the map entry (remove already happened above).
    let tx = map.lock().ok().and_then(|mut g| g.remove(resume_token));
    match tx {
        Some(tx) => tx.send(decision).map_err(|_| ()),
        None => Err(()),
    }
}

/// Reclaim every pending approval for a session — called on abort so a
/// cancelled run doesn't leak Senders (whose Receivers died with the dropped
/// driver future). Tokens embed the session id as `approve__{sid}__`.
pub fn clear_session_approvals(map: &ApprovalMap, session_id: &str) {
    let prefix = format!("approve__{session_id}__");
    if let Ok(mut g) = map.lock() {
        g.retain(|k, _| !k.starts_with(&prefix));
    }
}

/// One-line human-readable description of WHY this action is destructive —
/// shown as the modal title / summary so the user knows what they're approving.
fn describe_destructive(action: &Action) -> String {
    match action {
        Action::RunCommand { command } => {
            format!("即将执行破坏性命令：{command}")
        }
        Action::WriteFile { path, .. } => {
            format!("即将覆盖已存在的文件：{path}")
        }
        Action::CallTool { tool, .. } => {
            format!("即将执行破坏性操作：{tool}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_session_approvals_drops_only_matching_tokens() {
        let map: ApprovalMap = Arc::new(Mutex::new(HashMap::new()));
        let (tx_a1, _rx_a1) = oneshot::channel();
        let (tx_a2, _rx_a2) = oneshot::channel();
        let (tx_b1, _rx_b1) = oneshot::channel();
        map.lock().unwrap().insert("approve__sess-a__0".into(), tx_a1);
        map.lock().unwrap().insert("approve__sess-a__1".into(), tx_a2);
        map.lock().unwrap().insert("approve__sess-b__0".into(), tx_b1);
        clear_session_approvals(&map, "sess-a");
        let g = map.lock().unwrap();
        assert!(g.keys().all(|k| !k.contains("sess-a")), "sess-a entries cleared");
        assert_eq!(g.len(), 1, "sess-b entry survives");
        assert!(g.contains_key("approve__sess-b__0"));
    }

    #[test]
    fn resolve_approval_delivers_decision_and_removes_entry() {
        let map: ApprovalMap = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = oneshot::channel();
        map.lock().unwrap().insert("approve__s__0".into(), tx);
        // Unknown token → Err (caller maps to NotFound).
        assert!(resolve_approval(&map, "approve__s__99", HumanGateDecision::Approve).is_err());
        // Known token → delivered.
        resolve_approval(&map, "approve__s__0", HumanGateDecision::Reject).unwrap();
        assert!(matches!(rx.try_recv(), Ok(HumanGateDecision::Reject)));
        // Entry removed after resolve.
        assert!(!map.lock().unwrap().contains_key("approve__s__0"));
    }
}
