//! Skills management commands — CRUD + local catalog + market ops.

use std::path::PathBuf;

use kernel_core::Tool;
use tauri::State;

use crate::db::DbState;
use crate::error::AppError;
use crate::kernel_impl::skill_tool::SkillTool;
use crate::models::Skill;

#[tauri::command]
pub async fn list_skills(db: State<'_, DbState>) -> Result<Vec<Skill>, AppError> {
    let conn = db
        .0
        .lock()
        .map_err(|e| crate::error::AppError::Config(format!("Lock error: {}", e)))?;
    crate::skills::registry::list_skills(&conn)
}

#[tauri::command]
pub async fn install_skill(db: State<'_, DbState>, skill: Skill) -> Result<(), AppError> {
    let conn = db
        .0
        .lock()
        .map_err(|e| crate::error::AppError::Config(format!("Lock error: {}", e)))?;
    crate::skills::registry::install_skill(&conn, &skill)
}

#[tauri::command]
pub async fn uninstall_skill(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db
        .0
        .lock()
        .map_err(|e| crate::error::AppError::Config(format!("Lock error: {}", e)))?;
    crate::skills::registry::uninstall_skill(&conn, &id)
}

/// One entry in the local skill catalog (an installed Skill on disk).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillCatalogEntry {
    pub name: String,
    pub description: String,
    /// Where this skill lives on disk.
    pub source: String,
    /// "global" (under ~/.agents/skills) or "project" (under .agents/skills).
    pub scope: String,
}

/// Scan all skill directories (global ~/.agents/skills + project-local
/// .agents/skills) and return the discoverable catalog. Each SKILL.md found is
/// one entry. This is the Skills Market's "browse" source of truth.
#[tauri::command]
pub async fn skill_catalog(project_path: Option<String>) -> Result<Vec<SkillCatalogEntry>, AppError> {
    let mut entries = Vec::new();
    // Global skills: ~/.agents/skills
    if let Some(home) = dirs_home_opt() {
        let global_dir = home.join(".agents").join("skills");
        for t in SkillTool::load_dir(&global_dir) {
            entries.push(SkillCatalogEntry {
                name: t.info().name,
                description: t.info().description,
                source: global_dir.display().to_string(),
                scope: "global".into(),
            });
        }
    }
    // Project-local skills: <project>/.agents/skills
    if let Some(pp) = project_path {
        let proj_dir = PathBuf::from(&pp).join(".agents").join("skills");
        if proj_dir.is_dir() {
            for t in SkillTool::load_dir(&proj_dir) {
                entries.push(SkillCatalogEntry {
                    name: t.info().name,
                    description: t.info().description,
                    source: proj_dir.display().to_string(),
                    scope: "project".into(),
                });
            }
        }
    }
    Ok(entries)
}

/// Install a skill by recording it in the skills table from a catalog entry.
/// (The skill files already exist on disk; this registers metadata + a score.)
#[tauri::command]
pub async fn install_skill_from_catalog(
    db: State<'_, DbState>,
    name: String,
    source: String,
) -> Result<Skill, AppError> {
    // Find the SKILL.md under <source>/<leaf-name>/SKILL.md or <source>/SKILL.md.
    let leaf = name.strip_prefix("skill__").unwrap_or(&name);
    let candidates = [
        PathBuf::from(&source).join(leaf).join("SKILL.md"),
        PathBuf::from(&source).join("SKILL.md"),
    ];
    let path = candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| AppError::Skill(format!("SKILL.md not found under {source} for {name}")))?;
    let tool = SkillTool::parse_file(&path)
        .map_err(|e| AppError::Skill(format!("parse skill: {e}")))?;
    let skill = Skill {
        id: uuid::Uuid::new_v4().to_string(),
        org: "local".into(),
        name: leaf.into(),
        version: None,
        installed_at: Some(chrono::Utc::now().to_rfc3339()),
        path: Some(path.display().to_string()),
        quality_score: None,
        metadata: None,
        description: Some(tool.info().description),
        icon: None,
        category: None,
        security_score: None,
        installs: None,
        rating: None,
        author: None,
        compatible_agents: None,
        quality_details: None,
        security_details: None,
        config_schema: None,
    };
    let conn = db
        .0
        .lock()
        .map_err(|e| AppError::Config(format!("Lock error: {e}")))?;
    crate::skills::registry::install_skill(&conn, &skill)?;
    Ok(skill)
}

/// Rate a skill (persist into its metadata). Simple 0..=5 rating for the market.
#[tauri::command]
pub async fn rate_skill(
    db: State<'_, DbState>,
    skill_id: String,
    rating: f64,
) -> Result<(), AppError> {
    if !(0.0..=5.0).contains(&rating) {
        return Err(AppError::Skill("rating must be 0..=5".into()));
    }
    let conn = db
        .0
        .lock()
        .map_err(|e| AppError::Config(format!("Lock error: {e}")))?;
    // Load existing metadata, merge rating, write back.
    let existing: Option<String> = conn
        .query_row(
            "SELECT metadata FROM skills WHERE id = ?1",
            rusqlite::params![&skill_id],
            |r| r.get(0),
        )
        .ok();
    let mut meta: serde_json::Value = existing
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    meta["rating"] = serde_json::json!(rating);
    let meta_str = serde_json::to_string(&meta)
        .map_err(|e| AppError::Skill(format!("serialize metadata: {e}")))?;
    conn.execute(
        "UPDATE skills SET metadata = ?1 WHERE id = ?2",
        rusqlite::params![meta_str, &skill_id],
    )?;
    Ok(())
}

/// Resolve the user home directory (mirrors commands::projects::dirs_home but
/// without pulling that module's other deps here).
fn dirs_home_opt() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}
