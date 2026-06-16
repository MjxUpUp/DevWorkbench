//! Tauri commands exposing shadow-git checkpoints to the frontend: probe
//! whether a session has one (`get_checkpoint`) and roll the agent's changes
//! back (`rollback_to_checkpoint`). See `kernel_impl::checkpoint` for the
//! mechanism and safety invariants.

use crate::kernel_impl::checkpoint::{self, Checkpoint, RollbackResult};

/// Read the checkpoint captured at this session's start. None when no
/// checkpoint exists (session pre-dates the feature, git was unavailable, or
/// the path isn't a repo). The frontend uses this to decide whether to show
/// the "roll back changes" button.
#[tauri::command]
pub fn get_checkpoint(
    project_path: String,
    session_id: String,
) -> Result<Option<Checkpoint>, String> {
    checkpoint::read(&project_path, &session_id)
}

/// Roll the working tree back to the session's checkpoint state. Refuses if
/// HEAD has moved since the checkpoint unless `force` is true. Always captures
/// a pre-rollback snapshot first so the rollback itself is reversible.
#[tauri::command]
pub fn rollback_to_checkpoint(
    project_path: String,
    session_id: String,
    force: Option<bool>,
) -> Result<RollbackResult, String> {
    checkpoint::apply_rollback(&project_path, &session_id, force.unwrap_or(false))
}
