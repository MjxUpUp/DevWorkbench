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
    let conn = db.get()
        .map_err(|e| crate::error::AppError::Config(format!("Lock error: {}", e)))?;
    crate::skills::registry::list_skills(&conn)
}

#[tauri::command]
pub async fn uninstall_skill(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db.get()
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
    let conn = db.get()
        .map_err(|e| AppError::Config(format!("Lock error: {e}")))?;
    // Idempotent: the catalog lists skills already on disk, so re-clicking
    // "安装" on an already-registered skill must NOT create a duplicate row.
    // (Each install used to mint a fresh uuid id, so INSERT OR IGNORE by-id
    // never deduped — parallel rows piled up.) Resolve by the natural key
    // (org, name) the user sees; return the existing record instead.
    if let Some(existing) = crate::skills::registry::find_by_org_name(&conn, "local", leaf)? {
        return Ok(existing);
    }
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

    // B4: static security scan at install time. Runs over the skill's on-disk
    // content (SKILL.md body + scripts/ + references/) and assigns a
    // security_score + findings the catalog surfaces before the user trusts the
    // skill. Persisted into the `metadata` JSON column so `list_skills` →
    // enrich_from_metadata surfaces it without re-scanning.
    let base_dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(&source));
    let scan = crate::skills::scanner::scan_skill(&base_dir);
    log::info!(
        "skill_scanner: '{leaf}' scored {:.0} ({} findings, has_block={})",
        scan.security_score,
        scan.findings.len(),
        scan.has_block(),
    );
    let security_details = if scan.findings.is_empty() {
        None
    } else {
        Some(scan.details_text())
    };
    let metadata = Some(serde_json::json!({
        "securityScore": scan.security_score,
        "securityDetails": security_details,
    })
    .to_string());

    let skill = Skill {
        id: uuid::Uuid::new_v4().to_string(),
        org: "local".into(),
        name: leaf.into(),
        version: None,
        installed_at: Some(chrono::Utc::now().to_rfc3339()),
        path: Some(path.display().to_string()),
        quality_score: None,
        metadata,
        description: Some(tool.info().description),
        icon: None,
        category: None,
        security_score: Some(scan.security_score),
        installs: None,
        rating: None,
        author: None,
        compatible_agents: None,
        quality_details: None,
        security_details: security_details,
        config_schema: None,
    };
    crate::skills::registry::install_skill(&conn, &skill)?;
    Ok(skill)
}

/// Resolve the user home directory (mirrors commands::projects::dirs_home but
/// without pulling that module's other deps here).
fn dirs_home_opt() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}
