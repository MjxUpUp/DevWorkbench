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
    /// The model's context window in tokens (v2.0: drives context auto-compaction
    /// threshold). Optional — a model with no declared window falls back to a
    /// conservative default in `build_react_agent`. Declare it so the compactor
    /// sizes to the REAL window instead of assuming every model is small.
    #[serde(default)]
    pub context_window: Option<usize>,
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

/// A resolved provider ready to construct a `ChatModel`: the endpoint to hit,
/// the credential, and the concrete model id to request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProvider {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    /// The matched model's declared context window, if any (v2.0: drives
    /// auto-compaction sizing in `build_react_agent`).
    pub context_window: Option<usize>,
}

/// Resolve which provider + credentials serve a given model id.
///
/// Strategy:
/// 1. Honor `model_mapping` first (e.g. `"claude_opus"` → `"glm-4.6"`), so a
///    request for a Claude model transparently maps to its GLM stand-in.
/// 2. Then find the first **enabled** provider whose models contain the resolved
///    id AND whose `api_key` is non-empty (no key = not actually usable).
///
/// Returns `None` when nothing serves the request — the caller then falls back
/// to a default empty-key model (calls fail at request time, but construction
/// doesn't crash the whole graph run).
pub fn resolve_provider(config: &ProvidersConfig, model_id: &str) -> Option<ResolvedProvider> {
    let resolved_model = config
        .model_mapping
        .get(model_id)
        .map(|s| s.as_str())
        .unwrap_or(model_id);
    for p in &config.providers {
        if !p.enabled || p.api_key.is_empty() {
            continue;
        }
        if let Some(matched) = p
            .models
            .iter()
            .find(|m| m.id == resolved_model && m.enabled)
        {
            // Carry the matched model's declared window so the executor sizes
            // auto-compaction to the real model, not a hardcoded constant.
            return Some(ResolvedProvider {
                endpoint: p.endpoint.clone(),
                api_key: p.api_key.clone(),
                model: resolved_model.to_string(),
                context_window: matched.context_window,
            });
        }
    }
    None
}

/// Default providers configuration with preset entries.
///
/// NOTE on protocol: the kernel's only `ChatModel` impl (`GlmChatModel`) speaks
/// the **Anthropic Messages API** (`POST {base}/v1/messages`, `x-api-key`,
/// `anthropic-version`). So every preset endpoint MUST be an Anthropic-compatible
/// base (no trailing `/v1` — the impl appends it). Z.AI exposes such an endpoint;
/// Anthropic itself is the canonical one. OpenAI-compatible providers (DeepSeek,
/// OpenRouter, …) are intentionally omitted until a second ChatModel impl lands —
/// pre-shipping a provider the kernel can't call would mislead users.
fn default_providers_config() -> ProvidersConfig {
    let mut model_mapping = HashMap::new();
    model_mapping.insert("claude_opus".to_string(), "glm-4.6".to_string());
    model_mapping.insert("claude_sonnet".to_string(), "glm-4-flash".to_string());

    ProvidersConfig {
        providers: vec![
            ProviderConfig {
                id: "zai".to_string(),
                name: "Z.AI (GLM)".to_string(),
                endpoint: "https://open.bigmodel.cn/api/anthropic".to_string(),
                api_key: String::new(),
                enabled: true,
                models: vec![
                    ModelEntry { id: "glm-4.6".to_string(), label: "GLM-4.6".to_string(), enabled: true, context_window: Some(128_000) },
                    ModelEntry { id: "glm-4-flash".to_string(), label: "GLM-4 Flash".to_string(), enabled: true, context_window: Some(128_000) },
                ],
            },
            ProviderConfig {
                id: "anthropic".to_string(),
                name: "Anthropic".to_string(),
                endpoint: "https://api.anthropic.com".to_string(),
                api_key: String::new(),
                enabled: false,
                models: vec![
                    ModelEntry { id: "claude-opus-4-8".to_string(), label: "Claude Opus 4.8".to_string(), enabled: true, context_window: Some(200_000) },
                    ModelEntry { id: "claude-sonnet-4-6".to_string(), label: "Claude Sonnet 4.6".to_string(), enabled: true, context_window: Some(200_000) },
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
        assert_eq!(loaded.providers.len(), 2);
        assert_eq!(loaded.providers[0].id, "zai");
        assert!(loaded.model_mapping.contains_key("claude_opus"));
        // glm-4.6 is the flagship default the executor falls back to.
        assert_eq!(loaded.model_mapping.get("claude_opus").unwrap(), "glm-4.6");
    }

    #[test]
    fn resolve_provider_finds_enabled_with_key() {
        let mut config = default_providers_config();
        config.providers[0].api_key = "sk-real".into(); // zai enabled by default
        let r = resolve_provider(&config, "glm-4.6").expect("zai serves glm-4.6");
        assert_eq!(r.api_key, "sk-real");
        assert_eq!(r.model, "glm-4.6");
        assert!(r.endpoint.ends_with("/api/anthropic"));
        // v2.0: the preset GLM window carries through so the executor can size
        // auto-compaction to the real 128k, not a hardcoded constant.
        assert_eq!(r.context_window, Some(128_000));
    }

    #[test]
    fn resolve_provider_skips_empty_key() {
        let config = default_providers_config(); // no key anywhere
        assert!(resolve_provider(&config, "glm-4.6").is_none());
    }

    #[test]
    fn resolve_provider_honors_mapping() {
        let mut config = default_providers_config();
        config.providers[0].api_key = "sk-real".into();
        // claude_opus maps to glm-4.6 — must resolve to the GLM provider+model.
        let r = resolve_provider(&config, "claude_opus").expect("mapping resolves");
        assert_eq!(r.model, "glm-4.6");
        assert_eq!(r.api_key, "sk-real");
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
