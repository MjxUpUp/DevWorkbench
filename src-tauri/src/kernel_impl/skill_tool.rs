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
        // Try strict YAML first — handles well-formed frontmatter including the
        // optional `metadata:` block. But third-party SKILL.md files in the
        // wild frequently break serde_yaml: `description` values embed
        // unescaped quotes (`用户问"X"时`) or bare colons (`Use when: 恢复`),
        // which serde_yaml rejects. load_dir then silently dropped the whole
        // skill (`skip skill ... parse frontmatter`), and a kernel agent built
        // against the resulting empty registry told users "I only have
        // dispatch_subagent, I can't see skills". Fall back to a line scan
        // that needs only name + description and reads their values raw
        // (stripping one optional wrapping quote pair) — a malformed
        // description then costs nothing but the metadata block.
        let (name, description) = match serde_yaml::from_str::<SkillFrontmatter>(&fm) {
            Ok(parsed) => (parsed.name, parsed.description),
            Err(_) => extract_name_desc(&fm)
                .ok_or_else(|| Error::Tool("parse frontmatter: missing name".into()))?,
        };
        Ok(Self::new(name, description, body, path))
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
pub(crate) fn split_frontmatter(raw: &str) -> (String, String) {
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

/// Best-effort name/description extraction for frontmatter serde_yaml rejects.
/// Walks lines, takes the FIRST `name:` / `description:` and treats everything
/// after the colon as a raw string (stripping one matching wrapping quote pair).
/// Returns None only when `name` is absent — description defaults to empty,
/// mirroring SkillFrontmatter's `#[serde(default)]`.
fn extract_name_desc(fm: &str) -> Option<(String, String)> {
    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    for line in fm.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name:") {
            if name.is_none() {
                name = Some(strip_wrap_quotes(rest.trim()));
            }
        } else if let Some(rest) = line.strip_prefix("description:") {
            if description.is_none() {
                description = Some(strip_wrap_quotes(rest.trim()));
            }
        }
    }
    Some((name?, description.unwrap_or_default()))
}

/// Strip ONE wrapping pair of quotes (`"` or `'`) from both ends, only when
/// the first and last byte are the SAME quote kind. Inner quotes survive —
/// that is the whole point for values like `用户问"X"时`.
fn strip_wrap_quotes(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
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

    // Regression: third-party SKILL.md commonly embeds an unescaped quote in
    // `description` (`用户问"X"时`). serde_yaml rejects it; the line-scan
    // fallback must still yield the skill so the agent's registry isn't empty.
    #[test]
    fn parses_description_with_embedded_quotes_via_fallback() {
        let raw = "---\nname: adr\ndescription: \"Use when: 用户问\"X\"时、评估方案权衡时。SKIP: 日常编码。\"\nmetadata:\n  pattern: x\n---\n\n# ADR\n";
        let t = SkillTool::parse_text(raw, PathBuf::from("/x/SKILL.md")).unwrap();
        assert_eq!(t.name, "adr");
        assert!(t.description.contains("用户问"));
        // inner quotes survive — they're the value, not YAML syntax
        assert!(t.description.contains("\"X\""));
        assert!(t.body.contains("# ADR"));
    }

    // Regression: a bare (unquoted) description that contains a colon
    // (`Use when: 恢复`) reads as a nested mapping to serde_yaml. Fallback
    // keeps the whole line as the description.
    #[test]
    fn parses_bare_description_with_colon_via_fallback() {
        let raw = "---\nname: session-continuity\ndescription: 跨会话接力。Use when: 恢复工作时、用户说\"继续\"时。SKIP: 新项目。\nmetadata:\n  pattern: y\n---\n\n# Continuity\n";
        let t = SkillTool::parse_text(raw, PathBuf::from("/x/SKILL.md")).unwrap();
        assert_eq!(t.name, "session-continuity");
        assert!(t.description.starts_with("跨会话接力"));
        assert!(t.description.contains("Use when: 恢复"));
    }

    #[test]
    fn rejects_frontmatter_without_name() {
        // Neither serde_yaml (missing required `name`) nor the line scan finds
        // a name → hard error, not a silently empty skill.
        let raw = "---\ndescription: nothing useful here\n---\n\nbody\n";
        assert!(SkillTool::parse_text(raw, PathBuf::from("/x/SKILL.md")).is_err());
    }

    /// Live check against the real `~/.agents/skills`: the three skills that
    /// serde_yaml used to skip (malformed frontmatter) must now load. Ignored by
    /// default — it touches a machine-specific catalog — run with
    /// `cargo test -- --ignored` to confirm the fix end-to-end on this box.
    #[test]
    #[ignore = "touches the real ~/.agents/skills catalog; run with --ignored"]
    fn loads_real_global_skills_without_skipping() {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
        let Some(home) = home else { return };
        let dir = std::path::PathBuf::from(home).join(".agents").join("skills");
        if !dir.is_dir() {
            eprintln!("no ~/.agents/skills on this machine; nothing to assert");
            return;
        }
        let skills = SkillTool::load_dir(&dir);
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        eprintln!("loaded real skills ({})", names.len());
        for expected in [
            "architecture-decision-record",
            "evidence-based-proposal",
            "session-continuity",
        ] {
            assert!(
                names.iter().any(|n| *n == expected),
                "{expected} still missing — frontmatter fallback did not recover it. Loaded: {names:?}"
            );
        }
    }
}
