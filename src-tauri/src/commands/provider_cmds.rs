//! Providers config management commands — read/write the GLOBAL providers.toml
//! (lives in the app data dir, not per-project) + test that a provider's
//! credentials actually work before the user trusts an agent run to it.

use serde::Serialize;

use crate::commands::projects::dirs_home;
use crate::config::providers::{
    load_providers_config, save_providers_config, ProvidersConfig,
};
use crate::error::AppError;

/// The app data dir holding providers.toml (same place as data.db / settings).
fn data_dir() -> std::path::PathBuf {
    dirs_home().join(".dev-workbench")
}

/// Read the global providers config. Creates the default preset file on first
/// call (load_providers_config handles the absent-file case).
#[tauri::command]
pub fn get_providers_config() -> Result<ProvidersConfig, AppError> {
    load_providers_config(&data_dir())
}

/// Persist the full providers config (whole-file replace — the frontend sends
/// the entire edited structure back).
#[tauri::command]
pub fn set_providers_config(config: ProvidersConfig) -> Result<(), AppError> {
    save_providers_config(&data_dir(), &config)
}

/// Result of probing one provider's credentials with a 1-token ping.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTestResult {
    pub ok: bool,
    pub status: u16,
    pub message: String,
}

/// Verify endpoint + api_key + model by sending a minimal probe request in the
/// provider's wire `protocol`: `"openai"` → OpenAI Chat Completions
/// (`/v1/chat/completions`, `Authorization: Bearer`); anything else (incl.
/// `None`) → Anthropic Messages (`/v1/messages`, `x-api-key`). `endpoint` is the
/// base with no trailing `/v1`; the probe appends the protocol's version path.
/// Returns a structured result (never errors — network/auth failures come back
/// as `ok:false` with a message, so the UI can show them instead of a toast
/// panic).
#[tauri::command]
pub async fn test_provider_connection(
    endpoint: String,
    api_key: String,
    model: String,
    protocol: Option<String>,
) -> Result<ProviderTestResult, AppError> {
    if api_key.is_empty() {
        return Ok(ProviderTestResult {
            ok: false,
            status: 0,
            message: "API Key 为空".into(),
        });
    }
    let is_openai = protocol
        .as_deref()
        .map(|p| p.eq_ignore_ascii_case("openai"))
        .unwrap_or(false);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| AppError::Config(format!("构建 HTTP 客户端失败: {e}")))?;
    // Both protocols accept the same minimal ping body ({model,messages,
    // max_tokens}); only the URL + auth header differ.
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 1,
        "messages": [{"role":"user","content":"ping"}],
    });
    let resp = if is_openai {
        client
            .post(crate::kernel_impl::openai_chat_model::OpenAIChatModel::chat_completions_url(
                &endpoint,
            ))
            .header("Authorization", format!("Bearer {api_key}"))
            .json(&body)
            .send()
            .await
    } else {
        client
            .post(format!("{}/v1/messages", endpoint.trim_end_matches('/')))
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
    };
    match resp {
        Ok(r) => {
            let status = r.status().as_u16();
            if r.status().is_success() {
                Ok(ProviderTestResult {
                    ok: true,
                    status,
                    message: "连接成功".into(),
                })
            } else {
                let body_text = r.text().await.unwrap_or_default();
                Ok(ProviderTestResult {
                    ok: false,
                    status,
                    message: format!(
                        "HTTP {status}: {}",
                        body_text.chars().take(200).collect::<String>()
                    ),
                })
            }
        }
        Err(e) => Ok(ProviderTestResult {
            ok: false,
            status: 0,
            message: format!("请求失败: {e}"),
        }),
    }
}
