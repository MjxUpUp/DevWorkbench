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

/// 统计工作区相对 HEAD 的增删行数（含未跟踪文件，遵守 .gitignore）。
///
/// 早期实现用 `git2` 逐个 `read_to_string` 未跟踪文件并递归目录，对一个含
/// `node_modules`/`target` 的项目会读取成千上万个文件，阻塞命令数十秒，导致
/// 前端 IPC 卡死、界面白屏。改为直接调用 `git diff --shortstat` —— 命令行 git
/// 原生遵守 .gitignore、单次扫描完成，且自带 --untracked-files，既快又安全。
fn count_line_changes(repo: &git2::Repository) -> (u64, u64) {
    let workdir = match repo.workdir() {
        Some(p) => p,
        None => return (0, 0),
    };

    // `git diff --shortstat HEAD --untracked-files=normal`
    // 输出形如：" 2 files changed, 42 insertions(+), 7 deletions(-)\n"
    let mut cmd = std::process::Command::new("git");
    cmd.args(["diff", "--shortstat", "--untracked-files=normal", "HEAD"])
        .current_dir(workdir);

    // CREATE_NO_WINDOW — 不弹出控制台窗口（与 editor.rs/pty.rs 一致）。
    // 没有这个标志，Windows 上每次 git 子进程都会闪一个黑框，而此命令在
    // 每次切换项目（GitPanel + TitleBar 各调一次 get_git_status）都会触发。
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let output = cmd.output();

    let stdout = match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => return (0, 0),
    };

    let parse = |key: &str| -> u64 {
        stdout.lines().next().and_then(|line| {
            let mut tokens = line.split_whitespace();
            let mut prev: Option<&str> = None;
            for tok in tokens.by_ref() {
                if tok.starts_with(key) {
                    if let Some(n) = prev {
                        if let Ok(v) = n.parse::<u64>() {
                            return Some(v);
                        }
                    }
                }
                prev = Some(tok);
            }
            None
        }).unwrap_or(0)
    };

    (parse("insertion"), parse("deletion"))
}
