use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single model provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
}

/// A model entry within a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub enabled: bool,
}

/// The full providers configuration file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersConfig {
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub model_mapping: HashMap<String, String>,
}

/// Load providers config from the app data directory.
pub fn load_providers_config(data_dir: &std::path::Path) -> Result<ProvidersConfig, crate::error::AppError> {
    let config_path = data_dir.join("providers.toml");

    if !config_path.exists() {
        // Create default config with preset providers
        let default_config = default_providers_config();
        save_providers_config(data_dir, &default_config)?;
        return Ok(default_config);
    }

    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| crate::error::AppError::Config(format!("读取 providers 配置失败: {}", e)))?;

    let config: ProvidersConfig = toml::from_str(&content)
        .map_err(|e| crate::error::AppError::Config(format!("解析 providers 配置失败: {}", e)))?;

    Ok(config)
}

/// Save providers config to the app data directory.
pub fn save_providers_config(data_dir: &std::path::Path, config: &ProvidersConfig) -> Result<(), crate::error::AppError> {
    let config_path = data_dir.join("providers.toml");

    let content = toml::to_string_pretty(config)
        .map_err(|e| crate::error::AppError::Config(format!("序列化 providers 配置失败: {}", e)))?;

    std::fs::write(&config_path, content)
        .map_err(|e| crate::error::AppError::Config(format!("写入 providers 配置失败: {}", e)))?;

    Ok(())
}

/// Default providers configuration with preset entries.
fn default_providers_config() -> ProvidersConfig {
    let mut model_mapping = HashMap::new();
    model_mapping.insert("claude_opus".to_string(), "glm-5.1".to_string());
    model_mapping.insert("claude_sonnet".to_string(), "glm-4-flash".to_string());

    ProvidersConfig {
        providers: vec![
            ProviderConfig {
                id: "zai".to_string(),
                name: "Z.AI (GLM)".to_string(),
                endpoint: "https://open.bigmodel.cn/api/paas/v4".to_string(),
                api_key: String::new(),
                enabled: true,
                models: vec![
                    ModelEntry { id: "glm-5.1".to_string(), label: "GLM-5.1".to_string(), enabled: true },
                    ModelEntry { id: "glm-4-flash".to_string(), label: "GLM-4 Flash".to_string(), enabled: true },
                ],
            },
            ProviderConfig {
                id: "anthropic".to_string(),
                name: "Anthropic".to_string(),
                endpoint: "https://api.anthropic.com/v1".to_string(),
                api_key: String::new(),
                enabled: false,
                models: vec![
                    ModelEntry { id: "claude-opus-4-8".to_string(), label: "Claude Opus 4.8".to_string(), enabled: true },
                    ModelEntry { id: "claude-sonnet-4-6".to_string(), label: "Claude Sonnet 4.6".to_string(), enabled: true },
                ],
            },
            ProviderConfig {
                id: "deepseek".to_string(),
                name: "DeepSeek".to_string(),
                endpoint: "https://api.deepseek.com/v1".to_string(),
                api_key: String::new(),
                enabled: false,
                models: vec![
                    ModelEntry { id: "deepseek-chat".to_string(), label: "DeepSeek Chat".to_string(), enabled: true },
                    ModelEntry { id: "deepseek-reasoner".to_string(), label: "DeepSeek Reasoner".to_string(), enabled: true },
                ],
            },
        ],
        model_mapping,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_config_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let config = default_providers_config();
        save_providers_config(tmp.path(), &config).unwrap();
        let loaded = load_providers_config(tmp.path()).unwrap();
        assert_eq!(loaded.providers.len(), 3);
        assert_eq!(loaded.providers[0].id, "zai");
        assert!(loaded.model_mapping.contains_key("claude_opus"));
    }

    #[test]
    fn test_load_creates_default_if_missing() {
        let tmp = TempDir::new().unwrap();
        let config = load_providers_config(tmp.path()).unwrap();
        assert!(!config.providers.is_empty());
        // Config file should now exist
        assert!(tmp.path().join("providers.toml").exists());
    }
}
