//! Local Skill registry — SQLite CRUD for the skills table.

use rusqlite::Connection;

use crate::error::AppError;
use crate::models::{Skill, SkillReport};

/// List all installed skills.
pub fn list_skills(conn: &Connection) -> Result<Vec<Skill>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, org, name, version, installed_at, path, quality_score, metadata FROM skills",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Skill {
            id: row.get(0)?,
            org: row.get(1)?,
            name: row.get(2)?,
            version: row.get(3)?,
            installed_at: row.get(4)?,
            path: row.get(5)?,
            quality_score: row.get(6)?,
            metadata: row.get(7)?,
        })
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
