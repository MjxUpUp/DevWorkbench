use crate::models::GitStatus;

/// 获取单个项目的 Git 状态（当前仅分支名）。
///
/// 历史上还返回 dirty/ahead-behind/增删行数等字段，唯一消费方是已删除的
/// GitPanel。收敛为只返回分支名——TitleBar/StatusBar 面包屑只用这一个字段，
/// 且顺带移除了每次切换项目都跑 `git diff --shortstat` 的开销。
#[tauri::command]
pub fn get_git_status(project_path: String) -> Result<GitStatus, String> {
    let repo = git2::Repository::open(&project_path)
        .map_err(|e| format!("无法打开 Git 仓库: {}", e))?;

    Ok(GitStatus {
        branch: get_branch_name(&repo),
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
