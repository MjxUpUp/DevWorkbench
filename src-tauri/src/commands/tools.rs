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
///   3. 从 PATH 上 git.exe 反推 <install>\bin\bash.exe（最稳健：git-bash 装
///      在非 C 盘/D 盘/便携位时，固定位探测全落空，git.exe 默认在 PATH 而
///      bash.exe 不一定，反推是唯一可靠定位；见 git_bash_from_git_exe）
///   4. %ProgramFiles%\Git\bin\bash.exe
///   5. %ProgramFiles(x86)%\Git\bin\bash.exe
///   6. PATH 上首个非 WSL 的 bash.exe（遍历，跳过 System32 + WindowsApps 入口）
///
/// 每步用 `.exists()` 校验；全部失败返回 None。**不降级到 cmd**——由调用方
/// 决定 None 的处理（BashTool 返回 Err 带安装指引，终端启动器报错）。
/// 之前 BashTool 用 cmd /C 导致 agent 发的 Unix 命令（ls/find）失败、盲切
/// 语法死循环烧光步数预算（回归 70e762f7），锁 git-bash 从源头消除该问题。
/// 兜底（第 6 步）会跳过 WSL 的 bash.exe（System32 + WindowsApps 两个入口）——
/// 它们在路径风格/用户态上与 git-bash 不一致，静默采用会让 agent 的 bash
/// 工具跑进 WSL（见 looks_like_wsl_bash）。第 3 步反推是 da2975fe 修复的核心：
/// 该机器 git-bash 装在 D:\Program Files\Git，C 盘固定位不存在、bin 不在 PATH，
/// 旧实现兜底遍历误中 WindowsApps 的 WSL bash。
#[cfg(target_os = "windows")]
pub fn resolve_git_bash(custom: Option<&str>) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let candidates: Vec<PathBuf> = [
        std::env::var_os("DEVWORKBENCH_BASH_PATH").map(PathBuf::from),
        custom.map(PathBuf::from),
        git_bash_from_git_exe(),
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
    // 遍历 PATH 上所有 bash.exe，跳过 WSL 入口（<SystemRoot>\System32\bash.exe）。
    // 装了 WSL 的机器 System32 在 PATH 最前，旧实现用 which::which 只取首个会
    // 恒命中 WSL 的 bash.exe，被静默当 git-bash 采用——agent 的 bash 工具就跑
    // 进了 WSL（pwd=/mnt/e/...、Linux 用户态）而非 MINGW git-bash（pwd=/e/...）。
    // 两者路径风格/工具链不一致会持续扰动 agent；故遍历找首个非 WSL 的，找不到
    // 就返回 None（上层 BashTool 报"git-bash 未找到"带安装指引），不静默采用
    // WSL。手动 split_paths 而非 which_all，绕开 which 各版本迭代器 item 类型
    // 差异（PathBuf vs Result<PathBuf>），显式控制跳过逻辑。
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = dir.join("bash.exe");
            if cand.exists() && !looks_like_wsl_bash(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

/// 判断 bash.exe 路径是否为 WSL 安装入口（而非 MINGW git-bash）。WSL 有两个
/// Windows 端入口，git-bash 永不落在任何之一：
///   1. <SystemRoot>\System32\bash.exe（系统内置 LXSS 子系统入口）
///   2. <LocalAppData>\Microsoft\WindowsApps\bash.exe（Microsoft Store / appx
///      分发的 WSL 入口——与 System32 入口同效，都把 shell 跑进 WSL Linux
///      用户态；装了 WSL 的机器两者常并存于 PATH）。
/// 优先精确比对 SystemRoot，再退到路径启发式（小写 + 斜杠统一）兜住非标准
/// 安装。WindowsApps 入口是会话 da2975fe 的泄漏点——旧实现只检 system32
/// 后缀，PATH 兜底遍历时跳过 System32 却命中 WindowsApps，把 Store 版 WSL
/// bash 当 git-bash 返回，agent bash 跑进 WSL。
#[cfg(target_os = "windows")]
fn looks_like_wsl_bash(path: &std::path::Path) -> bool {
    let norm = |p: &std::path::Path| p.to_string_lossy().replace('/', r"\").to_ascii_lowercase();
    let n = norm(path);
    if let Some(sr) = std::env::var_os("SystemRoot") {
        let wsl = std::path::PathBuf::from(sr)
            .join("System32")
            .join("bash.exe");
        if n == norm(&wsl) {
            return true;
        }
    }
    // 两种 WSL 入口的后缀启发（git-bash 永不落 system32 或 WindowsApps）。
    n.ends_with(r"\system32\bash.exe") || n.ends_with(r"\windowsapps\bash.exe")
}

/// 从一个 git.exe 路径向上（最多 3 层）找 git 安装根下的 `bin\bash.exe`。抽成
/// 纯函数（不依赖 PATH/which）以便单元测试。Git for Windows 里 git.exe 有两
/// 种布局：`<install>\cmd\git.exe`（系统 PATH 默认加的位）和
/// `<install>\mingw64\bin\git.exe`（mingw 内部委派位）。向上 3 层覆盖两者：
/// cmd→install（2 层）、mingw64\bin→install（3 层）。
#[cfg(target_os = "windows")]
fn find_bash_near(git_exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut cur = git_exe.parent();
    for _ in 0..3 {
        let dir = cur?;
        let bash = dir.join("bin").join("bash.exe");
        if bash.exists() && !looks_like_wsl_bash(&bash) {
            return Some(bash);
        }
        cur = dir.parent();
    }
    None
}

/// 从 PATH 上的 git.exe 反推 git-bash。git.exe 几乎总在系统 PATH（Git for
/// Windows 安装默认把 `<install>\cmd` 加进 PATH），而 bash.exe 不一定在——当
/// git-bash 装在非 `C:\Program Files\Git`（D 盘、便携位）时，固定路径探测全
/// 落空，从 git.exe 反推是唯一可靠的定位方式（见会话 da2975fe：git-bash 装在
/// D:\Program Files\Git，C 盘固定位不存在，PATH 兜底误中 WSL）。
#[cfg(target_os = "windows")]
fn git_bash_from_git_exe() -> Option<std::path::PathBuf> {
    find_bash_near(&which_expanded("git")?)
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

    #[test]
    fn looks_like_wsl_bash_detects_system32_entry() {
        // 标准 WSL 入口（大小写/盘符/正反斜杠变体都要命中）。
        assert!(looks_like_wsl_bash(std::path::Path::new(
            r"C:\Windows\System32\bash.exe"
        )));
        assert!(looks_like_wsl_bash(std::path::Path::new(
            r"c:\windows\system32\bash.exe"
        )));
        assert!(looks_like_wsl_bash(std::path::Path::new(
            r"C:/Windows/System32/bash.exe"
        )));
    }

    #[test]
    fn looks_like_wsl_bash_rejects_git_bash_locations() {
        // 真 MINGW git-bash（标准位 + 便携位）绝不能被误判为 WSL——否则兜底
        // 会把用户真正想用的 git-bash 也跳过。
        assert!(!looks_like_wsl_bash(std::path::Path::new(
            r"C:\Program Files\Git\bin\bash.exe"
        )));
        assert!(!looks_like_wsl_bash(std::path::Path::new(
            r"D:\tools\PortableGit\bin\bash.exe"
        )));
    }

    #[test]
    fn looks_like_wsl_bash_detects_windowsapps_entry() {
        // Microsoft Store / appx 分发的 WSL 入口（System32 之外的第二入口）。
        // 旧实现只检 system32 后缀，漏了这条 → PATH 兜底误把它当 git-bash
        // 返回（会话 da2975fe 的泄漏点）。
        assert!(looks_like_wsl_bash(std::path::Path::new(
            r"C:\Users\me\AppData\Local\Microsoft\WindowsApps\bash.exe"
        )));
        // 正反斜杠 + 大小写变体都要命中（norm 统一了）。
        assert!(looks_like_wsl_bash(std::path::Path::new(
            r"c:/users/me/appdata/local/microsoft/windowsapps/bash.exe"
        )));
    }

    #[test]
    fn find_bash_near_finds_cmd_layout() {
        // <install>\cmd\git.exe → 向上到 install root，bin\bash.exe 命中。
        // 模拟会话 da2975fe 的 D:\Program Files\Git 布局（cmd 在 PATH，bin 不在）。
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path();
        std::fs::create_dir_all(install.join("cmd")).unwrap();
        std::fs::create_dir_all(install.join("bin")).unwrap();
        let git = install.join("cmd").join("git.exe");
        let bash = install.join("bin").join("bash.exe");
        std::fs::write(&git, b"").unwrap();
        std::fs::write(&bash, b"").unwrap();
        assert_eq!(find_bash_near(&git), Some(bash));
    }

    #[test]
    fn find_bash_near_finds_mingw64_layout() {
        // <install>\mingw64\bin\git.exe → 向上 3 层到 install root，bin\bash.exe。
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path();
        std::fs::create_dir_all(install.join("mingw64").join("bin")).unwrap();
        std::fs::create_dir_all(install.join("bin")).unwrap();
        let git = install.join("mingw64").join("bin").join("git.exe");
        let bash = install.join("bin").join("bash.exe");
        std::fs::write(&git, b"").unwrap();
        std::fs::write(&bash, b"").unwrap();
        assert_eq!(find_bash_near(&git), Some(bash));
    }

    #[test]
    fn find_bash_near_returns_none_when_no_bash() {
        // git.exe 存在但安装根下没有 bin\bash.exe（不是 Git for Windows，或
        // 被裁剪的安装）→ 不能瞎返回，调用方应继续试下一位/兜底。
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path();
        std::fs::create_dir_all(install.join("cmd")).unwrap();
        let git = install.join("cmd").join("git.exe");
        std::fs::write(&git, b"").unwrap();
        assert_eq!(find_bash_near(&git), None);
    }
}
