use ignore::WalkBuilder;
use std::collections::HashSet;
use std::path::Path;

#[derive(serde::Serialize)]
pub struct GitRepo {
    pub path: String,
    pub name: String,
}

#[tauri::command]
pub fn scan_git_repos(root_path: String, max_depth: Option<usize>) -> Result<Vec<GitRepo>, String> {
    let root = Path::new(&root_path);
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

/// 检测项目技术栈标签
#[tauri::command]
pub fn detect_project_tags(project_path: String) -> Result<Vec<String>, String> {
    let root = Path::new(&project_path);
    if !root.exists() {
        return Err(format!("路径不存在: {}", project_path));
    }

    let mut tags = Vec::new();

    // === Node.js / JavaScript 生态 ===
    if root.join("package.json").exists() {
        tags.push("Node.js".to_string());
        if let Ok(content) = std::fs::read_to_string(root.join("package.json")) {
            if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&content) {
                // 收集所有依赖名到 HashSet<&str>，便于 O(1) 查找
                let mut all_deps: HashSet<&str> = HashSet::new();
                if let Some(obj) = pkg.get("dependencies").and_then(|v| v.as_object()) {
                    for k in obj.keys() {
                        all_deps.insert(k.as_str());
                    }
                }
                if let Some(obj) = pkg.get("devDependencies").and_then(|v| v.as_object()) {
                    for k in obj.keys() {
                        all_deps.insert(k.as_str());
                    }
                }

                // 框架检测（按优先级，互斥）
                if all_deps.contains("next") {
                    tags.push("Next.js".to_string());
                } else if all_deps.contains("nuxt") {
                    tags.push("Nuxt".to_string());
                } else if all_deps.contains("svelte") || all_deps.contains("@sveltejs/kit") {
                    tags.push("Svelte".to_string());
                } else if all_deps.contains("vue") || has_prefix(&all_deps, "@vue/") {
                    tags.push("Vue".to_string());
                } else if all_deps.contains("react") || has_prefix(&all_deps, "@react") {
                    tags.push("React".to_string());
                } else if all_deps.contains("angular") || has_prefix(&all_deps, "@angular/") {
                    tags.push("Angular".to_string());
                }

                // 构建工具（互斥）
                if all_deps.contains("vite") || has_prefix(&all_deps, "@vitejs/") {
                    tags.push("Vite".to_string());
                } else if all_deps.contains("webpack") || has_prefix(&all_deps, "webpack-") {
                    tags.push("Webpack".to_string());
                }

                // 后端框架（互斥）
                if all_deps.contains("express") {
                    tags.push("Express".to_string());
                } else if all_deps.contains("fastify") {
                    tags.push("Fastify".to_string());
                } else if all_deps.contains("koa") {
                    tags.push("Koa".to_string());
                }

                // 桌面框架
                if has_prefix(&all_deps, "@tauri-apps/") {
                    tags.push("Tauri".to_string());
                }
                if all_deps.contains("electron") {
                    tags.push("Electron".to_string());
                }
            }
        }
    }

    // TypeScript（独立检测，可能有 tsconfig 但没有 package.json）
    if root.join("tsconfig.json").exists() {
        tags.push("TypeScript".to_string());
    }

    // === Rust ===
    if root.join("Cargo.toml").exists() {
        tags.push("Rust".to_string());
    }

    // === Go ===
    if root.join("go.mod").exists() {
        tags.push("Go".to_string());
    }

    // === Python ===
    if root.join("pyproject.toml").exists()
        || root.join("requirements.txt").exists()
        || root.join("setup.py").exists()
        || root.join("Pipfile").exists()
    {
        tags.push("Python".to_string());
    }

    // === .NET ===
    if has_ext_in_dir(root, ".sln") || has_ext_in_dir(root, ".csproj") {
        tags.push(".NET".to_string());
    }

    // === Java / Kotlin ===
    if root.join("pom.xml").exists()
        || root.join("build.gradle").exists()
        || root.join("build.gradle.kts").exists()
    {
        tags.push("Java".to_string());
    }

    // === Ruby ===
    if root.join("Gemfile").exists() {
        tags.push("Ruby".to_string());
    }

    // === PHP ===
    if root.join("composer.json").exists() {
        tags.push("PHP".to_string());
    }

    // === Flutter / Dart ===
    if root.join("pubspec.yaml").exists() {
        tags.push("Flutter".to_string());
    }

    Ok(tags)
}

/// 检查 HashSet 中是否有任何元素以指定前缀开头
fn has_prefix(set: &HashSet<&str>, prefix: &str) -> bool {
    set.iter().any(|k| k.starts_with(prefix))
}

/// 检查目录中是否有指定后缀的文件
fn has_ext_in_dir(dir: &Path, ext: &str) -> bool {
    std::fs::read_dir(dir)
        .ok()
        .map(|mut entries| {
            entries.any(|e| {
                e.ok()
                    .and_then(|e| e.file_name().to_str().map(|n| n.ends_with(ext)))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}
