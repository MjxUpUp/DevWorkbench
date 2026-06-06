use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub fn open_in_finder(app: AppHandle, path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(format!("路径不存在: {}", path));
    }

    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|e| format!("打开文件管理器失败: {}", e))
}
