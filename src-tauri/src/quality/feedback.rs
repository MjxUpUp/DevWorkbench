use crate::activity;
use crate::models::QualityReport;
use rusqlite::Connection;

/// Feed quality report results back into the system:
/// 1. Create an activity event for the quality gate result
/// 2. Create knowledge entries from failure insights
pub fn create_feedback(
    conn: &Connection,
    report: &QualityReport,
    project_path: &str,
    agent_type: &crate::models::AgentType,
) -> Result<(), crate::error::AppError> {
    // 1. Activity event
    let failed_count = report.checks.iter().filter(|c| c.status == "failed").count();
    let title = if report.overall_status == "passed" {
        format!("Quality gate passed ({} checks)", report.checks.len())
    } else {
        format!("Quality gate failed ({}/{} checks failed)", failed_count, report.checks.len())
    };

    let description = report
        .checks
        .iter()
        .filter(|c| c.status == "failed")
        .filter_map(|c| c.message.as_ref().map(|m| format!("{}: {}", c.name, m)))
        .collect::<Vec<_>>()
        .join("; ");

    let _ = activity::record_event(
        conn,
        &activity::make_activity_event(
            &report.session_id,
            project_path,
            agent_type,
            "quality_gate",
            &title,
            if description.is_empty() { None } else { Some(description) },
            None,
        ),
    );

    // 2. Knowledge entry for failures
    if report.overall_status == "failed" {
        let failed_checks: Vec<&crate::models::QualityCheck> =
            report.checks.iter().filter(|c| c.status == "failed").collect();

        if !failed_checks.is_empty() {
            let content = failed_checks
                .iter()
                .map(|c| {
                    format!(
                        "- {}{}",
                        c.name,
                        c.message
                            .as_ref()
                            .map(|m| format!(": {}", m))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");

            let entry = crate::models::KnowledgeEntry {
                id: uuid::Uuid::new_v4().to_string(),
                project_hash: activity::hash_project_path(project_path),
                category: "quality_failure".to_string(),
                title: format!("Quality gate failures: {}", title),
                content,
                source_agent: agent_type.clone(),
                source_session_id: Some(report.session_id.clone()),
                source_type: "forge_gate".to_string(),
                confidence: 0.9,
                created_at: chrono::Local::now().to_rfc3339(),
                updated_at: chrono::Local::now().to_rfc3339(),
                access_count: 0,
            };

            let _ = crate::knowledge::store::add_entry(conn, &entry);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::{AgentType, QualityCheck};

    struct TempDb {
        _tmp: tempfile::TempDir,
        conn: Connection,
    }

    impl TempDb {
        fn new() -> Self {
            let tmp = tempfile::TempDir::new().unwrap();
            let db_path = tmp.path().join("test.db");
            let conn = db::init_db(&db_path).expect("init_db failed");
            Self { _tmp: tmp, conn }
        }
    }

    #[test]
    fn test_feedback_creates_activity() {
        let db = TempDb::new();
        let report = QualityReport {
            id: "qr1".to_string(),
            session_id: "s1".to_string(),
            checks: vec![QualityCheck {
                name: "compile".to_string(),
                status: "passed".to_string(),
                message: None,
            }],
            overall_status: "passed".to_string(),
            created_at: chrono::Local::now().to_rfc3339(),
        };

        create_feedback(&db.conn, &report, "/proj/a", &AgentType::ClaudeCode).unwrap();

        let events = crate::activity::get_events_for_project(&db.conn, "/proj/a").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "quality_gate");
    }

    #[test]
    fn test_feedback_creates_knowledge_on_failure() {
        let db = TempDb::new();
        let report = QualityReport {
            id: "qr2".to_string(),
            session_id: "s2".to_string(),
            checks: vec![QualityCheck {
                name: "test".to_string(),
                status: "failed".to_string(),
                message: Some("2 tests failed".to_string()),
            }],
            overall_status: "failed".to_string(),
            created_at: chrono::Local::now().to_rfc3339(),
        };

        create_feedback(&db.conn, &report, "/proj/b", &AgentType::ClaudeCode).unwrap();

        let hash = activity::hash_project_path("/proj/b");
        let entries = crate::knowledge::store::get_entries_for_project(&db.conn, &hash).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].category, "quality_failure");
    }
}
