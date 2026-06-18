//! Sub-agent authoring commands (D1) — the file-based CRUD the
//! `SubAgentsSection` UI drives. Named sub-agents live on disk as
//! `.agents/subagents/<name>/AGENT.md` (frontmatter: name/description/
//! system_prompt/tools_allow). The kernel loads them at agent build time via
//! [`crate::kernel_impl::subagent_spec::load_subagents`], so a file written here
//! is immediately delegatable by name (`dispatch_subagent {subagent: <name>}`).
//!
//! Read reuses the kernel loader (single source of truth for parse semantics);
//! write serializes a frontmatter block serde_yaml can read back (symmetric with
//! the loader's `serde_yaml::from_str`).

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::State;

use crate::db::DbState;
use crate::error::AppError;
use crate::kernel_impl::subagent_spec::{self, SubAgentSpec};

/// One named sub-agent surfaced to the UI. `scope` is where it lives
/// ("global" under ~/.agents/subagents, "project" under <project>/.agents/subagents);
/// `source_path` is the AGENT.md path for display.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentInfo {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub tools_allow: Vec<String>,
    pub scope: String,
    pub source_path: String,
}

impl From<(SubAgentSpec, &str, PathBuf)> for SubAgentInfo {
    fn from((spec, scope, path): (SubAgentSpec, &str, PathBuf)) -> Self {
        SubAgentInfo {
            name: spec.name,
            description: spec.description,
            system_prompt: spec.system_prompt,
            tools_allow: spec.tools_allow,
            scope: scope.into(),
            source_path: path.display().to_string(),
        }
    }
}

/// The two scopes the UI can write to. "global" (~/.agents/subagents) is shared
/// across projects; "project" (<project>/.agents/subagents) is checked into the
/// repo. The legacy app-private ~/.dev-workbench/subagents fallback is read-only
/// from the UI (load_subagents already covers it for the agent; surfacing it as
/// a write target would surprise users who expect project-level versioning).
fn scope_dir(scope: &str, project_path: Option<&str>, home: &Path) -> Result<PathBuf, AppError> {
    match scope {
        "global" => Ok(home.join(".agents").join("subagents")),
        "project" => project_path
            .map(|p| PathBuf::from(p).join(".agents").join("subagents"))
            .ok_or_else(|| AppError::Config("project scope 需要一个打开的项目".into())),
        other => Err(AppError::Config(format!("未知 scope: {other}"))),
    }
}

/// Validate a sub-agent name is a safe single path segment (slug). Prevents
/// `../` traversal and odd chars in the `<name>/AGENT.md` path. Mirrors the
/// identifiers users naturally pick ("researcher", "test-writer").
fn validate_name(name: &str) -> Result<(), AppError> {
    if name.is_empty() {
        return Err(AppError::Config("子智能体名不能为空".into()));
    }
    if name == "." || name == ".." {
        return Err(AppError::Config("子智能体名不能是 . 或 ..".into()));
    }
    let ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        return Err(AppError::Config(
            "子智能体名只能含字母、数字、-、_（避免路径问题）".into(),
        ));
    }
    Ok(())
}

/// List every named sub-agent the kernel would load for this project: global +
/// project + the app-private legacy fallback. Earlier scopes shadow later ones
/// (matching load order), so dedupe by name keeping the first (highest-priority)
/// occurrence — the UI must show the agent the kernel will actually dispatch to,
/// not every copy on disk.
#[tauri::command]
pub async fn list_subagents(
    _db: State<'_, DbState>,
    project_path: Option<String>,
) -> Result<Vec<SubAgentInfo>, AppError> {
    let home = crate::commands::projects::dirs_home();
    let data_dir = home.join(".dev-workbench");
    let dirs: [(String, PathBuf); 3] = [
        ("global".into(), home.join(".agents").join("subagents")),
        (
            "project".into(),
            PathBuf::from(project_path.as_deref().unwrap_or(""))
                .join(".agents")
                .join("subagents"),
        ),
        ("app-private".into(), data_dir.join("subagents")),
    ];

    // Earlier scopes shadow later ones (matching load order in executor), so
    // dedupe by name keeping the FIRST occurrence — the UI must show the agent
    // the kernel will actually dispatch to, not every copy on disk.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (scope, dir) in &dirs {
        if !dir.is_dir() {
            continue;
        }
        for spec in subagent_spec::load_subagents(dir) {
            if seen.insert(spec.name.clone()) {
                // The loader scanned <dir>/<name>/AGENT.md, so that's the source.
                let agent_md = dir.join(&spec.name).join("AGENT.md");
                out.push(SubAgentInfo::from((spec, scope.as_str(), agent_md)));
            }
        }
    }
    Ok(out)
}

/// Write (create or overwrite) a sub-agent's AGENT.md. Returns the resulting
/// info (re-loaded so the caller sees exactly what the kernel will parse).
#[tauri::command]
pub async fn save_subagent(
    _db: State<'_, DbState>,
    project_path: Option<String>,
    name: String,
    description: String,
    system_prompt: String,
    tools_allow: Vec<String>,
    scope: String,
) -> Result<SubAgentInfo, AppError> {
    let name = name.trim().to_string();
    validate_name(&name)?;
    let home = crate::commands::projects::dirs_home();
    let base = scope_dir(&scope, project_path.as_deref(), &home)?;
    let agent_dir = base.join(&name);
    let agent_md = agent_dir.join("AGENT.md");
    std::fs::create_dir_all(&agent_dir)
        .map_err(|e| AppError::Config(format!("创建目录失败: {e}")))?;
    let content = serialize_agent_md(&name, &description, &system_prompt, &tools_allow);
    std::fs::write(&agent_md, content)
        .map_err(|e| AppError::Config(format!("写入 AGENT.md 失败: {e}")))?;
    // Re-load via the kernel loader so the returned info matches what the agent
    // will actually see (round-trip proof: write then parse).
    let loaded = subagent_spec::load_subagents(&base)
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| AppError::Config("写入后重新加载失败".into()))?;
    Ok(SubAgentInfo::from((loaded, scope.as_str(), agent_md)))
}

/// Delete a sub-agent by removing its `<name>/` directory. Refuses to delete
/// outside the resolved scope base (defense-in-depth on top of name validation).
#[tauri::command]
pub async fn delete_subagent(
    _db: State<'_, DbState>,
    project_path: Option<String>,
    name: String,
    scope: String,
) -> Result<(), AppError> {
    let name = name.trim().to_string();
    validate_name(&name)?;
    let home = crate::commands::projects::dirs_home();
    let base = scope_dir(&scope, project_path.as_deref(), &home)?;
    let agent_dir = base.join(&name);
    // The dir must be INSIDE base (canonicalize both, then strip_prefix). This
    // catches any residual traversal even though validate_name already blocks it.
    let canon_base = std::fs::canonicalize(&base).unwrap_or_else(|_| base.clone());
    let canon_target = std::fs::canonicalize(&agent_dir)
        .map_err(|_| AppError::Config(format!("子智能体 {name} 不存在")))?;
    if !canon_target.starts_with(&canon_base) {
        return Err(AppError::Config(format!(
            "拒绝删除 base 之外的目录: {}",
            canon_target.display()
        )));
    }
    std::fs::remove_dir_all(&agent_dir)
        .map_err(|e| AppError::Config(format!("删除失败: {e}")))?;
    Ok(())
}

/// Serialize a sub-agent to AGENT.md frontmatter the kernel loader can parse.
/// Uses serde_yaml for the value block (symmetric with the loader's
/// `serde_yaml::from_str`), wrapped in `---` fences per [`split_frontmatter`]'s
/// contract (`---\n<yaml>\n---\n<body>`).
fn serialize_agent_md(
    name: &str,
    description: &str,
    system_prompt: &str,
    tools_allow: &[String],
) -> String {
    #[derive(Serialize)]
    struct Fm<'a> {
        name: &'a str,
        description: &'a str,
        system_prompt: &'a str,
        tools_allow: &'a [String],
    }
    let fm = Fm {
        name,
        description,
        system_prompt,
        tools_allow,
    };
    let mut yaml = serde_yaml::to_string(&fm).unwrap_or_else(|_| "name: unnamed\n".into());
    // serde_yaml 0.9 may emit a leading document marker `---\n`; strip it so we
    // control the fences (split_frontmatter keys off `---\n` at the start).
    if let Some(stripped) = yaml.strip_prefix("---\n") {
        yaml = stripped.to_string();
    }
    format!("---\n{yaml}---\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_accepts_slugs() {
        assert!(validate_name("researcher").is_ok());
        assert!(validate_name("test-writer_2").is_ok());
    }

    #[test]
    fn validate_name_rejects_traversal_and_empty() {
        assert!(validate_name("").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name(".").is_err());
        assert!(validate_name("../etc").is_err());
        assert!(validate_name("a/b").is_err(), "slash blocked");
        assert!(validate_name("a b").is_err(), "space blocked");
    }

    #[test]
    fn serialize_round_trips_through_kernel_loader() {
        // Write via our serializer, read via the kernel's load_subagents — the
        // round trip is the contract: anything we save must parse back exactly.
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join(".agents").join("subagents");
        let agent_dir = base.join("researcher");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let content = serialize_agent_md(
            "researcher",
            "deep web research",
            "你是调研专家\n只给结论，不要过程。",
            &["skill__web_search".into(), "read_file".into()],
        );
        std::fs::write(agent_dir.join("AGENT.md"), content).unwrap();
        let loaded = subagent_spec::load_subagents(&base);
        assert_eq!(loaded.len(), 1);
        let s = &loaded[0];
        assert_eq!(s.name, "researcher");
        assert_eq!(s.description, "deep web research");
        assert!(s.system_prompt.contains("调研专家"));
        assert!(s.system_prompt.contains("只给结论"));
        assert_eq!(s.tools_allow, vec!["skill__web_search", "read_file"]);
    }

    #[test]
    fn serialize_empty_tools_allow_round_trips() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join(".agents").join("subagents");
        let agent_dir = base.join("noop");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let content = serialize_agent_md("noop", "", "do nothing", &[]);
        std::fs::write(agent_dir.join("AGENT.md"), content).unwrap();
        let s = subagent_spec::load_subagents(&base)
            .into_iter()
            .find(|x| x.name == "noop")
            .expect("parses back");
        assert!(s.tools_allow.is_empty());
    }
}
