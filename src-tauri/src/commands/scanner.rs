use ignore::WalkBuilder;

#[derive(serde::Serialize)]
pub struct GitRepo {
    pub path: String,
    pub name: String,
}

#[tauri::command]
pub fn scan_git_repos(root_path: String, max_depth: Option<usize>) -> Result<Vec<GitRepo>, String> {
    let root = std::path::Path::new(&root_path);
    if !root.exists() {
        return Err(format!("路径不存在: {}", root_path));
    }

    let mut repos = Vec::new();

    // 检查 root 自身是否就是 git 仓库
    if root.join(".git").exists() {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        repos.push(GitRepo {
            path: root.to_string_lossy().to_string(),
            name,
        });
    }

    // 遍历子目录查找 git 仓库
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .follow_links(false);

    if let Some(depth) = max_depth {
        builder.max_depth(Some(depth));
    }

    for entry in builder.build().filter_map(|e| e.ok()) {
        if !entry.file_type().is_some_and(|ft| ft.is_dir()) {
            continue;
        }

        let path = entry.path();
        if path == root {
            continue;
        }

        let git_path = path.join(".git");
        if git_path.exists() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            repos.push(GitRepo {
                path: path.to_string_lossy().to_string(),
                name,
            });
        }
    }

    Ok(repos)
}
