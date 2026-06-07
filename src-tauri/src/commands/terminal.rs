use std::process::Command;
use serde::{Deserialize, Serialize};

/// 校验命令字符串只包含安全字符（字母、数字、横杠、下划线、点、空格）
/// 阻止 shell 元字符（; | $ ` & > < ( ) 等）注入
fn validate_command(cmd: &str) -> Result<(), String> {
    if cmd.is_empty() {
        return Ok(());
    }
    for ch in cmd.chars() {
        if !ch.is_alphanumeric() && ch != '-' && ch != '_' && ch != '.' && ch != ' ' {
            return Err(format!("命令包含非法字符: '{}'", ch));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalInfo {
    pub id: String,
    pub label: String,
    pub available: bool,
}

/// 检测可执行文件是否存在于 PATH 中
fn which_exists(name: &str) -> bool {
    which::which(name).is_ok()
}

#[tauri::command]
pub fn detect_terminals() -> Result<Vec<TerminalInfo>, String> {
    let mut terminals = Vec::new();

    #[cfg(target_os = "windows")]
    {
        // Windows Terminal
        terminals.push(TerminalInfo {
            id: "wt".into(),
            label: "Windows Terminal".into(),
            available: which_exists("wt.exe"),
        });
        // PowerShell 7
        terminals.push(TerminalInfo {
            id: "pwsh".into(),
            label: "PowerShell 7".into(),
            available: which_exists("pwsh.exe"),
        });
        // PowerShell 5 (Windows PowerShell)
        terminals.push(TerminalInfo {
            id: "powershell".into(),
            label: "Windows PowerShell".into(),
            available: which_exists("powershell.exe"),
        });
        // Git Bash
        let git_bash_available = which_exists("bash.exe")
            || std::path::Path::new(r"C:\Program Files\Git\bin\bash.exe").exists()
            || std::path::Path::new(r"C:\Program Files (x86)\Git\bin\bash.exe").exists();
        terminals.push(TerminalInfo {
            id: "git-bash".into(),
            label: "Git Bash".into(),
            available: git_bash_available,
        });
        // CMD (always available on Windows)
        terminals.push(TerminalInfo {
            id: "cmd".into(),
            label: "命令提示符".into(),
            available: true,
        });
    }

    #[cfg(target_os = "macos")]
    {
        // iTerm2
        terminals.push(TerminalInfo {
            id: "iterm".into(),
            label: "iTerm2".into(),
            available: std::path::Path::new("/Applications/iTerm.app").exists(),
        });
        // Warp
        terminals.push(TerminalInfo {
            id: "warp".into(),
            label: "Warp".into(),
            available: std::path::Path::new("/Applications/Warp.app").exists(),
        });
        // Terminal.app (always available)
        terminals.push(TerminalInfo {
            id: "terminal".into(),
            label: "Terminal".into(),
            available: true,
        });
        // Alacritty
        terminals.push(TerminalInfo {
            id: "alacritty".into(),
            label: "Alacritty".into(),
            available: which_exists("alacritty"),
        });
        // Kitty
        terminals.push(TerminalInfo {
            id: "kitty".into(),
            label: "Kitty".into(),
            available: which_exists("kitty"),
        });
    }

    #[cfg(target_os = "linux")]
    {
        terminals.push(TerminalInfo {
            id: "gnome-terminal".into(),
            label: "GNOME Terminal".into(),
            available: which_exists("gnome-terminal"),
        });
        terminals.push(TerminalInfo {
            id: "konsole".into(),
            label: "Konsole".into(),
            available: which_exists("konsole"),
        });
        terminals.push(TerminalInfo {
            id: "alacritty".into(),
            label: "Alacritty".into(),
            available: which_exists("alacritty"),
        });
        terminals.push(TerminalInfo {
            id: "kitty".into(),
            label: "Kitty".into(),
            available: which_exists("kitty"),
        });
        terminals.push(TerminalInfo {
            id: "terminator".into(),
            label: "Terminator".into(),
            available: which_exists("terminator"),
        });
    }

    Ok(terminals)
}

#[tauri::command]
pub fn open_terminal(working_dir: String, command: Option<String>) -> Result<(), String> {
    let dir = std::path::Path::new(&working_dir);
    if !dir.exists() {
        return Err(format!("目录不存在: {}", working_dir));
    }

    let cmd = command.unwrap_or_default();
    validate_command(&cmd)?;

    // 读取用户设置的偏好终端
    let preferred = crate::commands::projects::load_settings()
        .ok()
        .map(|s| s.preferred_terminal)
        .filter(|p| !p.is_empty())
        .unwrap_or_default();

    // 如果没有设置偏好终端，自动选第一个可用的并持久化
    let terminal_id = if preferred.is_empty() {
        let terminals = detect_terminals()?;
        let first_available = terminals.iter()
            .find(|t| t.available)
            .map(|t| t.id.clone())
            .unwrap_or_else(|| "cmd".to_string());
        // 自动保存，下次不再重新检测
        if let Ok(mut settings) = crate::commands::projects::load_settings() {
            settings.preferred_terminal = first_available.clone();
            let _ = crate::commands::projects::save_settings(settings);
        }
        first_available
    } else {
        preferred
    };

    // 根据终端 ID 分发到对应启动器
    #[cfg(target_os = "windows")]
    {
        match terminal_id.as_str() {
            "wt" => launch_windows_wt(&working_dir, &cmd),
            "pwsh" => launch_windows_pwsh(&working_dir, &cmd),
            "powershell" => launch_windows_powershell(&working_dir, &cmd),
            "git-bash" => launch_windows_git_bash(&working_dir, &cmd),
            "cmd" => launch_windows_cmd(&working_dir, &cmd),
            other => Err(format!("未知终端: {}", other)),
        }
    }

    #[cfg(target_os = "macos")]
    {
        match terminal_id.as_str() {
            "iterm" => launch_macos_iterm(&working_dir, &cmd),
            "warp" => launch_macos_warp(&working_dir, &cmd),
            "terminal" => launch_macos_terminal(&working_dir, &cmd),
            "alacritty" => launch_macos_alacritty(&working_dir, &cmd),
            "kitty" => launch_macos_kitty(&working_dir, &cmd),
            other => Err(format!("未知终端: {}", other)),
        }
    }

    #[cfg(target_os = "linux")]
    {
        match terminal_id.as_str() {
            "gnome-terminal" => launch_linux_gnome(&working_dir, &cmd),
            "konsole" => launch_linux_konsole(&working_dir, &cmd),
            "alacritty" => launch_linux_alacritty(&working_dir, &cmd),
            "kitty" => launch_linux_kitty(&working_dir, &cmd),
            "terminator" => launch_linux_terminator(&working_dir, &cmd),
            other => Err(format!("未知终端: {}", other)),
        }
    }
}

// ==================== Windows Launchers ====================

#[cfg(target_os = "windows")]
fn launch_windows_wt(working_dir: &str, cmd: &str) -> Result<(), String> {
    let result = if !cmd.is_empty() {
        // 优先 pwsh → powershell → cmd，匹配用户实际使用的 shell
        if which_exists("pwsh.exe") {
            Command::new("wt.exe")
                .args(["-d", working_dir, "--", "pwsh.exe", "-NoExit", "-Command", cmd])
                .spawn()
        } else {
            Command::new("wt.exe")
                .args(["-d", working_dir, "--", "cmd", "/K", cmd])
                .spawn()
        }
    } else {
        Command::new("wt.exe")
            .args(["-d", working_dir])
            .spawn()
    };

    if result.is_ok() {
        return Ok(());
    }
    launch_windows_cmd(working_dir, cmd)
}

#[cfg(target_os = "windows")]
fn launch_windows_pwsh(working_dir: &str, cmd: &str) -> Result<(), String> {
    // 如果有 wt.exe，用它打开 pwsh
    if which_exists("wt.exe") {
        let result = if !cmd.is_empty() {
            Command::new("wt.exe")
                .args(["-d", working_dir, "--", "pwsh.exe", "-NoExit", "-Command", cmd])
                .spawn()
        } else {
            Command::new("wt.exe")
                .args(["-d", working_dir, "--", "pwsh.exe"])
                .spawn()
        };
        if result.is_ok() {
            return Ok(());
        }
    }
    // 直接启动 pwsh.exe
    if !cmd.is_empty() {
        Command::new("pwsh.exe")
            .args(["-NoExit", "-Command", cmd])
            .current_dir(working_dir)
            .spawn()
    } else {
        Command::new("cmd.exe")
            .args(["/c", "start", "pwsh.exe", "-NoExit"])
            .current_dir(working_dir)
            .spawn()
    }
    .map(|_| ())
    .map_err(|e| format!("启动 PowerShell 7 失败: {}", e))
}

#[cfg(target_os = "windows")]
fn launch_windows_powershell(working_dir: &str, cmd: &str) -> Result<(), String> {
    if which_exists("wt.exe") {
        let args = if !cmd.is_empty() {
            vec!["-d", working_dir, "--", "powershell.exe", "-NoExit", "-Command", cmd]
        } else {
            vec!["-d", working_dir, "--", "powershell.exe"]
        };
        let result = Command::new("wt.exe")
            .args(&args)
            .spawn();
        if result.is_ok() {
            return Ok(());
        }
    }
    Command::new("cmd.exe")
        .args(["/c", "start", "powershell.exe", "-NoExit"])
        .current_dir(working_dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 Windows PowerShell 失败: {}", e))
}

#[cfg(target_os = "windows")]
fn launch_windows_git_bash(working_dir: &str, cmd: &str) -> Result<(), String> {
    // 查找 git bash 路径
    let bash_path = if std::path::Path::new(r"C:\Program Files\Git\bin\bash.exe").exists() {
        r"C:\Program Files\Git\bin\bash.exe".to_string()
    } else if std::path::Path::new(r"C:\Program Files (x86)\Git\bin\bash.exe").exists() {
        r"C:\Program Files (x86)\Git\bin\bash.exe".to_string()
    } else if let Ok(p) = which::which("bash.exe") {
        p.to_string_lossy().to_string()
    } else {
        return Err("Git Bash 未找到".to_string());
    };

    if which_exists("wt.exe") {
        let args = if !cmd.is_empty() {
            vec!["-d", working_dir, "--", &bash_path, "-c", cmd]
        } else {
            vec!["-d", working_dir, "--", &bash_path]
        };
        let result = Command::new("wt.exe")
            .args(&args)
            .spawn();
        if result.is_ok() {
            return Ok(());
        }
    }
    Command::new("cmd.exe")
        .args(["/c", "start", "", &bash_path])
        .current_dir(working_dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 Git Bash 失败: {}", e))
}

#[cfg(target_os = "windows")]
fn launch_windows_cmd(working_dir: &str, cmd: &str) -> Result<(), String> {
    if !cmd.is_empty() {
        Command::new("cmd.exe")
            .args(["/c", "start", "cmd.exe", "/K", cmd])
            .current_dir(working_dir)
            .spawn()
    } else {
        Command::new("cmd.exe")
            .args(["/c", "start", "cmd.exe"])
            .current_dir(working_dir)
            .spawn()
    }
    .map(|_| ())
    .map_err(|e| format!("启动 CMD 失败: {}", e))
}

// ==================== macOS Launchers ====================

#[cfg(target_os = "macos")]
fn launch_macos_terminal(working_dir: &str, cmd: &str) -> Result<(), String> {
    let escaped_dir = working_dir.replace('\'', "'\\''");
    let escaped_cmd = cmd.replace('\\', "\\\\").replace('"', "\\\"");
    let script = if !escaped_cmd.is_empty() {
        format!(
            "tell app \"Terminal\" to do script \"cd '{}' && {}\"",
            escaped_dir, escaped_cmd
        )
    } else {
        format!(
            "tell app \"Terminal\" to do script \"cd '{}'\"",
            escaped_dir
        )
    };
    Command::new("osascript")
        .args(["-e", &script])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 Terminal 失败: {}", e))
}

#[cfg(target_os = "macos")]
fn launch_macos_iterm(working_dir: &str, cmd: &str) -> Result<(), String> {
    let escaped_dir = working_dir.replace('\\', "\\\\").replace('"', "\\\"");
    let escaped_cmd = cmd.replace('\\', "\\\\").replace('"', "\\\"");

    let script = if !escaped_cmd.is_empty() {
        format!(
            r#"
            tell application "iTerm2"
                activate
                create window with default profile
                tell current session of current window
                    write text "cd '{}'"
                    write text "{}"
                end tell
            end tell
            "#,
            escaped_dir, escaped_cmd
        )
    } else {
        format!(
            r#"
            tell application "iTerm2"
                activate
                create window with default profile
                tell current session of current window
                    write text "cd '{}'"
                end tell
            end tell
            "#,
            escaped_dir
        )
    };
    Command::new("osascript")
        .args(["-e", &script])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 iTerm2 失败: {}", e))
}

#[cfg(target_os = "macos")]
fn launch_macos_warp(working_dir: &str, _cmd: &str) -> Result<(), String> {
    Command::new("open")
        .args(["-a", "Warp", working_dir])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 Warp 失败: {}", e))
}

#[cfg(target_os = "macos")]
fn launch_macos_alacritty(working_dir: &str, cmd: &str) -> Result<(), String> {
    Command::new("alacritty")
        .args(["--working-directory", working_dir])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 Alacritty 失败: {}", e))
}

#[cfg(target_os = "macos")]
fn launch_macos_kitty(working_dir: &str, cmd: &str) -> Result<(), String> {
    Command::new("kitty")
        .args(["--directory", working_dir])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 Kitty 失败: {}", e))
}

// ==================== Linux Launchers ====================

#[cfg(target_os = "linux")]
fn launch_linux_gnome(working_dir: &str, cmd: &str) -> Result<(), String> {
    if !cmd.is_empty() {
        Command::new("gnome-terminal")
            .args(["--working-directory", working_dir, "--", "bash", "-c", &format!("{}; exec $SHELL", cmd)])
            .spawn()
    } else {
        Command::new("gnome-terminal")
            .args(["--working-directory", working_dir])
            .spawn()
    }
    .map(|_| ())
    .map_err(|e| format!("启动 GNOME Terminal 失败: {}", e))
}

#[cfg(target_os = "linux")]
fn launch_linux_konsole(working_dir: &str, cmd: &str) -> Result<(), String> {
    if !cmd.is_empty() {
        Command::new("konsole")
            .args(["--workdir", working_dir, "-e", "bash", "-c", &format!("{}; exec $SHELL", cmd)])
            .spawn()
    } else {
        Command::new("konsole")
            .args(["--workdir", working_dir])
            .spawn()
    }
    .map(|_| ())
    .map_err(|e| format!("启动 Konsole 失败: {}", e))
}

#[cfg(target_os = "linux")]
fn launch_linux_alacritty(working_dir: &str, cmd: &str) -> Result<(), String> {
    Command::new("alacritty")
        .args(["--working-directory", working_dir])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 Alacritty 失败: {}", e))
}

#[cfg(target_os = "linux")]
fn launch_linux_kitty(working_dir: &str, cmd: &str) -> Result<(), String> {
    Command::new("kitty")
        .args(["--directory", working_dir])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 Kitty 失败: {}", e))
}

#[cfg(target_os = "linux")]
fn launch_linux_terminator(working_dir: &str, cmd: &str) -> Result<(), String> {
    Command::new("terminator")
        .args(["--working-directory", working_dir])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 Terminator 失败: {}", e))
}

#[cfg(target_os = "linux")]
fn launch_linux_auto(working_dir: &str, cmd: &str) -> Result<(), String> {
    let terminals = ["gnome-terminal", "konsole", "alacritty", "kitty", "terminator"];
    for term in &terminals {
        if which_exists(term) {
            return match *term {
                "gnome-terminal" => launch_linux_gnome(working_dir, cmd),
                "konsole" => launch_linux_konsole(working_dir, cmd),
                "alacritty" => launch_linux_alacritty(working_dir, cmd),
                "kitty" => launch_linux_kitty(working_dir, cmd),
                "terminator" => launch_linux_terminator(working_dir, cmd),
                _ => continue,
            };
        }
    }
    Err("未找到可用的终端模拟器".to_string())
}
