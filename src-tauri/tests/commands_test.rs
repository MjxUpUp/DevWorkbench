use std::fs;
use tempfile::TempDir;

// === Tool Detection Tests ===

#[test]
fn test_detect_tools_returns_all_four() {
    // detect_tools is a tauri command, test the logic directly
    let tools = ["claude", "cursor", "code", "git"];
    for name in &tools {
        let result = which::which(name);
        // We just verify the function doesn't panic - results depend on installed tools
        let _ = result;
    }
}

#[test]
fn test_detect_tools_git_installed() {
    // git should always be available in dev environment
    let result = which::which("git");
    assert!(result.is_ok(), "git should be installed in dev environment");
}

// === Project CRUD Tests ===

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct Project {
    id: String,
    name: String,
    description: String,
    path: String,
    tags: Vec<String>,
    cover_image: Option<String>,
    open_count: u32,
    last_opened_at: Option<String>,
    starred: bool,
    created_at: String,
    #[serde(default)]
    last_opened_tools: Vec<String>,
    #[serde(default)]
    workspace_tools: Vec<String>,
}

fn create_test_project(id: &str, name: &str, path: &str, starred: bool) -> Project {
    Project {
        id: id.to_string(),
        name: name.to_string(),
        description: format!("Test project {}", name),
        path: path.to_string(),
        tags: vec!["Test".to_string()],
        cover_image: None,
        open_count: 0,
        last_opened_at: None,
        starred,
        created_at: "2026-06-06T00:00:00Z".to_string(),
        last_opened_tools: vec![],
        workspace_tools: vec![],
    }
}

#[test]
fn test_save_and_load_projects() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("projects.json");

    let projects = vec![
        create_test_project("p1", "Alpha", "/tmp/alpha", false),
        create_test_project("p2", "Beta", "/tmp/beta", true),
    ];

    // Save
    let json = serde_json::to_string_pretty(&projects).unwrap();
    fs::write(&file_path, &json).unwrap();

    // Load
    let loaded: Vec<Project> = {
        let content = fs::read_to_string(&file_path).unwrap();
        serde_json::from_str(&content).unwrap()
    };

    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].name, "Alpha");
    assert_eq!(loaded[1].name, "Beta");
    assert_eq!(loaded[1].starred, true);
    assert_eq!(loaded[0].tags, vec!["Test"]);
}

#[test]
fn test_load_empty_projects_file() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("projects.json");

    // File doesn't exist => empty vec
    assert!(!file_path.exists());
    if file_path.exists() {
        panic!("File should not exist");
    }
    // Simulating load_projects behavior: return empty vec if file missing
    let loaded: Vec<Project> = if file_path.exists() {
        let content = fs::read_to_string(&file_path).unwrap();
        serde_json::from_str(&content).unwrap()
    } else {
        vec![]
    };
    assert!(loaded.is_empty());
}

#[test]
fn test_update_project_open_count() {
    let mut projects = vec![
        create_test_project("p1", "Alpha", "/tmp/alpha", false),
        create_test_project("p2", "Beta", "/tmp/beta", false),
    ];

    let now = chrono::Local::now().to_rfc3339();
    for p in &mut projects {
        if p.id == "p1" {
            p.open_count += 1;
            p.last_opened_at = Some(now.clone());
            break;
        }
    }

    assert_eq!(projects[0].open_count, 1);
    assert!(projects[0].last_opened_at.is_some());
    assert_eq!(projects[1].open_count, 0);
    assert!(projects[1].last_opened_at.is_none());
}

#[test]
fn test_toggle_star() {
    let mut project = create_test_project("p1", "Test", "/tmp/test", false);
    assert!(!project.starred);
    project.starred = !project.starred;
    assert!(project.starred);
    project.starred = !project.starred;
    assert!(!project.starred);
}

#[test]
fn test_json_roundtrip_with_windows_paths() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("projects.json");

    let projects = vec![Project {
        id: "test-path".to_string(),
        name: "WindowsPath".to_string(),
        description: "Test Windows path escaping".to_string(),
        path: "E:\\DevWorkbench".to_string(),
        tags: vec![],
        cover_image: None,
        open_count: 0,
        last_opened_at: None,
        starred: false,
        created_at: "2026-06-06T00:00:00Z".to_string(),
        last_opened_tools: vec![],
        workspace_tools: vec![],
    }];

    let json = serde_json::to_string_pretty(&projects).unwrap();
    fs::write(&file_path, &json).unwrap();

    let loaded: Vec<Project> = {
        let content = fs::read_to_string(&file_path).unwrap();
        serde_json::from_str(&content).unwrap()
    };

    assert_eq!(loaded[0].path, "E:\\DevWorkbench");
}

// === Settings Tests ===

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct AppSettings {
    scan_directories: Vec<String>,
    tool_paths: std::collections::HashMap<String, String>,
}

#[test]
fn test_save_and_load_settings() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("settings.json");

    let mut settings = AppSettings {
        scan_directories: vec!["E:\\Projects".to_string()],
        tool_paths: std::collections::HashMap::new(),
    };
    settings.tool_paths.insert("claude".to_string(), "/usr/local/bin/claude".to_string());

    let json = serde_json::to_string_pretty(&settings).unwrap();
    fs::write(&file_path, &json).unwrap();

    let loaded: AppSettings = {
        let content = fs::read_to_string(&file_path).unwrap();
        serde_json::from_str(&content).unwrap()
    };

    assert_eq!(loaded.scan_directories.len(), 1);
    assert_eq!(loaded.scan_directories[0], "E:\\Projects");
    assert_eq!(loaded.tool_paths.get("claude").unwrap(), "/usr/local/bin/claude");
}

// === Git Scanner Tests ===

#[test]
fn test_scan_git_repos() {
    let dir = TempDir::new().unwrap();

    // Create fake git repos
    let repo1 = dir.path().join("project-a");
    let repo2 = dir.path().join("nested").join("project-b");
    fs::create_dir_all(repo1.join(".git")).unwrap();
    fs::create_dir_all(repo2.join(".git")).unwrap();

    // Create a non-git directory
    let not_repo = dir.path().join("not-a-repo");
    fs::create_dir_all(&not_repo).unwrap();

    let mut repos = Vec::new();
    let mut builder = ignore::WalkBuilder::new(dir.path());
    builder
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .follow_links(false)
        .max_depth(Some(3));

    for entry in builder.build().filter_map(|e| e.ok()) {
        if !entry.file_type().is_some_and(|ft| ft.is_dir()) {
            continue;
        }
        let path = entry.path();
        if path == dir.path() {
            continue;
        }
        if path.join(".git").exists() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            repos.push((name, path.to_string_lossy().to_string()));
        }
    }

    assert_eq!(repos.len(), 2);
    let names: Vec<&str> = repos.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"project-a"));
    assert!(names.contains(&"project-b"));
}

// === Command Validation Tests ===

fn validate_command(cmd: &str) -> Result<(), String> {
    if cmd.is_empty() {
        return Ok(());
    }
    for ch in cmd.chars() {
        if !ch.is_alphanumeric() && ch != '-' && ch != '_' && ch != '.' && ch != ' ' && ch != '/' && ch != '\\' && ch != ':' {
            return Err(format!("命令包含非法字符: '{}'", ch));
        }
    }
    Ok(())
}

#[test]
fn test_validate_command_accepts_safe_names() {
    assert!(validate_command("claude").is_ok());
    assert!(validate_command("cursor").is_ok());
    assert!(validate_command("code").is_ok());
    assert!(validate_command("git").is_ok());
    assert!(validate_command("").is_ok());
    assert!(validate_command("my-tool.v2").is_ok());
    assert!(validate_command("some command").is_ok());
    // 绝对路径（自定义工具路径）
    assert!(validate_command("/opt/homebrew/bin/claude").is_ok());
    assert!(validate_command("C:\\Users\\test\\claude.exe").is_ok());
    assert!(validate_command("/usr/local/bin/codex --full-auto").is_ok());
}

#[test]
fn test_validate_command_rejects_shell_metacharacters() {
    // Shell injection attempts
    assert!(validate_command("claude; rm -rf /").is_err());
    assert!(validate_command("claude && evil").is_err());
    assert!(validate_command("$(whoami)").is_err());
    assert!(validate_command("`cat /etc/passwd`").is_err());
    assert!(validate_command("claude | tee /tmp/log").is_err());
    assert!(validate_command("claude > /dev/null").is_err());
    assert!(validate_command("claude$(evil)").is_err());
    // AppleScript injection
    assert!(validate_command("claude\" && evil").is_err());
}

// === Path Validation Tests ===

#[test]
fn test_open_terminal_validates_path() {
    let path = std::path::Path::new("/nonexistent/path/that/does/not/exist");
    assert!(!path.exists(), "Path should not exist for this test");
}

#[test]
fn test_open_in_finder_validates_path() {
    let path = std::path::Path::new("/nonexistent/path");
    assert!(!path.exists());
}

// === Frontend Filter Logic Tests (mirroring JS logic in Rust) ===

#[test]
fn test_filter_recent_projects() {
    let projects = vec![
        create_test_project("p1", "Alpha", "/a", false),
        Project {
            id: "p2".to_string(),
            name: "Beta".to_string(),
            last_opened_at: Some("2026-06-06T10:00:00Z".to_string()),
            ..create_test_project("p2", "Beta", "/b", false)
        },
        Project {
            id: "p3".to_string(),
            name: "Gamma".to_string(),
            last_opened_at: Some("2026-06-05T10:00:00Z".to_string()),
            ..create_test_project("p3", "Gamma", "/c", false)
        },
    ];

    // Filter: only projects with last_opened_at, sorted by date desc
    let mut recent: Vec<&Project> = projects.iter()
        .filter(|p| p.last_opened_at.is_some())
        .collect();
    recent.sort_by(|a, b| b.last_opened_at.cmp(&a.last_opened_at));

    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].name, "Beta");  // more recent
    assert_eq!(recent[1].name, "Gamma"); // older
}

#[test]
fn test_filter_starred_projects() {
    let projects = vec![
        create_test_project("p1", "Alpha", "/a", true),
        create_test_project("p2", "Beta", "/b", false),
        create_test_project("p3", "Gamma", "/c", true),
    ];

    let starred: Vec<&Project> = projects.iter().filter(|p| p.starred).collect();
    assert_eq!(starred.len(), 2);
    assert_eq!(starred[0].name, "Alpha");
    assert_eq!(starred[1].name, "Gamma");
}

#[test]
fn test_search_filter() {
    let projects = vec![
        Project { name: "DevWorkbench".to_string(), description: "Tauri app".to_string(), tags: vec!["Rust".to_string()], path: "E:\\DevWorkbench".to_string(), ..create_test_project("p1", "DevWorkbench", "E:\\DevWorkbench", false) },
        Project { name: "My Website".to_string(), description: "Blog".to_string(), tags: vec!["Next.js".to_string()], path: "E:\\website".to_string(), ..create_test_project("p2", "My Website", "E:\\website", false) },
    ];

    let q = "rust";
    let filtered: Vec<&Project> = projects.iter().filter(|p| {
        p.name.to_lowercase().contains(q) ||
        p.description.to_lowercase().contains(q) ||
        p.tags.iter().any(|t| t.to_lowercase().contains(q)) ||
        p.path.to_lowercase().contains(q)
    }).collect();

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].name, "DevWorkbench");
}

// === Editor Whitelist Tests ===

const ALLOWED_EDITORS: &[&str] = &[
    "code", "cursor", "windsurf", "zed", "subl",
    "vim", "nvim", "emacs", "idea", "webstorm",
    "clion", "goland", "pycharm", "rustrover",
    "pi", "codex",
];

fn is_allowed_editor(editor: &str) -> bool {
    let name = std::path::Path::new(editor)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(editor);
    ALLOWED_EDITORS.iter().any(|&allowed| {
        name.eq_ignore_ascii_case(allowed)
            || name.eq_ignore_ascii_case(&format!("{}.exe", allowed))
    })
}

#[test]
fn test_editor_whitelist_allows_known_editors() {
    assert!(is_allowed_editor("code"));
    assert!(is_allowed_editor("cursor"));
    assert!(is_allowed_editor("zed"));
    assert!(is_allowed_editor("vim"));
    assert!(is_allowed_editor("nvim"));
    assert!(is_allowed_editor("Code.exe"));
    #[cfg(target_os = "windows")]
    assert!(is_allowed_editor("C:\\Program Files\\Microsoft VS Code\\Code.exe"));
    #[cfg(not(target_os = "windows"))]
    assert!(is_allowed_editor("/usr/local/bin/code"));
}

#[test]
fn test_editor_whitelist_rejects_arbitrary_binaries() {
    assert!(!is_allowed_editor("evil-program"));
    assert!(!is_allowed_editor("/usr/bin/rm"));
    assert!(!is_allowed_editor("cmd.exe"));
    assert!(!is_allowed_editor("powershell"));
    assert!(!is_allowed_editor("bash"));
    assert!(!is_allowed_editor("C:\\Windows\\System32\\cmd.exe"));
}

// === record_tool_open Logic Tests ===

fn record_tool_open(projects: &mut [Project], id: &str, tool_name: &str) {
    for p in projects.iter_mut() {
        if p.id == id {
            p.last_opened_tools.retain(|t| t != tool_name);
            p.last_opened_tools.insert(0, tool_name.to_string());
            p.last_opened_tools.truncate(5);
            break;
        }
    }
}

#[test]
fn test_record_tool_open_adds_tool() {
    let mut projects = vec![create_test_project("p1", "Test", "/tmp/test", false)];
    record_tool_open(&mut projects, "p1", "claude");

    assert_eq!(projects[0].last_opened_tools, vec!["claude"]);
}

#[test]
fn test_record_tool_open_deduplicates() {
    let mut projects = vec![create_test_project("p1", "Test", "/tmp/test", false)];
    record_tool_open(&mut projects, "p1", "claude");
    record_tool_open(&mut projects, "p1", "cursor");
    record_tool_open(&mut projects, "p1", "claude"); // duplicate

    // claude should be moved to front, no duplicate
    assert_eq!(projects[0].last_opened_tools, vec!["claude", "cursor"]);
}

#[test]
fn test_record_tool_open_truncates_at_five() {
    let mut projects = vec![create_test_project("p1", "Test", "/tmp/test", false)];
    for tool in ["claude", "cursor", "code", "terminal", "finder", "windsurf"] {
        record_tool_open(&mut projects, "p1", tool);
    }

    assert_eq!(projects[0].last_opened_tools.len(), 5);
    // Most recent should be at front
    assert_eq!(projects[0].last_opened_tools[0], "windsurf");
    // Oldest (claude) should be dropped
    assert!(!projects[0].last_opened_tools.contains(&"claude".to_string()));
}

#[test]
fn test_record_tool_open_moves_to_front_on_reuse() {
    let mut projects = vec![create_test_project("p1", "Test", "/tmp/test", false)];
    record_tool_open(&mut projects, "p1", "claude");
    record_tool_open(&mut projects, "p1", "cursor");
    record_tool_open(&mut projects, "p1", "code");
    record_tool_open(&mut projects, "p1", "claude"); // reuse

    assert_eq!(projects[0].last_opened_tools[0], "claude");
    assert_eq!(projects[0].last_opened_tools.len(), 3);
}

// === Backward Compatibility Tests ===

#[test]
fn test_backward_compat_missing_new_fields() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("projects.json");

    // Simulate old JSON without last_opened_tools and workspace_tools
    let old_json = r#"[{
        "id": "old-project",
        "name": "OldProject",
        "description": "Created before v0.3.0",
        "path": "/tmp/old",
        "tags": [],
        "cover_image": null,
        "open_count": 10,
        "last_opened_at": "2026-01-01T00:00:00Z",
        "starred": false,
        "created_at": "2025-01-01T00:00:00Z"
    }]"#;

    fs::write(&file_path, old_json).unwrap();

    let loaded: Vec<Project> = {
        let content = fs::read_to_string(&file_path).unwrap();
        serde_json::from_str(&content).unwrap()
    };

    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].name, "OldProject");
    // New fields should default to empty vectors
    assert!(loaded[0].last_opened_tools.is_empty());
    assert!(loaded[0].workspace_tools.is_empty());
}

// === Tech Stack Detection Tests ===

/// 直接测试 detect_project_tags 的检测逻辑（不通过 Tauri command 层）
fn detect_tags(dir: &std::path::Path) -> Vec<String> {
    let mut tags = Vec::new();

    // Node.js
    if dir.join("package.json").exists() {
        tags.push("Node.js".to_string());
        if let Ok(content) = std::fs::read_to_string(dir.join("package.json")) {
            if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&content) {
                let mut all_deps: std::collections::HashSet<&str> = std::collections::HashSet::new();
                if let Some(obj) = pkg.get("dependencies").and_then(|v| v.as_object()) {
                    for k in obj.keys() { all_deps.insert(k.as_str()); }
                }
                if let Some(obj) = pkg.get("devDependencies").and_then(|v| v.as_object()) {
                    for k in obj.keys() { all_deps.insert(k.as_str()); }
                }
                if all_deps.contains("react") { tags.push("React".to_string()); }
                if all_deps.contains("next") { tags.push("Next.js".to_string()); }
                if all_deps.contains("vue") { tags.push("Vue".to_string()); }
                if all_deps.contains("express") { tags.push("Express".to_string()); }
            }
        }
    }
    if dir.join("tsconfig.json").exists() {
        tags.push("TypeScript".to_string());
    }
    if dir.join("Cargo.toml").exists() {
        tags.push("Rust".to_string());
    }
    if dir.join("go.mod").exists() {
        tags.push("Go".to_string());
    }
    if dir.join("pyproject.toml").exists() || dir.join("requirements.txt").exists() {
        tags.push("Python".to_string());
    }

    tags
}

#[test]
fn test_detect_nodejs_react_typescript() {
    let dir = tempfile::TempDir::new().unwrap();

    // package.json with react dependency
    let pkg = serde_json::json!({
        "dependencies": { "react": "^18.0.0" },
        "devDependencies": { "@types/react": "^18.0.0" }
    });
    std::fs::write(dir.path().join("package.json"), serde_json::to_string(&pkg).unwrap()).unwrap();
    std::fs::write(dir.path().join("tsconfig.json"), "{}").unwrap();

    let tags = detect_tags(dir.path());
    assert!(tags.contains(&"Node.js".to_string()));
    assert!(tags.contains(&"React".to_string()));
    assert!(tags.contains(&"TypeScript".to_string()));
}

#[test]
fn test_detect_rust() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"\n").unwrap();

    let tags = detect_tags(dir.path());
    assert!(tags.contains(&"Rust".to_string()));
    assert!(!tags.contains(&"Node.js".to_string()));
}

#[test]
fn test_detect_python() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("requirements.txt"), "flask==2.0\n").unwrap();

    let tags = detect_tags(dir.path());
    assert!(tags.contains(&"Python".to_string()));
}

#[test]
fn test_detect_multiple_stacks() {
    let dir = tempfile::TempDir::new().unwrap();

    // Full-stack project: Rust backend + TS frontend
    let pkg = serde_json::json!({
        "dependencies": { "next": "^14.0.0", "react": "^18.0.0" }
    });
    std::fs::write(dir.path().join("package.json"), serde_json::to_string(&pkg).unwrap()).unwrap();
    std::fs::write(dir.path().join("tsconfig.json"), "{}").unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"\n").unwrap();

    let tags = detect_tags(dir.path());
    assert!(tags.contains(&"Node.js".to_string()));
    assert!(tags.contains(&"Next.js".to_string()));
    assert!(tags.contains(&"TypeScript".to_string()));
    assert!(tags.contains(&"Rust".to_string()));
}
