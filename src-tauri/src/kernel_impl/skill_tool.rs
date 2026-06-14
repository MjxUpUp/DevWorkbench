//! Skill-tool wrapper — exposes one installed Skill (SKILL.md) as a `Tool`.
//!
//! Anthropic Agent Skills semantics: a skill's SKILL.md frontmatter declares
//! its name + description (the "Use when:" trigger text the model matches
//! against); invoking the tool returns the skill's full body so the agent reads
//! the procedure and follows it. This makes every skill callable by a
//! transparent agent like any other tool.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use kernel_core::{Error, Tool, ToolContext, ToolInfo};
use serde::Deserialize;

/// Parsed SKILL.md frontmatter (the YAML between the `---` fences).
#[derive(Debug, Clone, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// One loaded skill, exposed as a kernel Tool.
pub struct SkillTool {
    name: String,
    description: String,
    /// The full SKILL.md body (returned on invoke, so the agent reads it).
    body: String,
    /// Source SKILL.md path (provenance for future reload/diagnostics; not
    /// read on the current invoke path).
    #[allow(dead_code)]
    path: PathBuf,
}

impl SkillTool {
    pub fn new(name: impl Into<String>, description: impl Into<String>, body: impl Into<String>, path: PathBuf) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            body: body.into(),
            path,
        }
    }

    /// Parse a single SKILL.md file into a SkillTool.
    pub fn parse_file(path: &Path) -> Result<Self, Error> {
        let raw = std::fs::read_to_string(path).map_err(|e| Error::Tool(format!("read {}: {e}", path.display())))?;
        Self::parse_text(&raw, path.to_path_buf())
    }

    /// Parse SKILL.md text (split out for testing).
    pub fn parse_text(raw: &str, path: PathBuf) -> Result<Self, Error> {
        let (fm, body) = split_frontmatter(raw);
        let fm: SkillFrontmatter = serde_yaml::from_str(&fm)
            .map_err(|e| Error::Tool(format!("parse frontmatter: {e}")))?;
        Ok(Self::new(fm.name, fm.description, body, path))
    }

    /// Load all skills from a directory tree (recursively finds SKILL.md).
    pub fn load_dir(dir: &Path) -> Vec<Self> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let skill_md = p.join("SKILL.md");
                if skill_md.is_file() {
                    match Self::parse_file(&skill_md) {
                        Ok(t) => out.push(t),
                        Err(e) => log::warn!("skip skill {}: {e}", p.display()),
                    }
                } else {
                    // Recurse one level (skill packs may nest).
                    out.extend(Self::load_dir(&p));
                }
            }
        }
        out
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: format!("skill__{}", self.name),
            // Keep the "Use when:" trigger text in the description so the model
            // can decide when to invoke this skill.
            description: self.description.chars().take(1024).collect(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "description": "this skill takes no arguments; invoke it to read its procedure"
            }),
        }
    }

    async fn invoke(&self, _arguments: &str, _ctx: &ToolContext) -> Result<String, Error> {
        // Return the full skill body — the agent reads it and follows it.
        // Truncate to a sane bound so a huge skill doesn't blow the context.
        let bounded: String = self.body.chars().take(16_000).collect();
        Ok(bounded)
    }

    fn is_read_only(&self) -> bool {
        true // reading a skill never mutates state
    }
}

/// Split a SKILL.md into (frontmatter_yaml, body_markdown).
/// Returns ("", full_text) if no frontmatter fence is present.
fn split_frontmatter(raw: &str) -> (String, String) {
    let raw = raw.trim_start_matches('\u{feff}');
    let rest = match raw.strip_prefix("---\n") {
        Some(r) => r,
        None => match raw.strip_prefix("---\r\n") {
            Some(r) => r,
            None => return (String::new(), raw.to_string()),
        },
    };
    if let Some(end) = rest.find("\n---") {
        let fm = rest[..end].to_string();
        let body = rest[end + 4..].trim_start_matches(['\n', '\r']).to_string();
        (fm, body)
    } else {
        (String::new(), raw.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const SAMPLE_SKILL_MD: &str = "---\nname: agent-delegation\ndescription: \"Use when: dispatching tasks. SKIP: pure chat.\"\nmetadata:\n  pattern: reviewer\n---\n\n# Delegation Protocol\n\nDelegate execution, not understanding.\n";

    #[test]
    fn parses_skill_frontmatter_and_body() {
        let t = SkillTool::parse_text(SAMPLE_SKILL_MD, PathBuf::from("/x/SKILL.md")).unwrap();
        assert_eq!(t.name, "agent-delegation");
        assert!(t.description.contains("dispatching tasks"));
        assert!(t.body.contains("Delegation Protocol"));
    }

    #[test]
    fn info_name_prefixed_and_description_keeps_trigger() {
        let t = SkillTool::parse_text(SAMPLE_SKILL_MD, PathBuf::from("/x/SKILL.md")).unwrap();
        let info = t.info();
        assert_eq!(info.name, "skill__agent-delegation");
        assert!(info.description.contains("Use when"));
    }

    #[test]
    fn invoke_returns_body() {
        let t = SkillTool::parse_text(SAMPLE_SKILL_MD, PathBuf::from("/x/SKILL.md")).unwrap();
        let out = futures::executor::block_on(t.invoke("", &ToolContext::default())).unwrap();
        assert!(out.contains("Delegate execution"));
    }

    #[test]
    fn split_frontmatter_without_fence_returns_empty_fm() {
        let (fm, body) = split_frontmatter("just markdown, no front matter");
        assert!(fm.is_empty());
        assert_eq!(body, "just markdown, no front matter");
    }

    #[test]
    fn split_handles_crlf_fence() {
        let raw = "---\r\nname: x\r\ndescription: y\r\n---\r\nbody";
        let (fm, body) = split_frontmatter(raw);
        assert!(fm.contains("name: x"));
        assert!(body.contains("body"));
    }
}
