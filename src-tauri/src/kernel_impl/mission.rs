//! Mission Mode artifacts + state machine (D4 — plan/apply two-phase).
//!
//! Mirrors QwenPaw `agents/mission/mission_runner.py`'s lifecycle:
//! - **Phase 1 (plan)**: full toolset; the agent explores the codebase and
//!   writes `prd.json`. Control returns to the user for confirmation.
//! - **Phase 2 (executing)**: the master becomes a controller — it gets the
//!   read-only + `dispatch_subagent` tool subset (it must delegate all coding
//!   to worker sub-agents) and iterates until every story's `passes` flips
//!   true (mapped onto the Forge `task-verify`/`task-complete` gates).
//!
//! Artifacts persist under `<data_dir>/missions/<mission_id>/`:
//! - `prd.json`  — the requirement (stories + acceptance criteria). Written by
//!   the agent, so it is **untrusted** — `validate_prd_json` checks structure
//!   on the raw [`serde_json::Value`] *before* typed deserialization, exactly
//!   like QwenPaw's dict-level `validate_prd`.
//! - `state.json` — current phase + iteration counter.
//!
//! The phase loop driver itself lives in the Tauri command / `react_chat_driver`
//! layer (it owns the agent run); this module is the pure data + artifact layer,
//! unit-testable in isolation.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Required top-level keys in a valid prd.json (QwenPaw parity).
const REQUIRED_PRD_FIELDS: &[&str] = &["userStories"];
/// Required per-story keys (QwenPaw parity). `passes`/`notes` are optional —
/// they carry serde defaults, so their absence is not a validation failure.
const REQUIRED_STORY_FIELDS: &[&str] = &[
    "id",
    "title",
    "description",
    "acceptanceCriteria",
    "priority",
];

/// A single user story inside a PRD. Serialized camelCase to match the
/// QwenPaw schema the agent is prompted to emit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Story {
    pub id: String,
    pub title: String,
    pub description: String,
    pub acceptance_criteria: Vec<String>,
    pub priority: u32,
    #[serde(default)]
    pub passes: bool,
    #[serde(default)]
    pub notes: String,
}

/// The product requirement document. The agent writes this as camelCase JSON;
/// `from_json_str` round-trips it after [`validate_prd_json`] passes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Prd {
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub branch_name: String,
    #[serde(default)]
    pub description: String,
    pub user_stories: Vec<Story>,
}

impl Prd {
    /// Parse a validated prd.json string into the typed struct. Callers MUST
    /// gate this behind [`validate_prd_json`] — serde alone gives poor errors
    /// for the agent's free-form output.
    pub fn from_json_str(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// True when every story has `passes == true` (QwenPaw completion predicate:
    /// `all(s.get("passes") for s in stories)`).
    pub fn all_pass(&self) -> bool {
        self.user_stories.iter().all(|s| s.passes)
    }
}

/// Mission lifecycle phase. The progression is linear: `Plan` → `Executing`
/// → (`Completed` | `MaxIterationsReached`). Mirrors QwenPaw's
/// `current_phase` values (`prd_generation`/`execution`/`completed`/
/// `max_iterations_reached`), renamed to a typed enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MissionPhase {
    /// Phase 1 — agent is generating/refining the PRD. Read-only (Plan mode).
    #[default]
    Plan,
    /// Phase 2 — controller-only execution loop, delegating to sub-agents.
    Executing,
    /// All stories passed — mission succeeded.
    Completed,
    /// Iteration budget exhausted before all stories passed.
    MaxIterationsReached,
}

/// Persisted mission state. Lives in `state.json` next to the PRD.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MissionState {
    #[serde(default)]
    pub current_phase: MissionPhase,
    #[serde(default)]
    pub iteration: u32,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
}

fn default_max_iterations() -> u32 {
    20
}

impl Default for MissionState {
    fn default() -> Self {
        Self {
            current_phase: MissionPhase::Plan,
            iteration: 0,
            max_iterations: 20,
        }
    }
}

impl MissionState {
    /// Transition Plan → Executing (user confirmed the PRD / "apply").
    pub fn begin_execution(&mut self) {
        self.current_phase = MissionPhase::Executing;
        self.iteration = 0;
    }

    /// Record one completed execution iteration. Returns `true` if the budget
    /// is now exhausted (caller should mark `MaxIterationsReached`); `false`
    /// if another iteration is still allowed.
    pub fn record_iteration(&mut self) -> bool {
        self.iteration += 1;
        self.iteration >= self.max_iterations
    }

    pub fn mark_completed(&mut self) {
        self.current_phase = MissionPhase::Completed;
    }

    pub fn mark_max_iterations(&mut self) {
        self.current_phase = MissionPhase::MaxIterationsReached;
    }
}

// ── Artifact paths ───────────────────────────────────────────────────────

/// Root directory holding a mission's artifacts.
pub fn mission_dir(data_dir: &Path, mission_id: &str) -> PathBuf {
    data_dir.join("missions").join(mission_id)
}

pub fn prd_path(data_dir: &Path, mission_id: &str) -> PathBuf {
    mission_dir(data_dir, mission_id).join("prd.json")
}

pub fn state_path(data_dir: &Path, mission_id: &str) -> PathBuf {
    mission_dir(data_dir, mission_id).join("state.json")
}

// ── PRD validation (QwenPaw parity) ──────────────────────────────────────

/// Validate raw prd.json content. Returns human-readable problems (empty =
/// valid). Operates on [`serde_json::Value`] — NOT the typed [`Prd`] — so we
/// catch malformed structure with good messages, mirroring QwenPaw's
/// dict-level `validate_prd` (the agent writes this file; it's untrusted).
///
/// Required: top-level `userStories` (non-empty array), each story an object
/// with `id`/`title`/`description`/`acceptanceCriteria`/`priority`.
pub fn validate_prd_json(prd: &serde_json::Value) -> Vec<String> {
    let mut problems = Vec::new();

    let obj = match prd.as_object() {
        Some(o) => o,
        None => return vec!["prd.json is not a JSON object".to_string()],
    };

    for required in REQUIRED_PRD_FIELDS {
        if !obj.contains_key(*required) {
            problems.push(format!("Missing top-level '{required}' array"));
            return problems;
        }
    }

    let stories = match obj.get("userStories").and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => {
            problems.push("'userStories' must be a non-empty array".to_string());
            return problems;
        }
    };

    for (i, story) in stories.iter().enumerate() {
        let s = match story.as_object() {
            Some(o) => o,
            None => {
                problems.push(format!("userStories[{i}] is not an object"));
                continue;
            }
        };
        let mut missing: Vec<&str> = REQUIRED_STORY_FIELDS
            .iter()
            .copied()
            .filter(|f| !s.contains_key(*f))
            .collect();
        if missing.is_empty() {
            continue;
        }
        missing.sort();
        let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        problems.push(format!(
            "userStories[{i}] ('{id}') missing fields: {}",
            missing.join(", ")
        ));
    }

    problems
}

// ── Artifact I/O ─────────────────────────────────────────────────────────

/// Read prd.json as raw JSON. `None` if missing or empty (QwenPaw treats both
/// as "not found"). A corrupt (unparseable) file is surfaced as `None` here;
/// callers that need to distinguish corruption use [`is_prd_corrupted`].
pub fn read_prd(data_dir: &Path, mission_id: &str) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(prd_path(data_dir, mission_id)).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    serde_json::from_str(&text).ok()
}

/// True if prd.json exists but is unparseable (QwenPaw `_is_json_corrupted`).
pub fn is_prd_corrupted(data_dir: &Path, mission_id: &str) -> bool {
    let text = match std::fs::read_to_string(prd_path(data_dir, mission_id)) {
        Ok(t) => t,
        Err(_) => return false,
    };
    if text.trim().is_empty() {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(&text).is_err()
}

/// Read state.json; `None` if missing or unparseable.
pub fn read_state(data_dir: &Path, mission_id: &str) -> Option<MissionState> {
    let text = std::fs::read_to_string(state_path(data_dir, mission_id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Persist mission state, creating the mission dir if needed.
pub fn write_state(
    data_dir: &Path,
    mission_id: &str,
    state: &MissionState,
) -> std::io::Result<()> {
    std::fs::create_dir_all(mission_dir(data_dir, mission_id))?;
    let json = serde_json::to_string_pretty(state)
        .expect("MissionState serialization is infallible");
    std::fs::write(state_path(data_dir, mission_id), format!("{json}\n"))
}

/// Initialize a fresh mission: write a default (plan-phase) state.json. The
/// agent writes prd.json itself during Phase 1; this just stages the dir +
/// state so subsequent reads resolve.
pub fn init_mission(data_dir: &Path, mission_id: &str) -> std::io::Result<()> {
    write_state(data_dir, mission_id, &MissionState::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── validate_prd_json (QwenPaw parity) ────────────────────────────────

    fn valid_prd() -> serde_json::Value {
        json!({
            "project": "demo",
            "branchName": "mission/demo",
            "description": "a demo mission",
            "userStories": [
                {
                    "id": "US-001",
                    "title": "first",
                    "description": "As a user, I want X",
                    "acceptanceCriteria": ["criterion 1"],
                    "priority": 1,
                    "passes": false,
                    "notes": ""
                }
            ]
        })
    }

    #[test]
    fn validate_accepts_a_well_formed_prd() {
        assert!(validate_prd_json(&valid_prd()).is_empty());
    }

    #[test]
    fn validate_rejects_non_object() {
        assert_eq!(
            validate_prd_json(&json!([1, 2, 3])),
            vec!["prd.json is not a JSON object"]
        );
        assert_eq!(
            validate_prd_json(&json!("a string")),
            vec!["prd.json is not a JSON object"]
        );
    }

    #[test]
    fn validate_rejects_missing_user_stories_and_early_returns() {
        // QwenPaw returns immediately after the missing-field check, so we
        // surface exactly one problem, not a cascade.
        let probs = validate_prd_json(&json!({"project": "x"}));
        assert_eq!(probs, vec!["Missing top-level 'userStories' array"]);
    }

    #[test]
    fn validate_rejects_empty_user_stories() {
        let probs = validate_prd_json(&json!({"userStories": []}));
        assert_eq!(probs, vec!["'userStories' must be a non-empty array"]);
    }

    #[test]
    fn validate_rejects_non_array_user_stories() {
        let probs = validate_prd_json(&json!({"userStories": "not a list"}));
        assert_eq!(probs, vec!["'userStories' must be a non-empty array"]);
    }

    #[test]
    fn validate_reports_missing_story_fields_sorted() {
        let prd = json!({
            "userStories": [
                {"id": "US-001", "title": "t"}
            ]
        });
        let probs = validate_prd_json(&prd);
        // Fields reported sorted; passes/notes absent is NOT a failure.
        assert_eq!(
            probs,
            vec![
                "userStories[0] ('US-001') missing fields: acceptanceCriteria, description, priority"
            ]
        );
    }

    #[test]
    fn validate_reports_non_object_story_and_continues() {
        let prd = json!({
            "userStories": [
                "not an object",
                {"id": "US-002", "title": "t", "description": "d", "acceptanceCriteria": ["c"], "priority": 1}
            ]
        });
        let probs = validate_prd_json(&prd);
        // First story is malformed; second is fine → exactly one problem.
        assert_eq!(probs, vec!["userStories[0] is not an object"]);
    }

    #[test]
    fn validate_uses_question_mark_when_id_missing() {
        let prd = json!({"userStories": [{"title": "t"}]});
        let probs = validate_prd_json(&prd);
        assert_eq!(probs.len(), 1);
        assert!(probs[0].contains("'?'"), "got: {}", probs[0]);
    }

    // ── Prd typed round-trip ──────────────────────────────────────────────

    #[test]
    fn prd_round_trips_camel_case_json() {
        let text = serde_json::to_string(&valid_prd()).unwrap();
        let prd = Prd::from_json_str(&text).unwrap();
        assert_eq!(prd.user_stories.len(), 1);
        assert_eq!(prd.user_stories[0].id, "US-001");
        assert_eq!(prd.user_stories[0].acceptance_criteria, vec!["criterion 1"]);
        assert_eq!(prd.branch_name, "mission/demo");
        assert!(!prd.all_pass());
    }

    #[test]
    fn all_pass_true_only_when_every_story_passes() {
        let mut prd = Prd::from_json_str(&serde_json::to_string(&valid_prd()).unwrap()).unwrap();
        assert!(!prd.all_pass());
        prd.user_stories[0].passes = true;
        assert!(prd.all_pass());
        prd.user_stories.push(Story {
            id: "US-002".into(),
            title: "second".into(),
            description: "d".into(),
            acceptance_criteria: vec!["c".into()],
            priority: 2,
            passes: false,
            notes: String::new(),
        });
        assert!(!prd.all_pass(), "one failing story breaks all_pass");
    }

    // ── MissionState transitions ──────────────────────────────────────────

    #[test]
    fn default_state_starts_in_plan_phase() {
        let s = MissionState::default();
        assert_eq!(s.current_phase, MissionPhase::Plan);
        assert_eq!(s.iteration, 0);
        assert_eq!(s.max_iterations, 20);
    }

    #[test]
    fn begin_execution_resets_iteration_counter() {
        let mut s = MissionState {
            iteration: 5,
            ..Default::default()
        };
        s.begin_execution();
        assert_eq!(s.current_phase, MissionPhase::Executing);
        assert_eq!(s.iteration, 0, "entering Phase 2 resets the counter");
    }

    #[test]
    fn record_iteration_signals_budget_exhaustion_at_max() {
        let mut s = MissionState {
            max_iterations: 3,
            ..Default::default()
        };
        s.begin_execution();
        assert!(!s.record_iteration(), "iter 1/3 — budget remains");
        assert_eq!(s.iteration, 1);
        assert!(!s.record_iteration(), "iter 2/3 — budget remains");
        assert!(s.record_iteration(), "iter 3/3 — budget now exhausted");
        s.mark_max_iterations();
        assert_eq!(s.current_phase, MissionPhase::MaxIterationsReached);
    }

    #[test]
    fn mark_completed_sets_terminal_phase() {
        let mut s = MissionState::default();
        s.mark_completed();
        assert_eq!(s.current_phase, MissionPhase::Completed);
    }

    // ── Artifact I/O ──────────────────────────────────────────────────────

    fn temp_data_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("create temp dir")
    }

    #[test]
    fn init_then_read_state_round_trips() {
        let dir = temp_data_dir();
        let data_dir = dir.path();
        init_mission(data_dir, "m1").unwrap();
        let state = read_state(data_dir, "m1").expect("state.json written by init");
        assert_eq!(state, MissionState::default());
        // Paths nest under missions/<id>/.
        assert!(prd_path(data_dir, "m1").ends_with("missions/m1/prd.json"));
    }

    #[test]
    fn read_prd_none_when_missing_or_empty() {
        let dir = temp_data_dir();
        let data_dir = dir.path();
        assert!(read_prd(data_dir, "nope").is_none());
        init_mission(data_dir, "empty").unwrap();
        // No prd.json written yet → None.
        assert!(read_prd(data_dir, "empty").is_none());
    }

    #[test]
    fn write_and_read_back_prd_value() {
        let dir = temp_data_dir();
        let data_dir = dir.path();
        init_mission(data_dir, "m2").unwrap();
        let prd = valid_prd();
        std::fs::write(
            prd_path(data_dir, "m2"),
            serde_json::to_string_pretty(&prd).unwrap(),
        )
        .unwrap();
        let read_back = read_prd(data_dir, "m2").expect("prd.json present");
        assert_eq!(read_back, prd);
        assert!(validate_prd_json(&read_back).is_empty());
    }

    #[test]
    fn is_prd_corrupted_distinguishes_parse_failure_from_missing() {
        let dir = temp_data_dir();
        let data_dir = dir.path();
        // Missing → not "corrupted" (corruption requires the file to exist).
        assert!(!is_prd_corrupted(data_dir, "absent"));
        init_mission(data_dir, "bad").unwrap();
        std::fs::write(prd_path(data_dir, "bad"), "{ not valid json").unwrap();
        assert!(is_prd_corrupted(data_dir, "bad"));
        std::fs::write(prd_path(data_dir, "bad"), "{}").unwrap();
        // Valid JSON (even an empty object) is not corrupted.
        assert!(!is_prd_corrupted(data_dir, "bad"));
    }

    #[test]
    fn write_state_persists_advanced_phase() {
        let dir = temp_data_dir();
        let data_dir = dir.path();
        let mut state = MissionState::default();
        state.begin_execution();
        state.record_iteration();
        write_state(data_dir, "m3", &state).unwrap();
        let back = read_state(data_dir, "m3").unwrap();
        assert_eq!(back.current_phase, MissionPhase::Executing);
        assert_eq!(back.iteration, 1);
    }
}
