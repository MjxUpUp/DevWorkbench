//! Named sub-agent specs loaded from `.agents/subagents/<name>/AGENT.md`
//! frontmatter. A named sub-agent overrides the dispatcher's anonymous worker
//! prompt (and optionally narrows the read-only tool subset), so the user can
//! define specialized delegates (e.g. "researcher", "test-writer") the main
//! agent delegates to BY NAME via `dispatch_subagent {subagent: "researcher"}`.
//!
//! This is the D1 "named delegation" gap: before this, SubAgentTool dispatched
//! an anonymous worker with a fixed prompt — the agent couldn't say "hand this
//! to the researcher", only "hand this to some generic child".

use std::path::Path;

use kernel_core::Error;
use serde::Deserialize;

use super::skill_tool::split_frontmatter;

/// Parsed AGENT.md frontmatter (YAML between the `---` fences). `system_prompt`
/// is typically a YAML block scalar (`|`); `tools_allow` is a list of tool-name
/// prefixes the child is restricted to (empty = inherit the full read-only
/// subset, matching the anonymous worker).
#[derive(Debug, Clone, Deserialize)]
pub struct SubAgentFrontmatter {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub tools_allow: Vec<String>,
}

/// One loaded named sub-agent — the value side of the
/// `dispatch_subagent {subagent: <name>}` lookup.
#[derive(Debug, Clone)]
pub struct SubAgentSpec {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub tools_allow: Vec<String>,
}

/// Load every named sub-agent defined under `dir` (recursive AGENT.md). Returns
/// an empty vec if the dir is absent — the dispatcher then falls back to its
/// anonymous worker, so a project with no `.agents/subagents/` is unaffected.
pub fn load_subagents(dir: &Path) -> Vec<SubAgentSpec> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            let agent_md = p.join("AGENT.md");
            if agent_md.is_file() {
                match parse_file(&agent_md) {
                    Ok(s) => out.push(s),
                    Err(e) => log::warn!("skip subagent {}: {e}", p.display()),
                }
            } else {
                out.extend(load_subagents(&p));
            }
        }
    }
    out
}

fn parse_file(path: &Path) -> Result<SubAgentSpec, Error> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| Error::Tool(format!("read {}: {e}", path.display())))?;
    let (fm, _body) = split_frontmatter(&raw);
    let parsed: SubAgentFrontmatter = serde_yaml::from_str(&fm)
        .map_err(|e| Error::Tool(format!("parse subagent frontmatter: {e}")))?;
    if parsed.name.trim().is_empty() {
        return Err(Error::Tool("subagent frontmatter missing name".into()));
    }
    Ok(SubAgentSpec {
        name: parsed.name,
        description: parsed.description,
        system_prompt: parsed.system_prompt,
        tools_allow: parsed.tools_allow,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_name_description_system_prompt_and_tools_allow() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("researcher");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("AGENT.md"),
            "---\nname: researcher\ndescription: deep web research\nsystem_prompt: |\n  你是调研专家,只给结论。\ntools_allow:\n  - skill__web_search\n  - read_file\n---\nbody ignored\n",
        ).unwrap();
        let specs = load_subagents(tmp.path());
        assert_eq!(specs.len(), 1);
        let s = &specs[0];
        assert_eq!(s.name, "researcher");
        assert_eq!(s.description, "deep web research");
        assert!(s.system_prompt.contains("调研专家"), "system_prompt: {s:?}");
        assert_eq!(s.tools_allow, vec!["skill__web_search", "read_file"]);
    }

    #[test]
    fn missing_dir_is_empty_not_error() {
        let specs = load_subagents(std::path::Path::new("/no/such/subagents/dir"));
        assert!(specs.is_empty());
    }

    #[test]
    fn empty_name_is_skipped_good_one_survives() {
        let tmp = tempfile::TempDir::new().unwrap();
        let good = tmp.path().join("good");
        let bad = tmp.path().join("bad");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(good.join("AGENT.md"), "---\nname: good\ndescription: ok\n---\n").unwrap();
        // bad: empty name → parse_file rejects → skipped (not fatal).
        std::fs::write(bad.join("AGENT.md"), "---\nname: \"\"\ndescription: empty\n---\n").unwrap();
        let specs = load_subagents(tmp.path());
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"good"), "good survives, got {names:?}");
        assert!(!names.contains(&""), "empty-name skipped, got {names:?}");
    }
}
