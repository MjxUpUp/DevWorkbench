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

/// Virtual project_hash for the GLOBAL experience layer (D6). Lessons written
/// under this hash are cross-project: every project's `experience_prompt_suffix`
/// pulls them in addition to its own, so a quality lesson learned in one project
/// (e.g. "no test file changes detected") benefits the others instead of being
/// re-learned the hard way. Replay promotes one global lesson per *dimension*
/// (deduped by title `[通用] {dimension}`), so the global layer aggregates
/// recurring failure modes without per-task noise.
pub const GLOBAL_PROJECT_HASH: &str = "__global__";

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
    /// First-time cross-project (global) lessons promoted this call — one per
    /// newly-seen dimension. Subsequent reviews of an already-global dimension
    /// do NOT re-promote (deduped by `[通用] {dimension}` title).
    pub promoted_global: usize,
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

    // Global layer: one lesson per dimension (deduped by title). Promoted on
    // FIRST sight of a dimension so every project benefits immediately; later
    // projects (or a second review in the same call) with the same dimension
    // find it already present and skip. The set is mutable so a dimension seen
    // twice in ONE replay call promotes once, not twice — the authority for
    // `promoted_global` is set membership, not add_entry's silent content dedup
    // (which returns Ok on a dup and would otherwise double-count). The global
    // detail is whatever the first-seen review reported (first-write wins);
    // acceptable for v1 — budget selection + confidence decay keep the injected
    // set small and relevant.
    let mut global_titles: HashSet<String> =
        crate::knowledge::store::get_entries_for_project(conn, GLOBAL_PROJECT_HASH)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.category == "quality_failure")
            .map(|e| e.title)
            .collect();

    let now = chrono::Local::now().to_rfc3339();
    let mut replayed = 0usize;
    let mut skipped = 0usize;
    let mut promoted_global = 0usize;
    for r in reviews {
        for dim in &r.low_dimensions {
            let content = format!(
                "Forge 任务 {} 评分 {:.1}（{}级）— {}维度仅 {:.0} 分：{}",
                r.task_ref, r.score, r.grade, dim.dimension, dim.score, dim.detail
            );
            let prefix: String = content.chars().take(200).collect();
            if existing.contains(&prefix) {
                skipped += 1;
            } else {
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

            // Global promotion: dimension-level lesson under __global__, shared
            // across projects. Lower confidence (0.7) — it's an inferred
            // cross-project pattern, not a project-specific finding. The set is
            // the dedup authority: insert BEFORE add_entry so a second sighting
            // of the same dimension (in this call or a later one) never
            // double-counts, regardless of add_entry's own content dedup.
            let g_title = format!("[通用] {}", dim.dimension);
            if global_titles.insert(g_title.clone()) {
                let g_content = format!(
                    "跨项目反复出现的质量短板（{dim} 维度）：{detail}",
                    dim = dim.dimension,
                    detail = dim.detail
                );
                let g_entry = KnowledgeEntry {
                    id: uuid::Uuid::new_v4().to_string(),
                    project_hash: GLOBAL_PROJECT_HASH.to_string(),
                    category: "quality_failure".to_string(),
                    title: g_title,
                    content: g_content,
                    source_agent: agent_type.clone(),
                    source_session_id: None,
                    source_type: "forge_experience_global".to_string(),
                    confidence: 0.7,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                    access_count: 0,
                };
                let _ = crate::knowledge::store::add_entry(conn, &g_entry);
                promoted_global += 1;
            }
        }
    }
    ReplayResult {
        replayed,
        skipped,
        promoted_global,
    }
}

/// Reviews the user has ACCEPTED — their lessons leave the knowledge base
/// ENTIRELY (full flywheel exit via [`purge_lessons_for_resolved_reviews`]).
/// "Accepted" = the user signed off on the improvement; the lesson no longer
/// needs to nag. Pure (no I/O) → unit-testable.
pub fn accepted_only(reviews: &[ForgeExperienceReview]) -> Vec<&ForgeExperienceReview> {
    reviews
        .iter()
        .filter(|r| r.status == "accepted" && r.mandatory)
        .collect()
}

/// Reviews the user has RESOLVED but NOT accepted — their lessons stay in the
/// knowledge base but get their confidence DECAYED (improvement tracking via
/// [`decay_confidence_for_resolved_reviews`]). "Resolved" = addressed, but the
/// pattern is worth keeping on record (it may recur), so the flywheel soft-exits
/// instead of hard-deleting. Pure (no I/O) → unit-testable.
pub fn resolved_not_accepted(reviews: &[ForgeExperienceReview]) -> Vec<&ForgeExperienceReview> {
    reviews
        .iter()
        .filter(|r| r.status == "resolved" && r.mandatory)
        .collect()
}

/// Decay the confidence of project-local `quality_failure` lessons whose review
/// was RESOLVED (not accepted) — the soft-exit counterpart to replay. The lesson
/// stays (improvement tracking) but its confidence halves (floored at 0.1) so it
/// sorts behind fresh lessons and eventually falls out of the token-budget
/// window. Matches the same `"Forge 任务 {task_ref} 评分"` marker as purge (whole
/// marker — never a bare task_ref substring, so a short ref that happens to be a
/// substring of another never cross-decays). GLOBAL lessons are NOT decayed
/// here: they are cross-project aggregates (`source_type = forge_experience_global`),
/// not tied to one project's resolve. Returns the count of lessons decayed.
pub fn decay_confidence_for_resolved_reviews(
    conn: &Connection,
    project_hash: &str,
    reviews: &[&ForgeExperienceReview],
) -> usize {
    if reviews.is_empty() {
        return 0;
    }
    let markers: Vec<String> = reviews
        .iter()
        .map(|r| format!("Forge 任务 {} 评分", r.task_ref))
        .collect();
    let candidates = crate::knowledge::store::get_entries_for_project(conn, project_hash)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.category == "quality_failure" && e.source_type == "forge_experience");
    let mut decayed = 0usize;
    for entry in candidates {
        if markers.iter().any(|m| entry.content.contains(m)) {
            let new_conf = (entry.confidence * 0.5).max(0.1);
            if crate::knowledge::store::set_entry_confidence(conn, &entry.id, new_conf).is_ok() {
                decayed += 1;
            }
        }
    }
    decayed
}

/// Reverse of [`replay_to_knowledge`]: remove the `quality_failure` lessons
/// written for now-resolved/accepted reviews, closing the flywheel's exit.
///
/// A lesson belongs to a review iff its content carries the review's task_ref
/// (replay embeds `"Forge 任务 {task_ref} 评分 …"`). We match the WHOLE
/// `"Forge 任务 {task_ref} 评分"` marker — not a bare task_ref substring — so a
/// short task_ref that happens to be a substring of another never causes
/// cross-purging. One resolve removes every dimension that review produced.
/// Returns the count removed.
pub fn purge_lessons_for_resolved_reviews(
    conn: &Connection,
    project_hash: &str,
    reviews: &[&ForgeExperienceReview],
) -> usize {
    if reviews.is_empty() {
        return 0;
    }
    let markers: Vec<String> = reviews
        .iter()
        .map(|r| format!("Forge 任务 {} 评分", r.task_ref))
        .collect();
    let candidates = crate::knowledge::store::get_entries_for_project(conn, project_hash)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.category == "quality_failure" && e.source_type == "forge_experience");
    let mut removed = 0usize;
    for entry in candidates {
        if markers.iter().any(|m| entry.content.contains(m))
            && crate::knowledge::store::delete_entry(conn, &entry.id).is_ok()
        {
            removed += 1;
        }
    }
    removed
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
            rev(
                "b",
                50.0,
                true,
                "resolved",
                &[("testing", 20.0, "no tests")],
            ),
            rev(
                "c",
                75.0,
                false,
                "pending",
                &[("testing", 20.0, "no tests")],
            ),
            rev(
                "d",
                50.0,
                true,
                "accepted",
                &[("testing", 20.0, "no tests")],
            ),
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
            &[
                ("testing", 20.0, "No test file changes"),
                ("tool-selection", 20.0, "12 anti-patterns"),
            ],
        );
        let refs = vec![&r];
        let res1 = replay_to_knowledge(&conn, "/proj", &refs, &AgentType::ClaudeCode);
        assert_eq!(
            res1.replayed, 2,
            "two distinct dimensions → two entries: {res1:?}"
        );
        assert_eq!(res1.skipped, 0);
        // Both dimensions are first-seen → two global lessons promoted.
        assert_eq!(
            res1.promoted_global, 2,
            "one global per new dimension: {res1:?}"
        );

        // Replay again → idempotent: project-local both skipped, global both
        // already present (no re-promotion).
        let res2 = replay_to_knowledge(&conn, "/proj", &refs, &AgentType::ClaudeCode);
        assert_eq!(res2.replayed, 0);
        assert_eq!(res2.skipped, 2);
        assert_eq!(res2.promoted_global, 0, "global deduped by dimension title");

        let hash = activity::hash_project_path("/proj");
        let entries = crate::knowledge::store::get_entries_for_project(&conn, &hash).unwrap();
        let qf = entries
            .iter()
            .filter(|e| e.category == "quality_failure")
            .count();
        assert_eq!(qf, 2, "no duplicates after two replays");
        assert!(entries.iter().all(|e| e.source_type == "forge_experience"));

        // Global layer holds exactly one lesson per dimension (2), each titled
        // `[通用] {dimension}` and source_type forge_experience_global.
        let globals =
            crate::knowledge::store::get_entries_for_project(&conn, GLOBAL_PROJECT_HASH).unwrap();
        let gqf: Vec<_> = globals
            .iter()
            .filter(|e| e.category == "quality_failure")
            .collect();
        assert_eq!(gqf.len(), 2, "one global per dimension: {gqf:?}");
        assert!(gqf
            .iter()
            .all(|e| e.source_type == "forge_experience_global"));
        assert!(globals.iter().any(|e| e.title == "[通用] testing"));
        assert!(globals.iter().any(|e| e.title == "[通用] tool-selection"));
    }

    #[test]
    fn replay_distinguishes_same_dimension_across_tasks() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = db::init_db(&tmp.path().join("t.db")).unwrap();
        let r1 = rev(
            "feat/a",
            50.0,
            true,
            "pending",
            &[("testing", 20.0, "no tests")],
        );
        let r2 = rev(
            "feat/b",
            50.0,
            true,
            "pending",
            &[("testing", 20.0, "no tests")],
        );
        let refs = vec![&r1, &r2];
        let res = replay_to_knowledge(&conn, "/proj", &refs, &AgentType::ClaudeCode);
        assert_eq!(
            res.replayed, 2,
            "distinct task_ref must not collide: {res:?}"
        );
        // Same dimension twice → only ONE global lesson (deduped by title).
        assert_eq!(
            res.promoted_global, 1,
            "same dimension → one global: {res:?}"
        );
    }

    #[test]
    fn accepted_only_filters_status_and_flag() {
        let reviews = vec![
            rev("a", 50.0, true, "pending", &[("testing", 20.0, "no tests")]),
            rev(
                "b",
                50.0,
                true,
                "resolved",
                &[("testing", 20.0, "no tests")],
            ),
            rev(
                "c",
                50.0,
                true,
                "accepted",
                &[("testing", 20.0, "no tests")],
            ),
            rev(
                "d",
                50.0,
                false,
                "accepted",
                &[("testing", 20.0, "no tests")],
            ),
            rev(
                "e",
                50.0,
                true,
                "rejected",
                &[("testing", 20.0, "no tests")],
            ),
        ];
        let got = accepted_only(&reviews);
        let refs: Vec<&str> = got.iter().map(|r| r.task_ref.as_str()).collect();
        assert_eq!(refs, vec!["c"], "only accepted + mandatory");
    }

    #[test]
    fn resolved_not_accepted_filters_status_and_flag() {
        let reviews = vec![
            rev("a", 50.0, true, "pending", &[("testing", 20.0, "no tests")]),
            rev(
                "b",
                50.0,
                true,
                "resolved",
                &[("testing", 20.0, "no tests")],
            ),
            rev(
                "c",
                50.0,
                true,
                "accepted",
                &[("testing", 20.0, "no tests")],
            ),
            rev(
                "d",
                50.0,
                false,
                "resolved",
                &[("testing", 20.0, "no tests")],
            ),
            rev(
                "e",
                50.0,
                true,
                "rejected",
                &[("testing", 20.0, "no tests")],
            ),
        ];
        let got = resolved_not_accepted(&reviews);
        let refs: Vec<&str> = got.iter().map(|r| r.task_ref.as_str()).collect();
        assert_eq!(refs, vec!["b"], "only resolved (not accepted) + mandatory");
    }

    #[test]
    fn purge_removes_resolved_review_lessons_keeps_unrelated() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = db::init_db(&tmp.path().join("t.db")).unwrap();
        let hash = activity::hash_project_path("/proj");

        // Replay two pending reviews → 2 lessons (one dimension each).
        let r1 = rev(
            "feat/a",
            50.0,
            true,
            "pending",
            &[("testing", 20.0, "no tests")],
        );
        let r2 = rev("feat/b", 50.0, true, "pending", &[("tooling", 20.0, "x")]);
        let res = replay_to_knowledge(&conn, "/proj", &[&r1, &r2], &AgentType::ClaudeCode);
        assert_eq!(res.replayed, 2);

        // feat/a is now resolved → purge ONLY its lesson (the exit side of the
        // flywheel — previously lessons accumulated forever after a resolve).
        let r1_resolved = rev(
            "feat/a",
            50.0,
            true,
            "resolved",
            &[("testing", 20.0, "no tests")],
        );
        let removed = purge_lessons_for_resolved_reviews(&conn, &hash, &[&r1_resolved]);
        assert_eq!(removed, 1);

        let qf = crate::knowledge::store::get_entries_for_project(&conn, &hash)
            .unwrap()
            .into_iter()
            .filter(|e| e.category == "quality_failure")
            .collect::<Vec<_>>();
        assert_eq!(qf.len(), 1, "feat/b's lesson survives the purge");
        assert!(qf[0].content.contains("feat/b"));
    }

    #[test]
    fn purge_is_noop_with_empty_resolved_list() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = db::init_db(&tmp.path().join("t.db")).unwrap();
        let hash = activity::hash_project_path("/proj");
        let r = rev(
            "feat/a",
            50.0,
            true,
            "pending",
            &[("testing", 20.0, "no tests")],
        );
        replay_to_knowledge(&conn, "/proj", &[&r], &AgentType::ClaudeCode);
        assert_eq!(purge_lessons_for_resolved_reviews(&conn, &hash, &[]), 0);
    }

    #[test]
    fn decay_halves_confidence_of_resolved_only() {
        // feat/a resolved (not accepted) → its project-local lesson's confidence
        // halves; the lesson STAYS (improvement tracking, unlike purge's delete).
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = db::init_db(&tmp.path().join("t.db")).unwrap();
        let hash = activity::hash_project_path("/proj");
        let r = rev(
            "feat/a",
            50.0,
            true,
            "pending",
            &[("testing", 20.0, "no tests")],
        );
        replay_to_knowledge(&conn, "/proj", &[&r], &AgentType::ClaudeCode);

        let before = crate::knowledge::store::get_entries_for_project(&conn, &hash)
            .unwrap()
            .into_iter()
            .find(|e| e.content.contains("feat/a"))
            .unwrap();
        assert!((before.confidence - 0.85).abs() < 1e-6, "starts at 0.85");

        let r_resolved = rev(
            "feat/a",
            50.0,
            true,
            "resolved",
            &[("testing", 20.0, "no tests")],
        );
        let decayed = decay_confidence_for_resolved_reviews(&conn, &hash, &[&r_resolved]);
        assert_eq!(decayed, 1);

        let after = crate::knowledge::store::get_entries_for_project(&conn, &hash)
            .unwrap()
            .into_iter()
            .find(|e| e.content.contains("feat/a"))
            .unwrap();
        // 0.85 * 0.5 = 0.425 — halved but NOT removed.
        assert!(
            (after.confidence - 0.425).abs() < 1e-6,
            "decayed to 0.425: {after:?}"
        );
    }

    #[test]
    fn decay_leaves_global_and_unrelated_untouched() {
        // Decay matches by task_ref marker on PROJECT-LOCAL lessons only. A global
        // lesson (no task_ref) and an unrelated project lesson must keep their
        // confidence — decay never cross-fires on a bare dimension match.
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = db::init_db(&tmp.path().join("t.db")).unwrap();
        let hash = activity::hash_project_path("/proj");
        let r1 = rev(
            "feat/a",
            50.0,
            true,
            "pending",
            &[("testing", 20.0, "no tests")],
        );
        let r2 = rev("feat/b", 50.0, true, "pending", &[("tooling", 20.0, "x")]);
        replay_to_knowledge(&conn, "/proj", &[&r1, &r2], &AgentType::ClaudeCode);

        // Resolve ONLY feat/a.
        let r1_resolved = rev(
            "feat/a",
            50.0,
            true,
            "resolved",
            &[("testing", 20.0, "no tests")],
        );
        let decayed = decay_confidence_for_resolved_reviews(&conn, &hash, &[&r1_resolved]);
        assert_eq!(decayed, 1, "only feat/a's lesson decays");

        // feat/b's project lesson untouched (still 0.85).
        let b = crate::knowledge::store::get_entries_for_project(&conn, &hash)
            .unwrap()
            .into_iter()
            .find(|e| e.content.contains("feat/b"))
            .unwrap();
        assert!(
            (b.confidence - 0.85).abs() < 1e-6,
            "unrelated lesson keeps 0.85: {b:?}"
        );

        // Global lessons are NEVER decayed (cross-project aggregate, source_type
        // forge_experience_global is excluded by decay's filter).
        let globals =
            crate::knowledge::store::get_entries_for_project(&conn, GLOBAL_PROJECT_HASH).unwrap();
        assert!(
            globals.iter().all(|e| (e.confidence - 0.7).abs() < 1e-6),
            "global confidence unchanged: {globals:?}"
        );
    }

    #[test]
    fn decay_is_noop_with_empty_resolved_list() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conn = db::init_db(&tmp.path().join("t.db")).unwrap();
        let hash = activity::hash_project_path("/proj");
        let r = rev(
            "feat/a",
            50.0,
            true,
            "pending",
            &[("testing", 20.0, "no tests")],
        );
        replay_to_knowledge(&conn, "/proj", &[&r], &AgentType::ClaudeCode);
        assert_eq!(decay_confidence_for_resolved_reviews(&conn, &hash, &[]), 0);
    }
}
