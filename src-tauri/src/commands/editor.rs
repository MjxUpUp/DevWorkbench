use std::process::Command;

/// 已知编辑器白名单，只允许启动这些可执行文件
const ALLOWED_EDITORS: &[&str] = &[
    "code",       // VS Code
    "cursor",     // Cursor
    "windsurf",   // Windsurf
    "zed",        // Zed
    "subl",       // Sublime Text
    "vim",        // Vim
    "nvim",       // Neovim
    "emacs",      // Emacs
    "idea",       // IntelliJ IDEA
    "webstorm",   // WebStorm
    "clion",      // CLion
    "goland",     // GoLand
    "pycharm",    // PyCharm
    "rustrover",  // RustRover
];

/// 校验编辑器名称是否在白名单中
fn is_allowed_editor(editor: &str) -> bool {
    // 提取可执行文件名（处理绝对路径和相对路径）
    let name = std::path::Path::new(editor)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(editor);

    ALLOWED_EDITORS.iter().any(|&allowed| {
        // 精确匹配或处理 .exe 后缀
        name.eq_ignore_ascii_case(allowed)
            || name.eq_ignore_ascii_case(&format!("{}.exe", allowed))
    })
}

#[tauri::command]
pub fn open_in_editor(editor: String, project_path: String) -> Result<(), String> {
    let path = std::path::Path::new(&project_path);
    if !path.exists() {
        return Err(format!("目录不存在: {}", project_path));
    }

    if !is_allowed_editor(&editor) {
        return Err(format!(
            "不允许的编辑器: '{}'。支持的编辑器: {}",
            editor,
            ALLOWED_EDITORS.join(", ")
        ));
    }

    Command::new(&editor)
        .arg(&project_path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 {} 失败: {}", editor, e))
}
