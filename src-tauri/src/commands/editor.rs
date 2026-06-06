use std::process::Command;

#[tauri::command]
pub fn open_in_editor(editor: String, project_path: String) -> Result<(), String> {
    let path = std::path::Path::new(&project_path);
    if !path.exists() {
        return Err(format!("目录不存在: {}", project_path));
    }

    // 优先使用自定义路径，否则用 editor 名称（依赖 PATH）
    let editor_cmd = if std::path::Path::new(&editor).is_absolute() {
        editor.clone()
    } else {
        editor
    };

    Command::new(editor_cmd)
        .arg(&project_path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 {} 失败: {}", &project_path, e))
}
