use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Current providers.toml schema version. Bumped when a migration is added;
/// `load_providers_config` backfills + re-persists any config below this.
const PROVIDERS_CONFIG_VERSION: u32 = 2;

/// The wire protocol a provider's endpoint speaks. Drives which `ChatModel`
/// impl the executor constructs — Anthropic Messages API or OpenAI Chat
/// Completions API. User-selectable per provider: most vendors expose exactly
/// one protocol, but proxies/aggregators may expose either.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProtocolKind {
    Anthropic,
    OpenAI,
}

impl Default for ProtocolKind {
    fn default() -> Self {
        ProtocolKind::Anthropic
    }
}

/// A model's routing tier within its provider. `Strong` = the capable/expensive
/// model for hard reasoning steps; `Cheap` = the fast/cheap one for trivial
/// steps (tool results, short confirmations). A provider that declares BOTH a
/// Strong and a Cheap model gets the per-step strong/cheap router; one with
/// neither (or only one) stays single-model. This is the data-driven
/// replacement for the old `starts_with("glm-")` family guard.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
    Strong,
    Cheap,
}

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
    /// Wire protocol this endpoint speaks. Defaults to Anthropic (the
    /// historical only impl); set OpenAI for DeepSeek/OpenRouter/OpenAI-native.
    #[serde(default)]
    pub protocol: ProtocolKind,
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
    /// Routing tier: Strong (capable) or Cheap (fast). None = this model does
    /// not participate in per-step routing. A provider needs one Strong AND one
    /// Cheap to activate the router (data-driven version of the old GLM-family
    /// guard, which matched `starts_with("glm-")`).
    #[serde(default)]
    pub tier: Option<ModelTier>,
}

/// The full providers configuration file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersConfig {
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub model_mapping: HashMap<String, String>,
    /// Schema version. Older configs (no field → 0) are migrated up by
    /// `load_providers_config` and re-persisted.
    #[serde(default)]
    pub version: u32,
}

/// Load providers config from the app data directory.
pub fn load_providers_config(
    data_dir: &std::path::Path,
) -> Result<ProvidersConfig, crate::error::AppError> {
    let config_path = data_dir.join("providers.toml");

    if !config_path.exists() {
        // Create default config with preset providers
        let default_config = default_providers_config();
        save_providers_config(data_dir, &default_config)?;
        return Ok(default_config);
    }

    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| crate::error::AppError::Config(format!("读取 providers 配置失败: {}", e)))?;

    let mut config: ProvidersConfig = toml::from_str(&content)
        .map_err(|e| crate::error::AppError::Config(format!("解析 providers 配置失败: {}", e)))?;

    // v1→v2: backfill ModelTier on the preset providers' known models so
    // existing Z.AI/Anthropic users keep per-step strong/cheap routing —
    // previously hardcoded via `starts_with("glm-")`, now data-driven via
    // ModelTier. Only touches known preset model ids with no tier set;
    // user-customized tiers/models are left alone.
    if config.version < PROVIDERS_CONFIG_VERSION {
        let changed = migrate_legacy_tiers(&mut config);
        config.version = PROVIDERS_CONFIG_VERSION;
        if changed {
            // Best-effort persist of the migrated config; a write failure must
            // not block loading (the in-memory config is already migrated).
            let _ = save_providers_config(data_dir, &config);
        }
    }

    Ok(config)
}

/// v1→v2 migration: set Strong/Cheap tiers on preset model ids that lack one,
/// so existing users keep strong/cheap routing after the data-driven rewrite.
/// Returns true if any model was changed (caller re-persists).
fn migrate_legacy_tiers(config: &mut ProvidersConfig) -> bool {
    let mut changed = false;
    for p in &mut config.providers {
        for m in &mut p.models {
            if m.tier.is_none() {
                let tier = match m.id.as_str() {
                    "glm-4.6" | "claude-opus-4-8" | "deepseek-chat" => Some(ModelTier::Strong),
                    "glm-4-flash" | "claude-sonnet-4-6" => Some(ModelTier::Cheap),
                    _ => None,
                };
                if tier.is_some() {
                    m.tier = tier;
                    changed = true;
                }
            }
        }
    }
    changed
}

/// Save providers config to the app data directory.
pub fn save_providers_config(
    data_dir: &std::path::Path,
    config: &ProvidersConfig,
) -> Result<(), crate::error::AppError> {
    let config_path = data_dir.join("providers.toml");

    let content = toml::to_string_pretty(config)
        .map_err(|e| crate::error::AppError::Config(format!("序列化 providers 配置失败: {}", e)))?;

    std::fs::write(&config_path, content)
        .map_err(|e| crate::error::AppError::Config(format!("写入 providers 配置失败: {}", e)))?;

    Ok(())
}

/// A resolved provider ready to construct a `ChatModel`: the endpoint to hit,
/// the credential, the concrete model id to request, and the wire protocol that
/// selects which `ChatModel` impl to build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProvider {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    /// The matched model's declared context window, if any (v2.0: drives
    /// auto-compaction sizing in `build_react_agent`).
    pub context_window: Option<usize>,
    /// Wire protocol of the resolved endpoint — selects the ChatModel impl.
    pub protocol: ProtocolKind,
    /// The provider's Strong-tier model id, if it declared one. Drives the
    /// per-step router's "powerful step → strong model" branch.
    pub strong_model: Option<String>,
    /// The provider's Cheap-tier model id, if it declared one. Drives the
    /// per-step router's "trivial step → cheap model" branch.
    pub cheap_model: Option<String>,
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
    // '__default__' is the settings UI's "默认模型" alias — the key the user's
    // default-model dropdown writes into modelMapping. Honor an explicit mapping
    // first (the user pinned a default model); else fall back to the data-driven
    // default_model_id (first enabled Strong model). Any other id runs through
    // model_mapping as a plain alias (e.g. claude_opus → glm-4.6). This keeps the
    // default path free of any hardcoded vendor model id — the old
    // modelMapping["glm-4.6"] assumed the executor defaulted to glm-4.6, which
    // broke the moment a non-GLM provider was the first enabled Strong.
    let resolved_owned: String = if model_id == "__default__" {
        config
            .model_mapping
            .get("__default__")
            .cloned()
            .unwrap_or_else(|| default_model_id(config))
    } else {
        config
            .model_mapping
            .get(model_id)
            .cloned()
            .unwrap_or_else(|| model_id.to_string())
    };
    let resolved_model = resolved_owned.as_str();
    for p in &config.providers {
        if !p.enabled || p.api_key.is_empty() {
            continue;
        }
        if let Some(matched) = p
            .models
            .iter()
            .find(|m| m.id == resolved_model && m.enabled)
        {
            // Scan the matched provider's models for declared Strong/Cheap tiers
            // (first of each wins) so the per-step router wires data-driven,
            // with no "glm-" string matching anywhere.
            let strong_model = p
                .models
                .iter()
                .find(|m| m.tier == Some(ModelTier::Strong))
                .map(|m| m.id.clone());
            let cheap_model = p
                .models
                .iter()
                .find(|m| m.tier == Some(ModelTier::Cheap))
                .map(|m| m.id.clone());
            return Some(ResolvedProvider {
                endpoint: p.endpoint.clone(),
                api_key: p.api_key.clone(),
                model: resolved_model.to_string(),
                context_window: matched.context_window,
                protocol: p.protocol,
                strong_model,
                cheap_model,
            });
        }
    }
    None
}

/// The default model id to use when none is explicitly requested: the first
/// enabled Strong-tier model across all enabled+keyed providers, else the first
/// enabled model of any tier, else a hardcoded fallback string. This is the
/// data-driven replacement for the old `unwrap_or("glm-4.6")` (which hardcoded
/// one vendor's flagship into the executor).
pub fn default_model_id(config: &ProvidersConfig) -> String {
    let enabled: Vec<&ProviderConfig> = config
        .providers
        .iter()
        .filter(|p| p.enabled && !p.api_key.is_empty())
        .collect();
    // Prefer the first enabled Strong-tier model.
    for p in &enabled {
        for m in &p.models {
            if m.enabled && m.tier == Some(ModelTier::Strong) {
                return m.id.clone();
            }
        }
    }
    // Else the first enabled model of any tier.
    for p in &enabled {
        for m in &p.models {
            if m.enabled {
                return m.id.clone();
            }
        }
    }
    // Ultimate fallback: a concrete id. With no enabled provider nothing serves
    // it, so a request fails at call time — but construction never panics.
    "glm-4.6".to_string()
}

/// Default providers configuration with preset entries.
///
/// Each preset declares its wire `protocol` so the executor picks the right
/// `ChatModel` impl: Z.AI + Anthropic speak the Anthropic Messages API
/// (`POST {base}/v1/messages`); DeepSeek speaks OpenAI Chat Completions
/// (`POST {base}/v1/chat/completions`). Every preset endpoint is a base URL
/// with no trailing `/v1` — the impl appends the version path. DeepSeek ships
/// `enabled: false` (no key by default); flip it on + add a key to use it.
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
                protocol: ProtocolKind::Anthropic,
                models: vec![
                    ModelEntry {
                        id: "glm-4.6".to_string(),
                        label: "GLM-4.6".to_string(),
                        enabled: true,
                        context_window: Some(128_000),
                        tier: Some(ModelTier::Strong),
                    },
                    ModelEntry {
                        id: "glm-4-flash".to_string(),
                        label: "GLM-4 Flash".to_string(),
                        enabled: true,
                        context_window: Some(128_000),
                        tier: Some(ModelTier::Cheap),
                    },
                ],
            },
            ProviderConfig {
                id: "anthropic".to_string(),
                name: "Anthropic".to_string(),
                endpoint: "https://api.anthropic.com".to_string(),
                api_key: String::new(),
                enabled: false,
                protocol: ProtocolKind::Anthropic,
                models: vec![
                    ModelEntry {
                        id: "claude-opus-4-8".to_string(),
                        label: "Claude Opus 4.8".to_string(),
                        enabled: true,
                        context_window: Some(200_000),
                        tier: Some(ModelTier::Strong),
                    },
                    ModelEntry {
                        id: "claude-sonnet-4-6".to_string(),
                        label: "Claude Sonnet 4.6".to_string(),
                        enabled: true,
                        context_window: Some(200_000),
                        tier: Some(ModelTier::Cheap),
                    },
                ],
            },
            ProviderConfig {
                id: "deepseek".to_string(),
                name: "DeepSeek".to_string(),
                endpoint: "https://api.deepseek.com".to_string(),
                api_key: String::new(),
                enabled: false,
                protocol: ProtocolKind::OpenAI,
                models: vec![ModelEntry {
                    id: "deepseek-chat".to_string(),
                    label: "DeepSeek Chat".to_string(),
                    enabled: true,
                    context_window: Some(64_000),
                    tier: Some(ModelTier::Strong),
                }],
            },
        ],
        model_mapping,
        version: PROVIDERS_CONFIG_VERSION,
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
        // zai + anthropic + deepseek presets.
        assert_eq!(loaded.providers.len(), 3);
        assert_eq!(loaded.providers[0].id, "zai");
        assert_eq!(loaded.providers[2].id, "deepseek");
        // DeepSeek ships as OpenAI protocol (the OpenAI ChatModel impl's example).
        assert_eq!(loaded.providers[2].protocol, ProtocolKind::OpenAI);
        // Z.AI speaks Anthropic protocol explicitly now (was implicit before).
        assert_eq!(loaded.providers[0].protocol, ProtocolKind::Anthropic);
        assert!(loaded.model_mapping.contains_key("claude_opus"));
        assert_eq!(loaded.model_mapping.get("claude_opus").unwrap(), "glm-4.6");
    }

    #[test]
    fn zai_models_carry_strong_cheap_tiers() {
        let config = default_providers_config();
        let zai = &config.providers[0];
        let glm46 = zai.models.iter().find(|m| m.id == "glm-4.6").unwrap();
        let flash = zai.models.iter().find(|m| m.id == "glm-4-flash").unwrap();
        assert_eq!(glm46.tier, Some(ModelTier::Strong));
        assert_eq!(flash.tier, Some(ModelTier::Cheap));
    }

    #[test]
    fn resolve_provider_finds_enabled_with_key() {
        let mut config = default_providers_config();
        config.providers[0].api_key = "sk-real".into(); // zai enabled by default
        let r = resolve_provider(&config, "glm-4.6").expect("zai serves glm-4.6");
        assert_eq!(r.api_key, "sk-real");
        assert_eq!(r.model, "glm-4.6");
        assert_eq!(r.protocol, ProtocolKind::Anthropic);
        assert!(r.endpoint.ends_with("/api/anthropic"));
        assert_eq!(r.context_window, Some(128_000));
        // Data-driven tiers carry through so the router wires without "glm-" matching.
        assert_eq!(r.strong_model.as_deref(), Some("glm-4.6"));
        assert_eq!(r.cheap_model.as_deref(), Some("glm-4-flash"));
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
    fn resolve_provider_deepseek_uses_openai_protocol() {
        let mut config = default_providers_config();
        let ds = config
            .providers
            .iter_mut()
            .find(|p| p.id == "deepseek")
            .unwrap();
        ds.api_key = "sk-ds".into();
        ds.enabled = true;
        let r = resolve_provider(&config, "deepseek-chat").expect("deepseek serves deepseek-chat");
        assert_eq!(r.protocol, ProtocolKind::OpenAI);
        assert_eq!(r.model, "deepseek-chat");
    }

    #[test]
    fn default_model_id_prefers_first_enabled_strong() {
        let mut config = default_providers_config();
        config.providers[0].api_key = "sk-real".into(); // zai enabled
        // First enabled Strong across providers = glm-4.6 (zai is first).
        assert_eq!(default_model_id(&config), "glm-4.6");
    }

    #[test]
    fn default_model_id_falls_back_to_any_enabled_model() {
        // A provider whose only enabled model is Cheap (no Strong declared) —
        // default should still return it via the "any enabled model" fallback,
        // not panic and not return the hardcoded fallback string.
        let config = ProvidersConfig {
            version: PROVIDERS_CONFIG_VERSION,
            providers: vec![ProviderConfig {
                id: "cheap-only".to_string(),
                name: "Cheap Only".to_string(),
                endpoint: "https://example.com".to_string(),
                api_key: "sk-x".to_string(),
                enabled: true,
                protocol: ProtocolKind::OpenAI,
                models: vec![ModelEntry {
                    id: "cheap-bot".to_string(),
                    label: "Cheap".to_string(),
                    enabled: true,
                    context_window: None,
                    tier: Some(ModelTier::Cheap),
                }],
            }],
            model_mapping: HashMap::new(),
        };
        assert_eq!(default_model_id(&config), "cheap-bot");
    }

    #[test]
    fn default_model_id_ultimate_fallback_when_nothing_enabled() {
        let config = default_providers_config(); // all empty key → nothing enabled
        assert_eq!(default_model_id(&config), "glm-4.6");
    }

    #[test]
    fn migrate_legacy_tiers_backfills_presets() {
        // Simulate a v1 config: tiers absent on preset models.
        let mut config = ProvidersConfig {
            version: 0,
            providers: vec![ProviderConfig {
                id: "zai".to_string(),
                name: "Z.AI".to_string(),
                endpoint: "https://open.bigmodel.cn/api/anthropic".to_string(),
                api_key: "sk-x".to_string(),
                enabled: true,
                protocol: ProtocolKind::Anthropic,
                models: vec![
                    ModelEntry {
                        id: "glm-4.6".to_string(),
                        label: "GLM-4.6".to_string(),
                        enabled: true,
                        context_window: Some(128_000),
                        tier: None,
                    },
                    ModelEntry {
                        id: "glm-4-flash".to_string(),
                        label: "Flash".to_string(),
                        enabled: true,
                        context_window: Some(128_000),
                        tier: None,
                    },
                ],
            }],
            model_mapping: HashMap::new(),
        };
        let changed = migrate_legacy_tiers(&mut config);
        assert!(changed);
        let zai = &config.providers[0];
        assert_eq!(
            zai.models.iter().find(|m| m.id == "glm-4.6").unwrap().tier,
            Some(ModelTier::Strong)
        );
        assert_eq!(
            zai.models
                .iter()
                .find(|m| m.id == "glm-4-flash")
                .unwrap()
                .tier,
            Some(ModelTier::Cheap)
        );
    }

    #[test]
    fn test_load_creates_default_if_missing() {
        let tmp = TempDir::new().unwrap();
        let config = load_providers_config(tmp.path()).unwrap();
        assert!(!config.providers.is_empty());
        // Config file should now exist
        assert!(tmp.path().join("providers.toml").exists());
    }

    #[test]
    fn resolve_default_alias_honors_explicit_mapping() {
        // The settings UI writes the user's "默认模型" pick into
        // modelMapping['__default__']. resolve_provider must honor it.
        let mut config = default_providers_config();
        config.providers[0].api_key = "sk-real".into(); // zai enabled
        config
            .model_mapping
            .insert("__default__".to_string(), "glm-4-flash".to_string());
        let r = resolve_provider(&config, "__default__").expect("default alias resolves");
        assert_eq!(r.model, "glm-4-flash");
    }

    #[test]
    fn resolve_default_alias_falls_back_to_first_strong() {
        // No explicit mapping → data-driven default (first enabled Strong model).
        // glm-4.6 is zai's Strong tier and zai is the first enabled provider.
        let mut config = default_providers_config();
        config.providers[0].api_key = "sk-real".into(); // zai enabled
        let r = resolve_provider(&config, "__default__").expect("default alias resolves");
        assert_eq!(r.model, "glm-4.6");
    }

    #[test]
    fn resolve_default_alias_none_enabled_is_none() {
        // Nothing enabled/keyed → '__default__' resolves to the ultimate fallback
        // id, which no provider serves → None (construction-safe, fails at call).
        let config = default_providers_config(); // all empty key
        assert!(resolve_provider(&config, "__default__").is_none());
    }
}
