use crate::activity;
use crate::models::QualityReport;
use rusqlite::Connection;

/// Feed a quality-report result back as an activity event so the run shows up
/// in the project's activity feed. (The knowledge/learning side was removed with
/// the memory system.)
pub fn create_feedback(
    conn: &Connection,
    report: &QualityReport,
    project_path: &str,
    agent_type: &crate::models::AgentType,
) -> Result<(), crate::error::AppError> {
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
}
