//! HarnessCenter CLI wrapper — calls `hc scan/search/install` commands.

use std::process::Command;

use crate::error::AppError;

/// Wrapper around the HarnessCenter (`hc`) CLI.
pub struct HarnessCli;

impl HarnessCli {
    /// Run `hc scan` and return parsed JSON output.
    pub fn scan() -> Result<serde_json::Value, AppError> {
        Self::run(&["scan", "--output", "json"])
    }

    /// Run `hc search <query>` and return parsed JSON output.
    pub fn search(query: &str) -> Result<serde_json::Value, AppError> {
        Self::run(&["search", query, "--output", "json"])
    }

    /// Run `hc install <org>/<name>` and return parsed JSON output.
    pub fn install(skill_ref: &str) -> Result<serde_json::Value, AppError> {
        Self::run(&["install", skill_ref, "--output", "json"])
    }

    fn run(args: &[&str]) -> Result<serde_json::Value, AppError> {
        let output = Command::new("hc")
            .args(args)
            .output()
            .map_err(|e| AppError::Skill(format!("Failed to run hc: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::Skill(format!("hc command failed: {}", stderr)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str(&stdout).map_err(|e| AppError::Skill(format!("Failed to parse hc output: {}", e)))
    }
}
