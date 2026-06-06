use std::process::Command;

#[tauri::command]
pub fn open_terminal(working_dir: String, command: Option<String>) -> Result<(), String> {
    let dir = std::path::Path::new(&working_dir);
    if !dir.exists() {
        return Err(format!("目录不存在: {}", working_dir));
    }

    let cmd = command.unwrap_or_default();

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
        let script = if !cmd.is_empty() {
            format!("tell app \"Terminal\" to do script \"cd '{}' && {}\"", working_dir, cmd)
        } else {
            format!("tell app \"Terminal\" to do script \"cd '{}'\"", working_dir)
        };
        Command::new("osascript")
            .args(["-e", &script])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("启动终端失败: {}", e))
    }

    #[cfg(target_os = "linux")]
    {
        let working_dir_str = working_dir.clone();
        let result = if !cmd.is_empty() {
            Command::new("gnome-terminal")
                .args(["--working-directory", &working_dir, "--", "bash", "-c", &format!("{}; exec $SHELL", cmd)])
                .spawn()
                .or_else(|_| {
                    Command::new("konsole")
                        .args(["--workdir", &working_dir, "-e", "bash", "-c", &format!("{}; exec $SHELL", cmd)])
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
