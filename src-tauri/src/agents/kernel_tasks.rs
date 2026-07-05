//! Tracks in-flight kernel ReactAgent chat tasks so they can be cancelled.
//!
//! `AgentProcesses` (in `pty.rs`) holds OS PIDs for CLI child processes — kill
//! by PID. A kernel agent is different: it runs as an async task on the Tauri
//! tokio runtime (driving a `BoxStream<AgentEvent>`), so there's no PID.
//! Cancellation is `JoinHandle::abort()`, which drops the future (and the
//! `BoxStream`/`generate()` future inside it) mid-await.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

use crate::agents::pty::ChatStreamEvent;

/// Active kernel agent tasks keyed by session ID. Inserted by the spawn driver
/// right after `tokio::spawn`; removed by the driver when the run finishes, or
/// aborted by `stop_agent_session` to cancel a running task.
///
/// Held as Tauri managed state (`lib.rs` → `.manage(KernelTasks::default())`).
#[derive(Default)]
pub struct KernelTasks(Mutex<HashMap<String, JoinHandle<()>>>);

impl KernelTasks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a just-spawned driver task so it can be aborted later. If a task
    /// is already registered for this session (shouldn't happen in normal flow),
    /// abort the stale one first to avoid leaking it.
    pub fn insert(&self, session_id: &str, handle: JoinHandle<()>) {
        if let Some(prev) = self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(session_id.to_string(), handle) {
            prev.abort();
        }
    }

    /// Remove a finished task from the table (called by the driver once its run
    /// completes, so the entry doesn't linger). Returns false if the session
    /// was already removed/aborted — harmless, the driver just doesn't double-clean.
    pub fn remove(&self, session_id: &str) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(session_id).is_some()
    }

    /// Abort a running task. Returns true if a kernel task was found and
    /// aborted, false if the session wasn't a kernel task — the caller then
    /// falls back to the pty/PID stop path. Aborting a task that already
    /// finished is a no-op (abort on a completed handle does nothing).
    pub fn abort(&self, session_id: &str) -> bool {
        if let Some(handle) = self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(session_id) {
            handle.abort();
            true
        } else {
            false
        }
    }
}

/// Live chat-event buffer per running kernel session (gap4: interrupt
/// persistence).
///
/// `final_blocks` — the driver's accumulated transcript — lives on the spawned
/// task's stack, so it is dropped when [`KernelTasks::abort`] cancels the
/// future. `stop_agent_session` (a separate Tauri command) therefore cannot
/// read it, and a mid-run stop used to lose the entire streamed-so-far turn
/// (status=cancelled, blocks=NULL). This map holds a parallel buffer the driver
/// pushes every event into, so the stop command can drain it post-abort and
/// persist the transcript — with a synthetic `tool_result` appended for any
/// trailing unmatched `tool_use` — instead of dropping the whole turn.
///
/// Removed on normal completion (`remove`, after the driver has written its
/// own `final_blocks`) and on stop (`take`). Mirrors the `compaction_blocks`
/// pattern but lives in managed state because it crosses command boundaries.
#[derive(Default)]
pub struct KernelLiveBlocks(Mutex<HashMap<String, Arc<Mutex<Vec<ChatStreamEvent>>>>>);

impl KernelLiveBlocks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a live buffer for a session at spawn time. Replaces (and drops)
    /// any stale entry for the same id; the matching `JoinHandle` is handled by
    /// [`KernelTasks::insert`], which aborts the stale task.
    pub fn insert(&self, session_id: &str, buf: Arc<Mutex<Vec<ChatStreamEvent>>>) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session_id.to_string(), buf);
    }

    /// Remove and return the buffer. `None` if the session already finalized
    /// (driver removed it on normal completion) or was never a kernel session.
    pub fn take(&self, session_id: &str) -> Option<Arc<Mutex<Vec<ChatStreamEvent>>>> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_id)
    }

    /// Drop the entry without returning the buffer — the driver's normal-
    /// completion path already has its own `final_blocks` and doesn't need the
    /// live copy. Keeps the map from leaking entries for finished sessions.
    pub fn remove(&self, session_id: &str) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn abort_registered_task_returns_true() {
        let tasks = KernelTasks::new();
        tasks.insert("s1", tokio::spawn(async {}));
        assert!(tasks.abort("s1"));
        // Second abort finds nothing (already removed).
        assert!(!tasks.abort("s1"));
    }

    #[tokio::test]
    async fn abort_unknown_session_returns_false() {
        let tasks = KernelTasks::new();
        assert!(!tasks.abort("never-spawned"));
    }

    #[tokio::test]
    async fn remove_then_abort_returns_false() {
        let tasks = KernelTasks::new();
        tasks.insert("s1", tokio::spawn(async {}));
        assert!(tasks.remove("s1"));
        assert!(!tasks.abort("s1"));
    }

    #[tokio::test]
    async fn insert_replacing_aborts_previous_handle() {
        // A long-running task that would block forever if not aborted. Replacing
        // the entry for the same session must abort it so it doesn't leak.
        let tasks = KernelTasks::new();
        let long = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        tasks.insert("s1", long);
        tasks.insert("s1", tokio::spawn(async {}));
        // Yield so the aborted task actually observes cancellation.
        tokio::time::sleep(Duration::from_millis(50)).await;
        // The replacement is still registered and removable.
        assert!(tasks.abort("s1"));
    }

    #[test]
    fn live_blocks_insert_take_roundtrip() {
        // gap4: the live mirror must hand back the exact Arc registered, and
        // take() must remove the entry so a second take misses.
        let lb = KernelLiveBlocks::new();
        let buf = Arc::new(Mutex::new(vec![ChatStreamEvent::Text {
            content: "x".into(),
        }]));
        lb.insert("s1", Arc::clone(&buf));
        let got = lb
            .take("s1")
            .expect("inserted buffer must be retrievable");
        assert!(Arc::ptr_eq(&got, &buf));
        assert!(lb.take("s1").is_none(), "take removes the entry");
    }

    #[test]
    fn live_blocks_take_unknown_session_none() {
        let lb = KernelLiveBlocks::new();
        assert!(lb.take("never-spawned").is_none());
    }

    #[test]
    fn live_blocks_remove_drops_entry_without_returning_buffer() {
        // The driver's normal-completion path calls remove (not take): it
        // already has its own final_blocks and doesn't need the live copy.
        let lb = KernelLiveBlocks::new();
        let buf = Arc::new(Mutex::new(Vec::new()));
        lb.insert("s1", Arc::clone(&buf));
        lb.remove("s1");
        assert!(lb.take("s1").is_none());
        // The caller's Arc is unaffected — remove only drops the map entry.
        assert_eq!(Arc::strong_count(&buf), 1);
    }
}
