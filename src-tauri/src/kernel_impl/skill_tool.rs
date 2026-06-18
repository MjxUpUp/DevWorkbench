//! Skill-tool wrapper — exposes one installed Skill (SKILL.md) as a `Tool`.
//!
//! Anthropic Agent Skills semantics, with progressive disclosure:
//!   Tier 1 — name + description live in ToolInfo (always visible to the model,
//!            it matches the "Use when:" trigger text to decide invocation).
//!   Tier 2 — invoking the tool returns the SKILL.md body (the procedure).
//!   Tier 3 — the same invoke appends a manifest of the skill's bundled
//!            resources (references/, scripts/, assets/) with absolute paths,
//!            so the agent reads/runs them ON DEMAND via read_file/bash rather
//!            than having them all pre-loaded into context.
//! A skill directory thus carries not just SKILL.md but a small tree of support
//! files; this module surfaces that tree to the agent so a multi-file skill
//! (e.g. body that says "加载 references/x.md") actually has those references
//! reachable instead of dangling.

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
    /// Source SKILL.md path — provenance, and the read_file target when the
    /// body is truncated past the invoke char cap.
    path: PathBuf,
    /// SKILL.md's parent dir — the root a skill's relative resource paths
    /// (`references/x.md`, `scripts/run.sh`, ...) resolve against.
    base_dir: PathBuf,
}

/// Hard cap on how much of the SKILL.md body one invoke returns. Beyond this
/// the agent is pointed at the source file to read the rest. GLM-4.6's 128k
/// window tolerates ~32k chars (≈8–16k tokens) for a single skill read.
const SKILL_BODY_MAX_CHARS: usize = 32_000;

/// Cap on the number of resource lines emitted in one invoke, so a pathological
/// assets/ tree can't blow out the response. Extra entries collapse to one
/// `... (N more)` line.
const SKILL_MAX_RESOURCES: usize = 64;

/// Files larger than this are still listed but tagged `(large)` so the agent
/// thinks twice before read_file-ing them.
const SKILL_LARGE_FILE_BYTES: u64 = 256 * 1024;

impl SkillTool {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        body: impl Into<String>,
        path: PathBuf,
        base_dir: PathBuf,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            body: body.into(),
            path,
            base_dir,
        }
    }

    /// Parse a single SKILL.md file into a SkillTool.
    pub fn parse_file(path: &Path) -> Result<Self, Error> {
        let raw = std::fs::read_to_string(path).map_err(|e| Error::Tool(format!("read {}: {e}", path.display())))?;
        Self::parse_text(&raw, path.to_path_buf())
    }

    /// Parse SKILL.md text (split out for testing).
    /// `parse_text` derives `base_dir` from `path.parent()` itself, so callers
    /// (including tests) keep the two-argument signature.
    pub fn parse_text(raw: &str, path: PathBuf) -> Result<Self, Error> {
        let base_dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| path.clone());
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
        Ok(Self::new(name, description, body, path, base_dir))
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
        // Progressive disclosure Tier 2 + Tier 3 in one response: the body
        // (possibly truncated to SKILL_BODY_MAX_CHARS) followed by a manifest
        // of bundled resources with absolute paths. The agent then reads/runs
        // those on demand via read_file/bash — it never gets the whole skill
        // tree dumped up front, which is the whole point of progressive
        // disclosure.
        let body_out = truncate_body(&self.body, SKILL_BODY_MAX_CHARS, &self.path);
        let resources = scan_skill_resources(&self.base_dir);
        let appendix = format_resources_appendix(&resources);
        Ok(format!("{body_out}{appendix}"))
    }

    fn is_read_only(&self) -> bool {
        true // reading a skill never mutates state
    }
}

/// Return the skill body, truncating past `max_chars` with a pointer back to
/// the source file so the agent can read_file the full text if it needs the tail.
fn truncate_body(body: &str, max_chars: usize, source: &Path) -> String {
    let count = body.chars().count();
    if count <= max_chars {
        return body.to_string();
    }
    let bounded: String = body.chars().take(max_chars).collect();
    let remaining = count - max_chars;
    format!(
        "{bounded}\n\n...(skill body truncated, {remaining} more chars; read_file \"{}\" 取全文)\n",
        source.display()
    )
}

/// A bundled skill resource (one file under references/ scripts/ or assets/).
#[derive(Debug, Clone)]
struct SkillResource {
    /// Path relative to the skill base dir, forward-slash separated for display.
    rel: String,
    /// Native absolute path (Windows backslashes) — the value the agent passes
    /// to read_file/bash.
    abs: PathBuf,
    kind: SkillResKind,
    /// File exceeds SKILL_LARGE_FILE_BYTES — surfaced as a `(large)` tag.
    large: bool,
}

/// The three conventional resource subdirs a skill may bundle. Declaration
/// order fixes the manifest section order (Reference → Script → Asset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SkillResKind {
    Reference,
    Script,
    Asset,
}

impl SkillResKind {
    const ALL: [SkillResKind; 3] = [
        SkillResKind::Reference,
        SkillResKind::Script,
        SkillResKind::Asset,
    ];

    /// The conventional subdir name under a skill's base dir.
    fn dir(self) -> &'static str {
        match self {
            SkillResKind::Reference => "references",
            SkillResKind::Script => "scripts",
            SkillResKind::Asset => "assets",
        }
    }

    /// (section heading, usage hint) — kept together so they never drift.
    fn section(self) -> (&'static str, &'static str) {
        match self {
            SkillResKind::Reference => ("References", "(read_file 读绝对路径)"),
            SkillResKind::Script => ("Scripts", "(bash 执行)"),
            SkillResKind::Asset => ("Assets", "(二进制,勿 read_file,按路径引用)"),
        }
    }
}

/// Scan ONLY the three conventional resource subdirs under `base_dir`, each
/// recursively. Sibling files/dirs (e.g. a SKILL.md living at a repo root next
/// to src/) are deliberately ignored so unrelated source never leaks into the
/// manifest. Errors are logged and skipped — scanning never panics.
fn scan_skill_resources(base_dir: &Path) -> Vec<SkillResource> {
    let mut out = Vec::new();
    for kind in SkillResKind::ALL {
        let subdir = base_dir.join(kind.dir());
        if let Err(e) = collect_resources(&subdir, kind.dir(), kind, &mut out) {
            log::warn!("scan skill {} in {}: {e}", kind.dir(), base_dir.display());
        }
    }
    // Stable order: by kind (Reference < Script < Asset), then relative path.
    out.sort_by(|a, b| (a.kind, a.rel.as_str()).cmp(&(b.kind, b.rel.as_str())));
    out
}

/// Recursively collect resource files under `dir`, prefixing each entry's rel
/// path with `rel_prefix` (the conventional subdir name, or `<subdir>/<nest>`
/// on recursion). Missing dir is a no-op (most skills have 0–1 resource kinds).
fn collect_resources(
    dir: &Path,
    rel_prefix: &str,
    kind: SkillResKind,
    out: &mut Vec<SkillResource>,
) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else { continue };
        // Skip dotfiles (VCS / editor cruft) — convention hygiene.
        if name.starts_with('.') {
            continue;
        }
        let p = entry.path();
        let rel = if rel_prefix.is_empty() {
            name.to_string()
        } else {
            format!("{rel_prefix}/{name}")
        };
        let meta = match std::fs::metadata(&p) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("stat skill resource {}: {e}", p.display());
                continue;
            }
        };
        if meta.is_dir() {
            collect_resources(&p, &rel, kind, out)?;
        } else {
            // references/ is text-only by convention — drop anything that looks
            // binary (NUL byte in the first 8KB) so a stray image doesn't end
            // up as a read_file target. scripts/assets may be binary, so they
            // skip the sniff and are listed as-is.
            if kind == SkillResKind::Reference && looks_binary(&p) {
                log::warn!("skip binary reference {}: not text", p.display());
                continue;
            }
            out.push(SkillResource {
                rel,
                abs: p,
                kind,
                large: meta.len() > SKILL_LARGE_FILE_BYTES,
            });
        }
    }
    Ok(())
}

/// Heuristic binary sniff: a NUL byte in the first 8KB means binary. An IO
/// error is treated as "not binary" so a transient failure never silently drops
/// a reference.
fn looks_binary(path: &Path) -> bool {
    use std::io::Read;
    let mut buf = [0u8; 8192];
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let n = f.read(&mut buf).unwrap_or(0);
    buf[..n].contains(&0)
}

/// Render the resource manifest appendix, or empty string when there are no
/// resources (keeps single-file skills' invoke output clean — no `---` fence).
fn format_resources_appendix(resources: &[SkillResource]) -> String {
    if resources.is_empty() {
        return String::new();
    }
    let mut s = String::new();
    s.push_str("\n\n---\n\n## Skill resources(按需加载,勿预读全部)\n");

    // Walk kinds in declaration order so sections are stable; emit entries
    // until the global SKILL_MAX_RESOURCES cap, then fold the rest into one
    // "... (N more)" line so a huge assets/ tree can't dominate the response.
    let mut emitted = 0usize;
    for kind in SkillResKind::ALL {
        let group: Vec<&SkillResource> = resources.iter().filter(|r| r.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        let (heading, hint) = kind.section();
        s.push_str(&format!("\n{heading} {hint}:\n"));
        for r in &group {
            if emitted >= SKILL_MAX_RESOURCES {
                break;
            }
            let large_tag = if r.large { " (large)" } else { "" };
            s.push_str(&format!("- {}{large_tag}  ->  {}\n", r.rel, r.abs.display()));
            emitted += 1;
        }
    }
    let more = resources.len().saturating_sub(emitted);
    if more > 0 {
        s.push_str(&format!("- ... ({more} more)\n"));
    }

    // How-to line: relative path in the body ↔ absolute path in the manifest.
    // Plain push_str (not format!) so the literal braces survive unescaped.
    s.push_str("\n调用:read_file {file_path:\"<绝对路径>\"};脚本:bash {command:\"bash \\\"<绝对路径>\\\"\"}。\n");
    s.push_str("body 内 references/x.md 即清单中对应绝对路径。\n");
    s
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
    use tempfile::tempdir;

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
        // Single-file skill (path has no resource subdirs beside it): invoke
        // returns the body with NO appendix — the manifest is omitted when
        // there's nothing to list.
        let t = SkillTool::parse_text(SAMPLE_SKILL_MD, PathBuf::from("/x/SKILL.md")).unwrap();
        let out = futures::executor::block_on(t.invoke("", &ToolContext::default())).unwrap();
        assert!(out.contains("Delegate execution"));
        assert!(!out.contains("Skill resources"));
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

    // ---- progressive-disclosure (Tier 3) helpers + invoke ----

    /// Write a text file under `dir` (creating parent dirs as needed).
    fn write_file(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
    }

    /// Write raw bytes (for binary fixtures like a PNG asset).
    fn write_file_bytes(dir: &Path, rel: &str, content: &[u8]) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
    }

    /// Core: a multi-file skill's invoke returns the body AND lists its
    /// references with absolute paths the agent can hand to read_file.
    #[test]
    fn invoke_returns_body_and_resources_for_multifile_skill() {
        let dir = tempdir().unwrap();
        write_file(
            dir.path(),
            "SKILL.md",
            "---\nname: crg\ndescription: gate\n---\n\n# Code Review Gate\n加载 [references/a.md] 和 [references/b.md]\n",
        );
        write_file(dir.path(), "references/a.md", "A");
        write_file(dir.path(), "references/b.md", "B");

        let t = SkillTool::parse_file(&dir.path().join("SKILL.md")).unwrap();
        let out = futures::executor::block_on(t.invoke("", &ToolContext::default())).unwrap();

        assert!(out.contains("Code Review Gate"));
        assert!(out.contains("References"));
        assert!(out.contains("references/a.md"));
        assert!(out.contains("references/b.md"));
        // absolute paths are emitted so the agent can copy them into read_file
        let a_abs = dir.path().join("references").join("a.md");
        assert!(out.contains(&a_abs.display().to_string()));
        // usage instruction is present
        assert!(out.contains("read_file"));
    }

    #[test]
    fn invoke_omits_appendix_for_singlefile_skill() {
        let dir = tempdir().unwrap();
        write_file(
            dir.path(),
            "SKILL.md",
            "---\nname: solo\ndescription: d\n---\n\n# Just body\nNo resources.\n",
        );
        let t = SkillTool::parse_file(&dir.path().join("SKILL.md")).unwrap();
        let out = futures::executor::block_on(t.invoke("", &ToolContext::default())).unwrap();

        // Body only — no `---` fence, no "Skill resources" heading.
        assert!(out.contains("Just body"));
        assert!(!out.contains("---"));
        assert!(!out.contains("Skill resources"));
    }

    #[test]
    fn invoke_lists_scripts_section() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "SKILL.md", "---\nname: s\ndescription: d\n---\n\n# Body\n");
        write_file(dir.path(), "scripts/run.sh", "#!/bin/bash\necho hi");

        let t = SkillTool::parse_file(&dir.path().join("SKILL.md")).unwrap();
        let out = futures::executor::block_on(t.invoke("", &ToolContext::default())).unwrap();

        assert!(out.contains("Scripts"));
        assert!(out.contains("scripts/run.sh"));
        assert!(out.contains("bash"));
    }

    #[test]
    fn invoke_lists_assets_section() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "SKILL.md", "---\nname: s\ndescription: d\n---\n\n# Body\n");
        // Binary bytes incl. a NUL — assets may be binary (no sniff on assets/).
        write_file_bytes(dir.path(), "assets/icon.png", &[0x89, 0x50, 0x4E, 0x47, 0x00, 0x0D]);

        let t = SkillTool::parse_file(&dir.path().join("SKILL.md")).unwrap();
        let out = futures::executor::block_on(t.invoke("", &ToolContext::default())).unwrap();

        assert!(out.contains("Assets"));
        assert!(out.contains("assets/icon.png"));
        assert!(out.contains("勿 read_file"));
    }

    #[test]
    fn invoke_resource_paths_absolute_and_native() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "SKILL.md", "---\nname: s\ndescription: d\n---\n\n# Body\n");
        write_file(dir.path(), "references/x.md", "X");

        let t = SkillTool::parse_file(&dir.path().join("SKILL.md")).unwrap();
        let out = futures::executor::block_on(t.invoke("", &ToolContext::default())).unwrap();

        let x_abs = dir.path().join("references").join("x.md");
        assert!(out.contains(&x_abs.display().to_string()));
    }

    #[test]
    fn invoke_truncates_long_body_and_points_to_source() {
        let dir = tempdir().unwrap();
        let skill_md = dir.path().join("SKILL.md");
        let body = "A".repeat(40_000);
        let raw = format!("---\nname: big\ndescription: d\n---\n\n{body}");
        std::fs::write(&skill_md, raw).unwrap();

        let t = SkillTool::parse_file(&skill_md).unwrap();
        let out = futures::executor::block_on(t.invoke("", &ToolContext::default())).unwrap();

        assert!(out.contains("truncated"));
        // notice points back at the source SKILL.md for read_file
        assert!(out.contains(&skill_md.display().to_string()));
        // The body portion (before the truncation notice) is exactly the cap of
        // A's. Splitting at the notice marker isolates the truncated body from
        // the notice+path — the tempdir path may itself contain 'A' (e.g.
        // C:\Users\Administrator\AppData), so counting A's over the whole `out`
        // would be flaky. If truncation never fired, no marker exists and
        // body_part is the full 40000 A's → assertion fails.
        let body_part = out.split("...(skill body truncated").next().unwrap();
        assert_eq!(body_part.matches('A').count(), SKILL_BODY_MAX_CHARS);
    }

    #[test]
    fn scan_ignores_sibling_dirs() {
        // A SKILL.md at a repo root sits next to unrelated source — the scanner
        // must only touch references/scripts/assets, never src/.
        let dir = tempdir().unwrap();
        write_file(dir.path(), "SKILL.md", "---\nname: s\ndescription: d\n---\n\n# Body\n");
        write_file(dir.path(), "src/main.rs", "fn main(){}");
        write_file(dir.path(), "references/x.md", "X");

        let t = SkillTool::parse_file(&dir.path().join("SKILL.md")).unwrap();
        let out = futures::executor::block_on(t.invoke("", &ToolContext::default())).unwrap();

        assert!(out.contains("references/x.md"));
        assert!(!out.contains("src/main.rs"));
        assert!(!out.contains("main.rs"));
    }

    #[test]
    fn scan_recurses_into_nested_resource_dirs() {
        let dir = tempdir().unwrap();
        write_file(dir.path(), "SKILL.md", "---\nname: s\ndescription: d\n---\n\n# Body\n");
        write_file(dir.path(), "references/sub/deep.md", "deep");

        let t = SkillTool::parse_file(&dir.path().join("SKILL.md")).unwrap();
        let out = futures::executor::block_on(t.invoke("", &ToolContext::default())).unwrap();

        assert!(out.contains("references/sub/deep.md"));
    }

    #[test]
    fn base_dir_set_to_skill_md_parent() {
        let dir = tempdir().unwrap();
        let skill_md = dir.path().join("SKILL.md");
        std::fs::write(&skill_md, "---\nname: s\ndescription: d\n---\n\n# Body\n").unwrap();

        let t = SkillTool::parse_file(&skill_md).unwrap();

        assert_eq!(t.base_dir, dir.path());
    }

    #[test]
    fn invoke_handles_empty_references_dir() {
        // An empty references/ dir must not crash and must not emit a section.
        let dir = tempdir().unwrap();
        write_file(dir.path(), "SKILL.md", "---\nname: s\ndescription: d\n---\n\n# Body\n");
        std::fs::create_dir_all(dir.path().join("references")).unwrap();

        let t = SkillTool::parse_file(&dir.path().join("SKILL.md")).unwrap();
        let out = futures::executor::block_on(t.invoke("", &ToolContext::default())).unwrap();

        assert!(!out.contains("Skill resources"));
        assert!(!out.contains("References"));
        assert!(out.contains("Body"));
    }

    #[test]
    fn scan_drops_binary_references_but_keeps_text() {
        // references/ is text-only: a NUL-byte file is skipped, a sibling text
        // file is kept. (Assets have no such sniff — see invoke_lists_assets_section.)
        let dir = tempdir().unwrap();
        write_file(dir.path(), "SKILL.md", "---\nname: s\ndescription: d\n---\n\n# Body\n");
        write_file_bytes(dir.path(), "references/img.png", &[0x89, 0x50, 0x00, 0x0D]);
        write_file(dir.path(), "references/notes.md", "text");

        let t = SkillTool::parse_file(&dir.path().join("SKILL.md")).unwrap();
        let out = futures::executor::block_on(t.invoke("", &ToolContext::default())).unwrap();

        assert!(out.contains("references/notes.md"));
        assert!(!out.contains("img.png"));
    }

    #[test]
    fn resource_list_caps_at_max_with_more_indicator() {
        // 70 references exceeds SKILL_MAX_RESOURCES (64) → 64 lines + "(6 more)".
        let dir = tempdir().unwrap();
        write_file(dir.path(), "SKILL.md", "---\nname: s\ndescription: d\n---\n\n# Body\n");
        for i in 0..70 {
            write_file(dir.path(), &format!("references/r{i:02}.md"), "x");
        }

        let t = SkillTool::parse_file(&dir.path().join("SKILL.md")).unwrap();
        let out = futures::executor::block_on(t.invoke("", &ToolContext::default())).unwrap();

        // 70 - 64 = 6 more
        assert!(out.contains("(6 more)"));
        // the cap applied: r00..r63 present, r64.. absent (sorted alpha, r64 > r63)
        assert!(out.contains("references/r63.md"));
        assert!(!out.contains("references/r64.md"));
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

    /// Live progressive-disclosure smoke against a real multi-file skill in the
    /// global catalog (e.g. code-review-gate with its references/ tree). Ignored
    /// by default — run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "touches the real ~/.agents/skills catalog; run with --ignored"]
    fn invoke_real_multifile_skill_lists_references() {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
        let Some(home) = home else { return };
        let catalog = std::path::PathBuf::from(home).join(".agents").join("skills");
        let skills = SkillTool::load_dir(&catalog);
        let Some(crg) = skills.into_iter().find(|s| s.name == "code-review-gate") else {
            eprintln!("no code-review-gate skill installed; nothing to assert");
            return;
        };
        let out = futures::executor::block_on(crg.invoke("", &ToolContext::default())).unwrap();
        assert!(out.contains("References"), "multi-file skill must list its references: {out}");
        assert!(out.contains("references/"), "expected reference paths in the manifest: {out}");
    }
}
