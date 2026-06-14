use crate::db::DbState;
use std::process::Command;
use tauri::State;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// CREATE_NO_WINDOW — 阻止子进程创建控制台窗口
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

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
    "pi",         // Pi Coding Agent
    "codex",      // OpenAI Codex
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

/// 解析编辑器的实际可执行文件路径。
/// 优先级：1. 用户自定义路径  2. which 查找  3. 原始命令名
fn resolve_editor_path(editor: &str, custom_path: Option<&str>) -> String {
    // 1. 用户自定义路径
    if let Some(path) = custom_path {
        if !path.is_empty() {
            return path.to_string();
        }
    }

    // 2. which 查找完整路径（GUI 应用 PATH 可能不完整）
    if let Ok(resolved) = which::which(editor) {
        return resolved.to_string_lossy().to_string();
    }

    // 3. 回退到原始命令名
    editor.to_string()
}

#[tauri::command]
pub fn open_in_editor(editor: String, project_path: String, db: State<'_, DbState>) -> Result<(), String> {
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

    // 优先读取用户设置中的自定义路径
    let conn = db.get().map_err(|e| e.to_string())?;
    let custom_path = crate::commands::projects::load_settings_from_db(&conn)
        .ok()
        .and_then(|s| s.tool_paths.get(&editor).cloned())
        .filter(|p| !p.is_empty());
    drop(conn);

    let resolved = resolve_editor_path(&editor, custom_path.as_deref());

    let mut cmd = Command::new(&resolved);
    cmd.arg(&project_path);

    // Windows: 阻止子进程弹出控制台窗口
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 {} ({}) 失败: {}", editor, resolved, e))
}
