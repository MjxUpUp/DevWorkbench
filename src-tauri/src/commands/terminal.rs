use std::process::Command;

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

#[tauri::command]
pub fn open_terminal(working_dir: String, command: Option<String>) -> Result<(), String> {
    let dir = std::path::Path::new(&working_dir);
    if !dir.exists() {
        return Err(format!("目录不存在: {}", working_dir));
    }

    let cmd = command.unwrap_or_default();
    validate_command(&cmd)?;

    #[cfg(target_os = "windows")]
    {
        // 优先尝试 Windows Terminal
        let result = if !cmd.is_empty() {
            Command::new("wt.exe")
                .args(["-d", &working_dir, "cmd", "/K", &cmd])
                .spawn()
        } else {
            Command::new("wt.exe")
                .args(["-d", &working_dir])
                .spawn()
        };

        if result.is_ok() {
            return Ok(());
        }

        // 回退到 cmd.exe
        if !cmd.is_empty() {
            Command::new("cmd.exe")
                .args(["/c", "start", "cmd.exe", "/K", &cmd])
                .current_dir(&working_dir)
                .spawn()
        } else {
            Command::new("cmd.exe")
                .args(["/c", "start", "cmd.exe"])
                .current_dir(&working_dir)
                .spawn()
        }
        .map(|_| ())
        .map_err(|e| format!("启动终端失败: {}", e))
    }

    #[cfg(target_os = "macos")]
    {
        // 转义路径中的单引号：' → '\'' (结束当前引号、插入转义单引号、重新开始引号)
        let escaped_dir = working_dir.replace('\'', "'\\''");
        // 转义命令中的双引号和反斜杠，防止 AppleScript 注入
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
            .map_err(|e| format!("启动终端失败: {}", e))
    }

    #[cfg(target_os = "linux")]
    {
        let result = if !cmd.is_empty() {
            Command::new("gnome-terminal")
                .args([
                    "--working-directory",
                    &working_dir,
                    "--",
                    "bash",
                    "-c",
                    &format!("{}; exec $SHELL", cmd),
                ])
                .spawn()
                .or_else(|_| {
                    Command::new("konsole")
                        .args([
                            "--workdir",
                            &working_dir,
                            "-e",
                            "bash",
                            "-c",
                            &format!("{}; exec $SHELL", cmd),
                        ])
                        .spawn()
                })
        } else {
            Command::new("gnome-terminal")
                .args(["--working-directory", &working_dir])
                .spawn()
                .or_else(|_| {
                    Command::new("konsole")
                        .args(["--workdir", &working_dir])
                        .spawn()
                })
        };

        result
            .map(|_| ())
            .map_err(|e| format!("启动终端失败: {}", e))
    }
}
