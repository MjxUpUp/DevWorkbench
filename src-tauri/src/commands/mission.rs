//! Mission Mode Tauri commands (D4) — plan/apply two-phase orchestration.
//!
//! Thin wrappers over [`crate::kernel_impl::mission`] artifact I/O. The
//! frontend drives the lifecycle:
//! 1. `mission_init`           — start Phase 1 (writes a plan-phase state.json)
//! 2. agent writes `prd.json`  — (happens inside the agent run, not here)
//! 3. `mission_load_prd`       — validate the agent's prd.json
//! 4. user confirms → `mission_apply` — flip state to Phase 2 (executing)
//! 5. `mission_status`         — poll progress (passed/total stories)
//!
//! These commands own ONLY the artifact/state transitions — they never touch
//! the agent run. The actual Phase 2 execution loop is the agent's run driven
//! by `spawn_agent_session` with `mode: "executing"` (which restricts the
//! toolset in `build_react_agent`). This split mirrors QwenPaw, where
//! `mission_runner.py` owns phases and `handler.py` stays a thin parser.

use crate::error::AppError;
use crate::kernel_impl::mission::{self, MissionState};
use serde::Serialize;
use std::path::PathBuf;

/// Resolve the data dir missions live under (`~/.dev-workbench`).
fn data_dir() -> PathBuf {
    crate::commands::projects::dirs_home().join(".dev-workbench")
}

/// `mission_load_prd` payload: the raw PRD (if present), whether it validates,
/// and the human-readable problems list.
#[derive(Debug, Serialize)]
pub struct MissionLoadResult {
    pub valid: bool,
    pub problems: Vec<String>,
    pub prd: Option<serde_json::Value>,
    /// True if prd.json exists but is unparseable (distinct from "missing" so
    /// the UI can tell the user the file is broken vs not-yet-written).
    pub corrupted: bool,
}

/// `mission_status` payload: phase + iteration + live pass count.
#[derive(Debug, Serialize)]
pub struct MissionStatusView {
    pub state: MissionState,
    pub passed: usize,
    pub total: usize,
    pub corrupted: bool,
}

/// Begin Phase 1: stage the mission dir + a plan-phase state.json. The agent
/// writes `prd.json` during its run; this just ensures the dir exists so later
/// reads resolve instead of 404'ing.
#[tauri::command]
pub async fn mission_init(mission_id: String) -> Result<MissionState, AppError> {
    let dir = data_dir();
    mission::init_mission(&dir, &mission_id)?;
    Ok(MissionState::default())
}

/// Read + validate the agent's prd.json. `valid` is true iff `problems` is
/// empty. Surfaces corruption distinctly from "missing" so the UI can tell the
/// user the file is broken vs not-yet-written.
#[tauri::command]
pub async fn mission_load_prd(mission_id: String) -> Result<MissionLoadResult, AppError> {
    let dir = data_dir();
    let corrupted = mission::is_prd_corrupted(&dir, &mission_id);
    let prd = mission::read_prd(&dir, &mission_id);
    let problems = match &prd {
        Some(p) => mission::validate_prd_json(p),
        None => vec!["prd.json not found or empty".to_string()],
    };
    Ok(MissionLoadResult {
        valid: problems.is_empty(),
        problems,
        prd,
        corrupted,
    })
}

/// User confirmed the PRD → flip state to Phase 2 (executing). Guards against
/// applying without a valid PRD (QwenPaw `run_mission_phase2` startup check) so
/// the controller loop can't start empty-handed.
#[tauri::command]
pub async fn mission_apply(mission_id: String) -> Result<MissionState, AppError> {
    let dir = data_dir();
    let prd = mission::read_prd(&dir, &mission_id).ok_or_else(|| {
        AppError::NotFound("mission_apply: prd.json not found — run Phase 1 first".into())
    })?;
    let problems = mission::validate_prd_json(&prd);
    if !problems.is_empty() {
        return Err(AppError::Config(format!(
            "mission_apply: prd.json invalid — {}",
            problems.join("; ")
        )));
    }
    let mut state = mission::read_state(&dir, &mission_id).unwrap_or_default();
    state.begin_execution();
    mission::write_state(&dir, &mission_id, &state)?;
    Ok(state)
}

/// Live status: current phase/iteration + how many stories have passed.
#[tauri::command]
pub async fn mission_status(mission_id: String) -> Result<MissionStatusView, AppError> {
    let dir = data_dir();
    let state = mission::read_state(&dir, &mission_id).unwrap_or_default();
    let (passed, total) = mission::read_prd(&dir, &mission_id)
        .as_ref()
        .and_then(|p| p.get("userStories")?.as_array())
        .map(|stories| {
            let total = stories.len();
            let passed = stories
                .iter()
                .filter(|s| s.get("passes").and_then(|v| v.as_bool()).unwrap_or(false))
                .count();
            (passed, total)
        })
        .unwrap_or((0, 0));
    Ok(MissionStatusView {
        state,
        passed,
        total,
        corrupted: mission::is_prd_corrupted(&dir, &mission_id),
    })
}
