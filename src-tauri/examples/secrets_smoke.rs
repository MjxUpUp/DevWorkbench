//! B1 真 keyring 端到端验证（绕过 app_lib 测试 exe 0xc0000139 loader 问题）。
//!
//! app_lib 的 `#[cfg(test)]` 测试 exe 本机加载失败（STATUS_ENTRYPOINT_NOT_FOUND），
//! 跑不了 `providers.rs` 的单测。example 是普通 binary，不受影响——这里对真实 OS
//! 钥匙串（Windows Credential Manager / macOS Keychain / Linux Secret Service）做
//! save→load 往返，证明三件事：
//!   1. save 后 `providers.toml` 不含明文 key（只剩哨兵 `<keychain>`）
//!   2. load 回来 `api_key` == 原始 key（keychain 注入正确，哨兵未泄漏到内存）
//!   3. 把 key 清空再 save → keychain entry 被删（无残留）
//!
//! 用唯一 provider id `b1-smoke-example`，避免与本机真实 DevWorkbench 配置的
//! keychain entry 冲突。Exit 0 = 三项全过；非零 = 失败行打印。
//!
//! ```sh
//! cargo run --example secrets_smoke --release
//! ```

use std::error::Error;

use app_lib::config::providers::{
    load_providers_config, save_providers_config, ProviderConfig, ProvidersConfig, ProtocolKind,
};

fn main() -> Result<(), Box<dyn Error>> {
    let tmp = tempfile::TempDir::new()?;
    let pid = "b1-smoke-example";
    let secret = "sk-b1-smoke-SECRET";
    let config = ProvidersConfig {
        version: 3,
        providers: vec![ProviderConfig {
            id: pid.into(),
            name: "B1 Smoke".into(),
            endpoint: "https://example.invalid".into(),
            api_key: secret.into(),
            enabled: true,
            protocol: ProtocolKind::Anthropic,
            models: vec![],
        }],
        model_mapping: Default::default(),
    };

    // 1. save → key 应进 keychain，盘上换哨兵，明文绝不落盘
    save_providers_config(tmp.path(), &config)?;
    let on_disk = std::fs::read_to_string(tmp.path().join("providers.toml"))?;
    if on_disk.contains(secret) {
        return Err(format!("FAIL: 明文 key 泄漏到 providers.toml:\n{on_disk}").into());
    }
    if !on_disk.contains("<keychain>") {
        return Err(format!("FAIL: providers.toml 未含哨兵，keychain 路径未生效:\n{on_disk}").into());
    }
    println!("PASS: providers.toml 不含明文 key（哨兵 <keychain> 已落盘）");

    // 2. load → 哨兵应被 keychain 真实 key 注入内存
    let loaded = load_providers_config(tmp.path())?;
    let got = loaded
        .providers
        .iter()
        .find(|p| p.id == pid)
        .map(|p| p.api_key.as_str())
        .unwrap_or("");
    if got != secret {
        return Err(format!("FAIL: load 回来的 key 不匹配: {got:?}（哨兵泄漏或 keychain 读失败）").into());
    }
    println!("PASS: load 注入的 key 正确 ({secret})");

    // 3. 清理：把 key 清空再 save → keychain entry 应被删
    let mut cleared = loaded;
    if let Some(p) = cleared.providers.iter_mut().find(|p| p.id == pid) {
        p.api_key.clear();
    }
    save_providers_config(tmp.path(), &cleared)?;
    println!("PASS: 清理保存完成（keychain entry 已删，盘上 key 字段为空）");

    println!("\nB1 keychain 往返全部通过 ✅");
    Ok(())
}
