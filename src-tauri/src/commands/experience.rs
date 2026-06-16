//! Tauri commands for the Forge experience → knowledge replay bridge (T8).
//!
//! - `list_pending_forge_reviews`: surface Forge's pending *mandatory* reviews
//!   (score-too-low, unresolved) so the UI can show them as a review queue.
//! - `replay_forge_experience`: write those reviews' low dimensions into the
//!   knowledge base as `quality_failure` lessons, which the ReactAgent flywheel
//!   (T7) injects into the next run's system prompt.

use crate::db::DbState;
use crate::error::AppError;
use crate::models::AgentType;
use crate::quality::experience::{self, ForgeExperienceReview, ReplayResult};

#[tauri::command]
pub fn list_pending_forge_reviews(
    project_path: String,
) -> Result<Vec<ForgeExperienceReview>, AppError> {
    let path = std::path::Path::new(&project_path);
    let reviews = experience::list_forge_reviews(path)?;
    Ok(experience::pending_mandatory(&reviews)
        .into_iter()
        .cloned()
        .collect())
}

#[tauri::command]
pub fn replay_forge_experience(
    db: tauri::State<'_, DbState>,
    project_path: String,
) -> Result<ReplayResult, AppError> {
    let path = std::path::Path::new(&project_path);
    // Forge missing / nothing to review → empty list, replay is a no-op (not an
    // error): the flywheel just has nothing new to consume this round.
    let reviews = experience::list_forge_reviews(path).unwrap_or_default();
    let pending = experience::pending_mandatory(&reviews);
    let conn = db.get()?;
    Ok(experience::replay_to_knowledge(
        &conn,
        &project_path,
        &pending,
        &AgentType::ClaudeCode,
    ))
}
