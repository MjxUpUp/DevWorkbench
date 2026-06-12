use crate::error::AppError;

/// List files in a project directory, returning a flat list of relative paths.
/// Used by the @ file trigger in the Composer.
#[tauri::command]
pub fn list_project_files(project_path: String) -> Result<Vec<FileEntry>, AppError> {
    let root = std::path::Path::new(&project_path);
    if !root.exists() || !root.is_dir() {
        return Err(AppError::NotFound(format!("项目目录不存在: {}", project_path)));
    }

    let mut entries = Vec::new();
    let max_depth = 3;
    let max_entries = 200;

    collect_files(root, root, 0, max_depth, max_entries, &mut entries);

    // Sort: directories first, then files, alphabetically
    entries.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.path.cmp(&b.path),
        }
    });

    Ok(entries)
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
}

fn collect_files(
    root: &std::path::Path,
    current: &std::path::Path,
    depth: usize,
    max_depth: usize,
    max_entries: usize,
    entries: &mut Vec<FileEntry>,
) {
    if depth > max_depth || entries.len() >= max_entries {
        return;
    }

    // Skip hidden dirs, node_modules, target, .git, etc.
    let skip_dirs = [
        ".git", "node_modules", "target", "dist", ".next", ".nuxt",
        "__pycache__", ".venv", "venv", ".tox", "build", ".build",
        ".cache", ".claude", ".openclaw",
    ];

    let Ok(dir_entries) = std::fs::read_dir(current) else {
        return;
    };

    let mut dir_entries: Vec<_> = dir_entries.filter_map(|e| e.ok()).collect();
    dir_entries.sort_by_key(|e| e.file_name());

    for entry in dir_entries {
        if entries.len() >= max_entries {
            break;
        }

        let file_name = entry.file_name();
        let name = file_name.to_string_lossy().to_string();

        // Skip hidden files/dirs
        if name.starts_with('.') && name != ".env" && name != ".env.local" {
            continue;
        }

        let path = entry.path();
        let is_dir = path.is_dir();

        if is_dir {
            if skip_dirs.contains(&name.as_str()) {
                continue;
            }
            let relative = path.strip_prefix(root).unwrap_or(&path);
            entries.push(FileEntry {
                path: relative.to_string_lossy().to_string(),
                name,
                is_dir: true,
            });
            collect_files(root, &path, depth + 1, max_depth, max_entries, entries);
        } else {
            // Skip binary/common non-interesting files
            let ext = path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let skip_exts = [
                "exe", "dll", "so", "dylib", "o", "obj", "pyc", "pyo",
                "wasm", "map", "lock", "log", "sqlite", "db",
            ];
            if skip_exts.contains(&ext.as_str()) {
                continue;
            }

            let relative = path.strip_prefix(root).unwrap_or(&path);
            entries.push(FileEntry {
                path: relative.to_string_lossy().to_string(),
                name,
                is_dir: false,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_list_files_basic() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src").join("main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        fs::write(root.join("README.md"), "# Test").unwrap();

        let entries = list_project_files(root.to_string_lossy().to_string()).unwrap();
        assert!(!entries.is_empty());

        // Normalize paths to forward slashes for cross-platform comparison
        let paths: Vec<String> = entries.iter().map(|e| e.path.replace('\\', "/")).collect();
        assert!(paths.iter().any(|p| p == "Cargo.toml"));
        assert!(paths.iter().any(|p| p == "README.md"));
        assert!(paths.iter().any(|p| p == "src"));
        assert!(paths.iter().any(|p| p == "src/main.rs"));
    }

    #[test]
    fn test_skips_node_modules_and_git() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        fs::create_dir_all(root.join("node_modules").join("pkg")).unwrap();
        fs::create_dir_all(root.join(".git").join("objects")).unwrap();
        fs::write(root.join("package.json"), "{}").unwrap();

        let entries = list_project_files(root.to_string_lossy().to_string()).unwrap();
        let paths: Vec<String> = entries.iter().map(|e| e.path.replace('\\', "/")).collect();
        assert!(!paths.iter().any(|p| p.starts_with("node_modules")));
        assert!(!paths.iter().any(|p| p.starts_with(".git")));
        assert!(paths.iter().any(|p| p == "package.json"));
    }

    #[test]
    fn test_nonexistent_dir() {
        let result = list_project_files("/nonexistent/path/12345".to_string());
        assert!(result.is_err());
    }
}
