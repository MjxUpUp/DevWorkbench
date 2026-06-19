use crate::db::DbState;
use crate::models::{AgentType, ToolStatus};
use tauri::State;

/// macOS GUI 应用 PATH 不包含 brew 等路径，需要手动扩展
#[cfg(target_os = "macos")]
fn expanded_paths() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    vec![
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
        format!("{}/.cargo/bin", home),
        format!("{}/.npm-global/bin", home),
        format!("{}/.local/bin", home),
    ]
}

/// 在扩展 PATH 中查找可执行文件
#[cfg(target_os = "macos")]
pub(crate) fn which_expanded(name: &str) -> Option<std::path::PathBuf> {
    // 先尝试默认 which
    if let Ok(p) = which::which(name) {
        return Some(p);
    }
    // 再搜索扩展路径
    for dir in expanded_paths() {
        let candidate = std::path::Path::new(&dir).join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn which_expanded(name: &str) -> Option<std::path::PathBuf> {
    which::which(name).ok()
}

/// 在 Windows 上定位 git-bash 的 `bash.exe`。优先级：
///   1. 环境变量 DEVWORKBENCH_BASH_PATH
///   2. custom 参数（预留：未来从 settings DB 列读；当前调用方传 None）
///   3. %ProgramFiles%\Git\bin\bash.exe
///   4. %ProgramFiles(x86)%\Git\bin\bash.exe
///   5. which::which("bash.exe")（PATH 上任意 bash）
///
/// 每步用 `.exists()` 校验；全部失败返回 None。**不降级到 cmd**——由调用方
/// 决定 None 的处理（BashTool 返回 Err 带安装指引，终端启动器报错）。
/// 之前 BashTool 用 cmd /C 导致 agent 发的 Unix 命令（ls/find）失败、盲切
/// 语法死循环烧光步数预算（回归 70e762f7），锁 git-bash 从源头消除该问题。
#[cfg(target_os = "windows")]
pub fn resolve_git_bash(custom: Option<&str>) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let candidates: Vec<PathBuf> = [
        std::env::var_os("DEVWORKBENCH_BASH_PATH").map(PathBuf::from),
        custom.map(PathBuf::from),
        std::env::var_os("ProgramFiles")
            .map(|pf| PathBuf::from(pf).join("Git").join("bin").join("bash.exe")),
        std::env::var_os("ProgramFiles(x86)")
            .map(|pf| PathBuf::from(pf).join("Git").join("bin").join("bash.exe")),
    ]
    .into_iter()
    .flatten()
    .collect();
    for c in candidates {
        if c.exists() {
            return Some(c);
        }
    }
    which::which("bash.exe").ok()
}

/// Unix 永远用原生 sh（见 BashTool / terminal.rs 的非 Windows 分支），此函数
/// 仅作占位让跨平台调用点编译通过，运行时不会被调用。
#[cfg(not(target_os = "windows"))]
pub fn resolve_git_bash(_custom: Option<&str>) -> Option<std::path::PathBuf> {
    None
}

/// Non-agent tools that are detected alongside agents (IDE, VCS)
const NON_AGENT_TOOLS: &[&str] = &["code", "git"];

#[tauri::command]
pub fn detect_tools(db: State<'_, DbState>) -> Vec<ToolStatus> {
    let conn = match db.get() {
        Ok(c) => c,
        Err(e) => {
            log::error!("detect_tools: pool get failed: {e}");
            return Vec::new();
        }
    };
    // 读取用户自定义路径
    let custom_paths = crate::commands::projects::load_settings_from_db(&conn)
        .ok()
        .map(|s| s.tool_paths)
        .unwrap_or_default();
    drop(conn);

    let mut results = Vec::new();

    // Agent tools — derived from AgentType enum (single source of truth)
    for agent_type in AgentType::all() {
        let cmd = agent_type.command_name();
        results.push(detect_one(cmd, &custom_paths));
    }

    // Non-agent tools (IDE, VCS)
    for &name in NON_AGENT_TOOLS {
        results.push(detect_one(name, &custom_paths));
    }

    results
}

fn detect_one(name: &str, custom_paths: &std::collections::HashMap<String, String>) -> ToolStatus {
    // 优先级：1. 用户自定义路径  2. which 查找（含扩展 PATH）
    if let Some(custom) = custom_paths.get(name) {
        if !custom.is_empty() {
            return ToolStatus {
                name: name.to_string(),
                installed: true,
                path: Some(custom.clone()),
            };
        }
    }

    match which_expanded(name) {
        Some(path) => ToolStatus {
            name: name.to_string(),
            installed: true,
            path: Some(path.to_string_lossy().to_string()),
        },
        None => ToolStatus {
            name: name.to_string(),
            installed: false,
            path: None,
        },
    }
}

#[cfg(test)]
#[cfg(target_os = "windows")]
mod tests {
    use super::*;
    use serial_test::serial;

    /// 进程级 env 是全局共享的，即便 #[serial] 串行执行也会泄漏到后续测试。
    /// EnvGuard 在 Drop 时还原原值，保证测试隔离。
    struct EnvGuard {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }
    impl EnvGuard {
        fn set(key: &'static str, val: &str) -> Self {
            let old = std::env::var_os(key);
            std::env::set_var(key, val);
            Self { key, old }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.old.clone() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    #[serial]
    fn resolve_git_bash_env_override_wins() {
        // env 指向真实存在的（假）bash.exe → 必须优先返回它，而非走
        // ProgramFiles/which。空文件即可，探测只 .exists()。
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("bash.exe");
        std::fs::write(&fake, b"").unwrap();
        let _g = EnvGuard::set("DEVWORKBENCH_BASH_PATH", fake.to_str().unwrap());
        let got = resolve_git_bash(None);
        assert_eq!(got, Some(fake));
    }

    #[test]
    #[serial]
    fn resolve_git_bash_programfiles_concat() {
        // env 不设（显式空跳过 env 步）；ProgramFiles 指向 tempdir，其下造
        // Git/bin/bash.exe → 必须拼出该路径并命中。
        let dir = tempfile::tempdir().unwrap();
        let git_bin = dir.path().join("Git").join("bin");
        std::fs::create_dir_all(&git_bin).unwrap();
        let fake = git_bin.join("bash.exe");
        std::fs::write(&fake, b"").unwrap();
        let _g_env = EnvGuard::set("DEVWORKBENCH_BASH_PATH", "");
        let _g_pf = EnvGuard::set("ProgramFiles", dir.path().to_str().unwrap());
        let got = resolve_git_bash(None);
        assert_eq!(got, Some(fake));
    }

    #[test]
    #[serial]
    fn resolve_git_bash_env_missing_path_skipped() {
        // env 指向不存在的路径 → env 步 .exists() 返回 false 必须跳过，不能
        // 把那个假路径返回给调用方。后续走 ProgramFiles/which（开发机可能命中
        // 真 git-bash），所以只断言「返回的绝不是 env 那个不存在路径」。
        let bogus = r"C:\nonexistent\devworkbench_test\bash.exe";
        let _g = EnvGuard::set("DEVWORKBENCH_BASH_PATH", bogus);
        let got = resolve_git_bash(None);
        assert_ne!(
            got.map(|p| p.to_string_lossy().to_string()),
            Some(bogus.to_string())
        );
    }
}
