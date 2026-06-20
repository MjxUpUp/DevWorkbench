//! Local Skill registry — SQLite CRUD for the skills table.

use rusqlite::Connection;
use serde::Deserialize;

use crate::error::AppError;
use crate::models::Skill;

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

/// Look up a skill by its natural key (org, name) — the dedup key the user
/// actually sees. install_skill_from_catalog uses this to make re-install
/// idempotent: install_skill writes `INSERT OR IGNORE` keyed by the uuid id,
/// and each install minted a fresh uuid, so the IGNORE never fired and parallel
/// rows for the same skill piled up. Resolving by (org, name) before inserting
/// returns the existing record instead of duplicating.
pub fn find_by_org_name(conn: &Connection, org: &str, name: &str) -> Result<Option<Skill>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, org, name, version, installed_at, path, quality_score, metadata FROM skills WHERE org = ?1 AND name = ?2",
    )?;
    let mut rows = stmt.query_map(rusqlite::params![org, name], |row| {
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
    match rows.next() {
        Some(s) => Ok(Some(s?)),
        None => Ok(None),
    }
}

/// Remove a skill by ID.
pub fn uninstall_skill(conn: &Connection, id: &str) -> Result<(), AppError> {
    conn.execute("DELETE FROM skills WHERE id = ?1", [id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Connection {
        // Mirror the `skills` DDL from db.rs (8 columns) so find_by_org_name /
    // install_skill run against the real shape without spinning up migrations.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE skills (
                id TEXT PRIMARY KEY,
                org TEXT NOT NULL,
                name TEXT NOT NULL,
                version TEXT,
                installed_at TEXT,
                path TEXT,
                quality_score REAL,
                metadata TEXT
            )",
            [],
        )
        .unwrap();
        conn
    }

    fn mk_skill(id: &str, name: &str) -> Skill {
        Skill {
            id: id.into(),
            org: "local".into(),
            name: name.into(),
            version: None,
            installed_at: None,
            path: Some(format!("/skills/{name}")),
            quality_score: None,
            metadata: None,
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
        }
    }

    #[test]
    fn find_by_org_name_returns_none_when_table_empty() {
        let conn = mem_db();
        assert!(find_by_org_name(&conn, "local", "missing").unwrap().is_none());
    }

    #[test]
    fn find_by_org_name_returns_the_registered_skill() {
        let conn = mem_db();
        install_skill(&conn, &mk_skill("s1", "my-skill")).unwrap();
        let found = find_by_org_name(&conn, "local", "my-skill").unwrap();
        let s = found.expect("expected the just-installed skill");
        assert_eq!(s.id, "s1");
        assert_eq!(s.name, "my-skill");
        assert_eq!(s.path.as_deref(), Some("/skills/my-skill"));
    }

    #[test]
    fn find_by_org_name_matches_on_org_and_name_together() {
        // Same name under different orgs must NOT cross-match — (org, name) is
        // the natural key install_skill_from_catalog dedups on, so a match must
        // pin both columns.
        let conn = mem_db();
        let mut a = mk_skill("a1", "shared");
        a.org = "team-a".into();
        let mut b = mk_skill("b1", "shared");
        b.org = "team-b".into();
        install_skill(&conn, &a).unwrap();
        install_skill(&conn, &b).unwrap();
        assert_eq!(find_by_org_name(&conn, "team-a", "shared").unwrap().unwrap().id, "a1");
        assert_eq!(find_by_org_name(&conn, "team-b", "shared").unwrap().unwrap().id, "b1");
    }

    #[test]
    fn security_score_metadata_round_trips_through_enrich() {
        // B4 contract: install_skill_from_catalog writes the scanner result into
        // the `metadata` JSON column (camelCase securityScore/securityDetails),
        // and list_skills / find_by_org_name must surface it back via
        // enrich_from_metadata. This pins the key-shape contract between the
        // writer (skills_cmds.rs) and the reader (here) so a rename on either
        // side is caught by a test, not by a silent 0-score in the catalog.
        let conn = mem_db();
        let mut skill = mk_skill("sec1", "risky");
        skill.metadata = Some(
            serde_json::json!({
                "securityScore": 50.0_f64,
                "securityDetails": "[BLOCK] SKILL.md:3 (rm_rf_system): blocked",
            })
            .to_string(),
        );
        install_skill(&conn, &skill).unwrap();
        let found = find_by_org_name(&conn, "local", "risky").unwrap().expect("skill gone");
        assert_eq!(found.security_score, Some(50.0));
        assert_eq!(
            found.security_details.as_deref(),
            Some("[BLOCK] SKILL.md:3 (rm_rf_system): blocked"),
        );
    }

    #[test]
    fn security_score_defaults_none_when_metadata_absent() {
        // A skill installed before B4 (or one the scanner ran clean on, with
        // null details) must surface security_score via the metadata key, not
        // crash on a missing/null value.
        let conn = mem_db();
        let mut skill = mk_skill("sec2", "clean");
        skill.metadata = Some(serde_json::json!({ "securityScore": 100.0_f64 }).to_string());
        install_skill(&conn, &skill).unwrap();
        let found = find_by_org_name(&conn, "local", "clean").unwrap().expect("skill gone");
        assert_eq!(found.security_score, Some(100.0));
        assert!(found.security_details.is_none());
    }
}
