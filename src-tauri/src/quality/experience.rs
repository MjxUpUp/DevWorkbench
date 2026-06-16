//! Forge experience → knowledge replay (v1.2 T8). The external Forge CLI scores
//! every completed task; a low score creates a *mandatory review*
//! (`forge experience list`, `status=pending`). This module bridges those
//! reviews into the knowledge base as `quality_failure` entries, which the
//! ReactAgent experience flywheel (T7) surfaces in the system prompt — so the
//! agent stops repeating the same low-scoring mistakes ("No test file changes
//! detected", "tool-selection: N anti-patterns").
//!
//! Bridge, not reimplementation: Forge stays the source of truth for scoring +
//! review lifecycle (accept/reject/resolve); we only replay the *pending
//! mandatory* ones into the ReactAgent-facing store.

use crate::activity;
use crate::error::AppError;
use crate::models::{AgentType, KnowledgeEntry};
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LowDimension {
    pub dimension: String,
    pub score: f64,
    pub detail: String,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgeExperienceReview {
    // Forge CLI emits snake_case (task_ref / low_dimensions / created_at); the
    // frontend wants camelCase. rename_all=camelCase drives Serialize; the
    // aliases accept forge's snake_case on Deserialize.
    #[serde(alias = "task_ref")]
    pub task_ref: String,
    pub score: f64,
    pub grade: String,
    #[serde(default, alias = "low_dimensions")]
    pub low_dimensions: Vec<LowDimension>,
    pub mandatory: bool,
    pub status: String,
    #[serde(alias = "created_at")]
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExperienceListResponse {
    #[serde(default)]
    reviews: Vec<ForgeExperienceReview>,
}

/// Result of replaying pending mandatory reviews into the knowledge base.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayResult {
    /// New quality_failure lessons written this call.
    pub replayed: usize,
    /// Lessons already present (dedup-skipped — replay is idempotent).
    pub skipped: usize,
}

/// Run `forge experience list --json` and parse the reviews. Empty vec (not an
/// error) when forge is missing or returns nothing — replay is an enhancement,
/// not a gate. Mirrors `forge::run_forge_gate`'s CREATE_NO_WINDOW shell-out.
pub fn list_forge_reviews(project: &Path) -> Result<Vec<ForgeExperienceReview>, AppError> {
    let forge_bin = super::forge::discover_forge().ok_or(AppError::ForgeNotInstalled)?;
    let mut cmd = std::process::Command::new(&forge_bin);
    cmd.arg("experience")
        .arg("list")
        .arg("--json")
        .current_dir(project);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = cmd
        .output()
        .map_err(|e| AppError::Agent(format!("forge experience 执行失败: {e}")))?;
    if !output.status.success() {
        // Non-zero with no JSON → treat as "nothing to review", not a hard error.
        return Ok(Vec::new());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: ExperienceListResponse = serde_json::from_str(&stdout)
        .map_err(|e| AppError::Agent(format!("forge experience 输出解析失败: {e}")))?;
    Ok(parsed.reviews)
}

/// Filter to pending *mandatory* reviews — the ones Forge flagged score-too-low
/// and the user hasn't resolved yet. Pure (no I/O) → unit-testable without forge.
pub fn pending_mandatory(reviews: &[ForgeExperienceReview]) -> Vec<&ForgeExperienceReview> {
    reviews
        .iter()
        .filter(|r| r.status == "pending" && r.mandatory)
        .collect()
}

/// Replay pending mandatory reviews' low dimensions into the knowledge base as
/// `quality_failure` entries. Each low dimension becomes one lesson: distinct
/// task_ref + dimension → distinct content → no dedup collision; replaying the
/// same review twice is idempotent. Returns (new, skipped).
///
/// The skipped count mirrors `add_entry`'s exact dedup key (project_hash +
/// content[:200] chars) so it's accurate, not a guess — important for an honest
/// "N 条经验已重放" report to the user.
pub fn replay_to_knowledge(
    conn: &Connection,
    project_path: &str,
    reviews: &[&ForgeExperienceReview],
    agent_type: &AgentType,
) -> ReplayResult {
    let hash = activity::hash_project_path(project_path);
    let existing: HashSet<String> = crate::knowledge::store::get_entries_for_project(conn, &hash)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.category == "quality_failure")
        .map(|e| e.content.chars().take(200).collect::<String>())
        .collect();

    let now = chrono::Local::now().to_rfc3339();
    let mut replayed = 0usize;
    let mut skipped = 0usize;
    for r in reviews {
        for dim in &r.low_dimensions {
            let content = format!(
                "Forge 任务 {} 评分 {:.1}（{}级）— {}维度仅 {:.0} 分：{}",
                r.task_ref, r.score, r.grade, dim.dimension, dim.score, dim.detail
            );
            let prefix: String = content.chars().take(200).collect();
            if existing.contains(&prefix) {
                skipped += 1;
                continue;
            }
            let entry = KnowledgeEntry {
                id: uuid::Uuid::new_v4().to_string(),
                project_hash: hash.clone(),
                category: "quality_failure".to_string(),
                title: format!("[{}] {}", dim.dimension, dim.detail),
                content,
                source_agent: agent_type.clone(),
                source_session_id: None,
                source_type: "forge_experience".to_string(),
                confidence: 0.85,
                created_at: now.clone(),
                updated_at: now.clone(),
                access_count: 0,
            };
            if crate::knowledge::store::add_entry(conn, &entry).is_ok() {
                replayed += 1;
            } else {
                skipped += 1;
            }
        }
    }
    ReplayResult { replayed, skipped }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::AgentType;

    fn rev(
        task: &str,
        score: f64,
        mandatory: bool,
        status: &str,
        dims: &[(&str, f64, &str)],
    ) -> ForgeExperienceReview {
        ForgeExperienceReview {
            task_ref: task.into(),
            score,
            grade: if score < 60.0 { "D" } else { "C" }.into(),
            low_dimensions: dims
                .iter()
                .map(|(d, s, t)| LowDimension {
                    dimension: (*d).into(),
                    score: *s,
                    detail: (*t).into(),
                })
                .collect(),
            mandatory,
            status: status.into(),
            created_at: "t".into(),
        }
    }

    #[test]
    fn pending_mandatory_filters_status_and_flag() {
        let reviews = vec![
            rev("a", 50.0, true, "pending", &[("testing", 20.0, "no tests")]),
            rev("b", 50.0, true, "resolved", &[("testing", 20.0, "no tests")]),
            rev("c", 75.0, false, "pending", &[("testing", 20.0, "no tests")]),
            rev("d", 50.0, true, "accepted", &[("testing", 20.0, "no tests")]),
        ];
        let got = pending_mandatory(&reviews);
        assert_eq!(got.len(), 1, "only pending+mandatory survives");
        assert_eq!(got[0].task_ref, "a");
    }

    #[test]
    fn parse_real_shape_extracts_reviews() {
        let json = r#"{"reviews":[{"task_ref":"chore/x","score":68.4,"grade":"D","low_dimensions":[{"dimension":"testing","score":20,"detail":"No test file changes detected"}],"mandatory":true,"status":"pending","created_at":"2026-06-14"}]}"#;
        let parsed: ExperienceListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.reviews.len(), 1);
        assert_eq!(parsed.reviews[0].task_ref, "chore/x");
        assert_eq!(parsed.reviews[0].low_dimensions[0].dimension, "testing");
    }

    #[test]
    fn replay_writes_per_dimension_and_dedups_on_repeat() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = db::init_db(&tmp.path().join("t.db")).unwrap();
        let r = rev(
            "feat/a",
            50.0,
            true,
            "pending",
            &[("testing", 20.0, "No test file changes"), ("tool-selection", 20.0, "12 anti-patterns")],
        );
        let refs = vec![&r];
        let res1 = replay_to_knowledge(&conn, "/proj", &refs, &AgentType::ClaudeCode);
        assert_eq!(res1.replayed, 2, "two distinct dimensions → two entries: {res1:?}");
        assert_eq!(res1.skipped, 0);

        // Replay again → idempotent, both skipped.
        let res2 = replay_to_knowledge(&conn, "/proj", &refs, &AgentType::ClaudeCode);
        assert_eq!(res2.replayed, 0);
        assert_eq!(res2.skipped, 2);

        let hash = activity::hash_project_path("/proj");
        let entries = crate::knowledge::store::get_entries_for_project(&conn, &hash).unwrap();
        let qf = entries.iter().filter(|e| e.category == "quality_failure").count();
        assert_eq!(qf, 2, "no duplicates after two replays");
        assert!(entries.iter().all(|e| e.source_type == "forge_experience"));
    }

    #[test]
    fn replay_distinguishes_same_dimension_across_tasks() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = db::init_db(&tmp.path().join("t.db")).unwrap();
        let r1 = rev("feat/a", 50.0, true, "pending", &[("testing", 20.0, "no tests")]);
        let r2 = rev("feat/b", 50.0, true, "pending", &[("testing", 20.0, "no tests")]);
        let refs = vec![&r1, &r2];
        let res = replay_to_knowledge(&conn, "/proj", &refs, &AgentType::ClaudeCode);
        assert_eq!(res.replayed, 2, "distinct task_ref must not collide: {res:?}");
    }
}
