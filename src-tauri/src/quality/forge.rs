use crate::error::AppError;
use crate::models::{QualityCheck, QualityReport};
use std::path::Path;
use std::process::Command;

/// Discover if Forge CLI is installed and available.
pub fn discover_forge() -> Option<std::path::PathBuf> {
    which::which("forge").ok().or_else(|| {
        // Check common local install paths
        let home = crate::commands::projects::dirs_home();
        let local_bin = home.join(".local").join("bin").join("forge");
        if local_bin.exists() {
            Some(local_bin)
        } else {
            None
        }
    })
}

/// Run Forge quality gate for a project and return the report.
pub fn run_forge_gate(project_path: &Path) -> Result<QualityReport, AppError> {
    let forge_bin = discover_forge().ok_or(AppError::ForgeNotInstalled)?;

    let mut cmd = Command::new(&forge_bin);
    cmd.arg("gate")
        .arg("--current")
        .current_dir(project_path);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = cmd.output()
        .map_err(|e| AppError::Agent(format!("Forge gate 执行失败: {}", e)))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() && stdout.trim().is_empty() {
        // Forge failed with no JSON output — create a basic report from stderr
        return Ok(QualityReport {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: String::new(),
            checks: vec![QualityCheck {
                name: "forge_gate".to_string(),
                status: "failed".to_string(),
                message: Some(stderr.trim().to_string()),
            }],
            overall_status: "failed".to_string(),
            created_at: chrono::Local::now().to_rfc3339(),
        });
    }

    parse_forge_output(&stdout)
}

/// Parse Forge JSON output into a QualityReport.
fn parse_forge_output(json_str: &str) -> Result<QualityReport, AppError> {
    let parsed: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| AppError::Agent(format!("Forge 输出解析失败: {}", e)))?;

    let mut checks = Vec::new();

    // Try to extract checks from various Forge output formats
    if let Some(checks_arr) = parsed.get("checks").and_then(|c| c.as_array()) {
        for check in checks_arr {
            checks.push(QualityCheck {
                name: check.get("name").and_then(|n| n.as_str()).unwrap_or("unknown").to_string(),
                status: check.get("status").and_then(|s| s.as_str()).unwrap_or("unknown").to_string(),
                message: check.get("message").and_then(|m| m.as_str()).map(|s| s.to_string()),
            });
        }
    } else if let Some(results) = parsed.get("results").and_then(|r| r.as_array()) {
        for result in results {
            checks.push(QualityCheck {
                name: result.get("check").and_then(|c| c.as_str()).unwrap_or("unknown").to_string(),
                status: result.get("passed").and_then(|p| p.as_bool()).map(|b| if b { "passed" } else { "failed" }).unwrap_or("unknown").to_string(),
                message: result.get("message").and_then(|m| m.as_str()).map(|s| s.to_string()),
            });
        }
    } else {
        // Single-status output
        let status = parsed.get("status").and_then(|s| s.as_str()).unwrap_or("unknown");
        checks.push(QualityCheck {
            name: "forge_gate".to_string(),
            status: status.to_string(),
            message: parsed.get("message").and_then(|m| m.as_str()).map(|s| s.to_string()),
        });
    }

    let overall = if checks.iter().all(|c| c.status == "passed") {
        "passed"
    } else if checks.iter().any(|c| c.status == "failed") {
        "failed"
    } else {
        "unknown"
    };

    Ok(QualityReport {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: String::new(),
        checks,
        overall_status: overall.to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_forge_output_checks_format() {
        let json = r#"{"checks": [{"name": "compile", "status": "passed"}, {"name": "test", "status": "failed", "message": "2 tests failed"}]}"#;
        let report = parse_forge_output(json).unwrap();
        assert_eq!(report.checks.len(), 2);
        assert_eq!(report.overall_status, "failed");
        assert_eq!(report.checks[0].status, "passed");
        assert_eq!(report.checks[1].status, "failed");
    }

    #[test]
    fn test_parse_forge_output_results_format() {
        let json = r#"{"results": [{"check": "lint", "passed": true}, {"check": "test", "passed": false, "message": "failure"}]}"#;
        let report = parse_forge_output(json).unwrap();
        assert_eq!(report.checks.len(), 2);
        assert_eq!(report.overall_status, "failed");
    }

    #[test]
    fn test_parse_forge_output_single_status() {
        let json = r#"{"status": "passed", "message": "All checks passed"}"#;
        let report = parse_forge_output(json).unwrap();
        assert_eq!(report.checks.len(), 1);
        assert_eq!(report.overall_status, "passed");
    }

    #[test]
    fn test_parse_forge_all_passed() {
        let json = r#"{"checks": [{"name": "compile", "status": "passed"}, {"name": "test", "status": "passed"}]}"#;
        let report = parse_forge_output(json).unwrap();
        assert_eq!(report.overall_status, "passed");
    }
}
