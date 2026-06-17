use serde::Serialize;

/// Unified error type for DevWorkbench.
/// All Tauri commands return `Result<T, AppError>` instead of `Result<T, String>`,
/// preserving structured error context for the frontend.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Agent error: {0}")]
    Agent(String),

    #[error("Knowledge collection failed ({agent}): {reason}")]
    KnowledgeCollection { agent: String, reason: String },

    #[error("Config write failed ({agent}): target file {path} not writable")]
    ConfigWriteFailed { agent: String, path: String },

    #[error("Forge CLI not installed")]
    ForgeNotInstalled,

    #[error("MCP error: {0}")]
    Mcp(String),

    #[error("Skill error: {0}")]
    Skill(String),

    #[error("Cost error: {0}")]
    Cost(String),

    /// Pool / internal string error (e.g. connection pool exhausted). Kept as a
    /// catch-all for the String-returning internal APIs being migrated.
    #[error("{0}")]
    Internal(String),
}

/// Allow `?` propagation from String-returning APIs (db pool, legacy spawn).
impl From<String> for AppError {
    fn from(s: String) -> Self {
        AppError::Internal(s)
    }
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
