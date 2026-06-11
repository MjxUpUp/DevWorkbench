use crate::error::AppError;
use crate::models::QualityReport;
use rusqlite::params;

/// Save a quality report to the database.
pub fn save_report(conn: &rusqlite::Connection, report: &QualityReport) -> Result<(), AppError> {
    let checks_json = serde_json::to_string(&report.checks)?;
    conn.execute(
        "INSERT INTO quality_reports (id, session_id, checks, overall_status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            report.id,
            report.session_id,
            checks_json,
            report.overall_status,
            report.created_at,
        ],
    )?;
    Ok(())
}

/// Get a quality report by session ID.
pub fn get_report_for_session(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Option<QualityReport>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, checks, overall_status, created_at
         FROM quality_reports WHERE session_id = ?1",
    )?;

    let result = stmt.query_row(params![session_id], |row| {
        let checks_str: String = row.get(2)?;
        let checks: Vec<crate::models::QualityCheck> =
            serde_json::from_str(&checks_str).unwrap_or_default();

        Ok(QualityReport {
            id: row.get(0)?,
            session_id: row.get(1)?,
            checks,
            overall_status: row.get(3)?,
            created_at: row.get(4)?,
        })
    });

    match result {
        Ok(report) => Ok(Some(report)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(AppError::from(e)),
    }
}

/// Get all quality reports.
pub fn get_all_reports(conn: &rusqlite::Connection) -> Result<Vec<QualityReport>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, checks, overall_status, created_at
         FROM quality_reports ORDER BY created_at DESC",
    )?;

    let reports = stmt.query_map([], |row| {
        let checks_str: String = row.get(2)?;
        let checks: Vec<crate::models::QualityCheck> =
            serde_json::from_str(&checks_str).unwrap_or_default();

        Ok(QualityReport {
            id: row.get(0)?,
            session_id: row.get(1)?,
            checks,
            overall_status: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;

    let mut result = Vec::new();
    for r in reports {
        result.push(r?);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::QualityCheck;

    struct TempDb {
        _tmp: tempfile::TempDir,
        conn: rusqlite::Connection,
    }

    impl TempDb {
        fn new() -> Self {
            let tmp = tempfile::TempDir::new().unwrap();
            let db_path = tmp.path().join("test.db");
            let conn = db::init_db(&db_path).expect("init_db failed");
            Self { _tmp: tmp, conn }
        }
    }

    fn make_report(id: &str, session_id: &str, status: &str) -> QualityReport {
        QualityReport {
            id: id.to_string(),
            session_id: session_id.to_string(),
            checks: vec![QualityCheck {
                name: "compile".to_string(),
                status: status.to_string(),
                message: None,
            }],
            overall_status: status.to_string(),
            created_at: chrono::Local::now().to_rfc3339(),
        }
    }

    #[test]
    fn test_save_and_get_report() {
        let db = TempDb::new();
        save_report(&db.conn, &make_report("q1", "s1", "passed")).unwrap();

        let report = get_report_for_session(&db.conn, "s1").unwrap();
        assert!(report.is_some());
        assert_eq!(report.unwrap().overall_status, "passed");
    }

    #[test]
    fn test_get_report_not_found() {
        let db = TempDb::new();
        let report = get_report_for_session(&db.conn, "nonexistent").unwrap();
        assert!(report.is_none());
    }

    #[test]
    fn test_get_all_reports() {
        let db = TempDb::new();
        save_report(&db.conn, &make_report("q1", "s1", "passed")).unwrap();
        save_report(&db.conn, &make_report("q2", "s2", "failed")).unwrap();

        let reports = get_all_reports(&db.conn).unwrap();
        assert_eq!(reports.len(), 2);
    }
}
