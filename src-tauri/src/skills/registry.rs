//! Local Skill registry — SQLite CRUD for the skills table.

use rusqlite::Connection;
use serde::Deserialize;

use crate::error::AppError;
use crate::models::{Skill, SkillReport};

/// Helper: parse metadata JSON and populate catalog fields on a Skill.
fn enrich_from_metadata(skill: &mut Skill) {
    let meta_str = match &skill.metadata {
        Some(s) if !s.is_empty() => s,
        _ => return,
    };

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SkillMeta {
        description: Option<String>,
        icon: Option<String>,
        category: Option<String>,
        security_score: Option<f64>,
        installs: Option<i64>,
        rating: Option<f64>,
        author: Option<String>,
        compatible_agents: Option<String>,
        quality_details: Option<String>,
        security_details: Option<String>,
        config_schema: Option<String>,
    }

    if let Ok(meta) = serde_json::from_str::<SkillMeta>(meta_str) {
        skill.description = meta.description;
        skill.icon = meta.icon;
        skill.category = meta.category;
        skill.security_score = meta.security_score;
        skill.installs = meta.installs;
        skill.rating = meta.rating;
        skill.author = meta.author;
        skill.compatible_agents = meta.compatible_agents;
        skill.quality_details = meta.quality_details;
        skill.security_details = meta.security_details;
        skill.config_schema = meta.config_schema;
    }
}

/// List all installed skills.
pub fn list_skills(conn: &Connection) -> Result<Vec<Skill>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, org, name, version, installed_at, path, quality_score, metadata FROM skills",
    )?;
    let rows = stmt.query_map([], |row| {
        let mut skill = Skill {
            id: row.get(0)?,
            org: row.get(1)?,
            name: row.get(2)?,
            version: row.get(3)?,
            installed_at: row.get(4)?,
            path: row.get(5)?,
            quality_score: row.get(6)?,
            metadata: row.get(7)?,
            description: None,
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
        enrich_from_metadata(&mut skill);
        Ok(skill)
    })?;
    let mut skills = Vec::new();
    for skill in rows {
        skills.push(skill?);
    }
    Ok(skills)
}

/// Insert a new skill record.
pub fn install_skill(conn: &Connection, skill: &Skill) -> Result<(), AppError> {
    conn.execute(
        "INSERT OR IGNORE INTO skills (id, org, name, version, installed_at, path, quality_score, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            skill.id,
            skill.org,
            skill.name,
            skill.version,
            skill.installed_at,
            skill.path,
            skill.quality_score,
            skill.metadata,
        ],
    )?;
    Ok(())
}

/// Remove a skill by ID.
pub fn uninstall_skill(conn: &Connection, id: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM skills WHERE id = ?1", [id])?;
    Ok(())
}

/// Insert a skill scan report.
pub fn add_report(conn: &Connection, report: &SkillReport) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO skill_reports (id, skill_id, scan_result, scanned_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![report.id, report.skill_id, report.scan_result, report.scanned_at],
    )?;
    Ok(())
}
