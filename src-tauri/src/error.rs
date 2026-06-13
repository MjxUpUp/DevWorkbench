use serde::Serialize;

/// Unified error type for DevWorkbench.
/// All Tauri commands return `Result<T, AppError>` instead of `Result<T, String>`.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("数据库错误: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("JSON 序列化错误: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML 解析错误: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("配置错误: {0}")]
    Config(String),

    #[error("未找到: {0}")]
    NotFound(String),

    #[error("Agent 错误: {0}")]
    Agent(String),

    #[error("知识采集失败 ({agent}): {reason}")]
    KnowledgeCollection { agent: String, reason: String },

    #[error("配置转写失败 ({agent}): 目标文件 {path} 无法写入")]
    ConfigWriteFailed { agent: String, path: String },

    #[error("Forge CLI 未安装")]
    ForgeNotInstalled,

    #[error("MCP error: {0}")]
    Mcp(String),

    #[error("Skill error: {0}")]
    Skill(String),

    #[error("Cost error: {0}")]
    Cost(String),
}

// Tauri commands require errors to implement Serialize.
// We serialize the display string so the frontend receives a human-readable message.
impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

// Tauri 2's #[tauri::command] handles Result<T, E> where E: Serialize.
// No manual Into<InvokeError> needed — the Serialize impl above is sufficient.
