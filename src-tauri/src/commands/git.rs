use crate::models::GitStatus;

/// 获取单个项目的 Git 状态
#[tauri::command]
pub fn get_git_status(project_path: String) -> Result<GitStatus, String> {
    let repo = git2::Repository::open(&project_path)
        .map_err(|e| format!("无法打开 Git 仓库: {}", e))?;

    let branch = get_branch_name(&repo);
    let is_dirty = is_repo_dirty(&repo);
    let (ahead, behind) = get_ahead_behind(&repo);
    let last_commit_time = get_last_commit_time(&repo);
    let (insertions, deletions) = count_line_changes(&repo);

    Ok(GitStatus {
        branch,
        is_dirty,
        ahead,
        behind,
        last_commit_time,
        insertions,
        deletions,
    })
}

/// 批量获取多个项目的 Git 状态
#[tauri::command]
pub fn batch_get_git_status(project_paths: Vec<String>) -> Result<Vec<(String, Option<GitStatus>)>, String> {
    let results = project_paths
        .into_iter()
        .map(|path| {
            let status = get_git_status(path.clone()).ok();
            (path, status)
        })
        .collect();
    Ok(results)
}

fn get_branch_name(repo: &git2::Repository) -> String {
    // 先尝试获取当前 HEAD 指向的分支名
    if let Ok(head) = repo.head() {
        if let Some(name) = head.shorthand() {
            return name.to_string();
        }
    }
    // detached HEAD 或其他情况
    "HEAD (detached)".to_string()
}

fn is_repo_dirty(repo: &git2::Repository) -> bool {
    let mut opts = git2::StatusOptions::new();
    // 包含工作区和暂存区的变更
    opts.include_untracked(true)
        .recurse_untracked_dirs(false)
        .exclude_submodules(true);

    match repo.statuses(Some(&mut opts)) {
        Ok(statuses) => statuses.iter().any(|s| {
            let s = s.status();
            // 任何非"当前"状态都算 dirty
            s != git2::Status::CURRENT
                && (s.is_wt_new()
                    || s.is_wt_modified()
                    || s.is_wt_deleted()
                    || s.is_wt_renamed()
                    || s.is_index_new()
                    || s.is_index_modified()
                    || s.is_index_deleted()
                    || s.is_index_renamed()
                    || s.is_conflicted())
        }),
        Err(_) => false,
    }
}

fn get_ahead_behind(repo: &git2::Repository) -> (u32, u32) {
    // 获取当前分支的 HEAD commit
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return (0, 0),
    };

    let branch_name = match head.shorthand() {
        Some(name) => name,
        None => return (0, 0),
    };

    // 查找本地分支
    let branch = match repo.find_branch(branch_name, git2::BranchType::Local) {
        Ok(b) => b,
        Err(_) => return (0, 0),
    };

    // 查找上游分支
    let upstream = match branch.upstream() {
        Ok(u) => u,
        Err(_) => return (0, 0), // 没有上游，不算 ahead/behind
    };

    let local_oid = match branch.get().target() {
        Some(oid) => oid,
        None => return (0, 0),
    };

    let upstream_oid = match upstream.get().target() {
        Some(oid) => oid,
        None => return (0, 0),
    };

    match repo.graph_ahead_behind(local_oid, upstream_oid) {
        Ok((a, b)) => (a as u32, b as u32),
        Err(_) => (0, 0),
    }
}

fn get_last_commit_time(repo: &git2::Repository) -> Option<String> {
    let head = repo.head().ok()?;
    let commit = repo.find_commit(head.target()?).ok()?;
    let time = commit.time();
    let secs = time.seconds();
    let dt = chrono::DateTime::from_timestamp(secs, 0)?;
    Some(dt.to_rfc3339())
}

/// 统计工作区相对 HEAD 的增删行数。
///
/// 返回 `(insertions, deletions)`：
/// - 已跟踪文件用 `git2` 的 `DiffStats` 统计 HEAD→worktree 的行差异；
/// - 未跟踪文件按其内容行数计入 insertions（与 `git diff --stat` 的语义一致）。
/// - 空 HEAD 仓库只统计未跟踪文件。
fn count_line_changes(repo: &git2::Repository) -> (u64, u64) {
    let mut insertions: u64 = 0;
    let mut deletions: u64 = 0;

    // HEAD→worktree 的已跟踪文件行差异
    if let Ok(head_tree) = repo.head() {
        if let Ok(head_commit) = head_tree.peel_to_commit() {
            if let Ok(head_tree) = head_commit.tree() {
                // diff_tree_to_tree with old=None compares against the worktree
                // (the diff Tree-to-workdir variant in this git2 version).
                if let Ok(diff) = repo.diff_tree_to_workdir(Some(&head_tree), None) {
                    if let Ok(stats) = diff.stats() {
                        insertions += stats.insertions() as u64;
                        deletions += stats.deletions() as u64;
                    }
                }
            }
        }
    }

    // 未跟踪文件：整份内容计入 insertions
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .exclude_submodules(true);

    if let Ok(statuses) = repo.statuses(Some(&mut opts)) {
        let workdir = repo.workdir();
        for entry in statuses.iter() {
            if !entry.status().is_wt_new() {
                continue;
            }
            let Some(path) = entry.path() else { continue };
            let Some(root) = workdir else { continue };
            let file = root.join(path);
            if let Ok(text) = std::fs::read_to_string(&file) {
                // 末尾换行不额外计一行；空文件计 0 行
                if !text.is_empty() {
                    insertions += text.lines().count() as u64;
                }
            }
        }
    }

    (insertions, deletions)
}
