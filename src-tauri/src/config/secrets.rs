//! API Key 的 OS 钥匙串存储（keyring v1 API）+ 明文 fallback。B1。
//!
//! 业界对齐（Claude Code / Cursor / Cline / Continue 均如此）：API Key 不落明文
//! 配置文件，存入 OS 原生安全存储——macOS Keychain / Windows Credential Manager /
//! Linux Secret Service（keyring crate 的 `v1` feature 默认链三平台 native store）。
//! `providers.toml` 里仅留哨兵 `<keychain>` 表示"key 在钥匙串"。
//!
//! keychain 不可用时（Linux 无 secret-service 守护进程）自动 fallback 明文——与
//! Cline 的 SecretStorage 不可用时行为一致。
//!
//! 安全边界（诚实告知）：OS 钥匙串绑定当前用户（DPAPI / Keychain 都以用户身份
//! 解密），拿到该用户权限的攻击者仍可解密。本方案防的是"配置文件被拷走 / 备份 /
//! 误提交导致的明文泄露"——这正是 `providers.toml` 此前明文存盘的真实风险面，与
//! 业界桌面 agent 的防护层级一致；不是防御已获本机用户权限的攻击者。
//!
//! 验证：纯逻辑（哨兵替换 / fallback 决策）走 `#[cfg(test)]` 单测（MemoryStore，
//! 不碰真实 OS 钥匙串）；真 keyring 往返（set→get→delete + 盘上不含明文）走
//! `examples/secrets_smoke.rs`——app_lib 测试 exe 本机 0xc0000139 加载失败，example
//! 是普通 binary 绕过该 loader 问题，对真实 Credential Manager 做端到端验证。

use crate::config::providers::ProvidersConfig;
use crate::error::AppError;

/// `providers.toml` 里标记"key 在 OS 钥匙串"的哨兵。hydrate 时必须替换成真实 key
/// 或空——绝不能让哨兵泄漏到内存逻辑（`resolve_provider` 的 `is_empty()` 会对非空
/// 哨兵误判为"有 key"，把无 key provider 当可用）。
pub(crate) const KEYCHAIN_SENTINEL: &str = "<keychain>";

/// 钥匙串 service 名（keyring `Entry::new` 第一参数）。一家 app 一个命名空间。
/// 仅生产 KeyringStore 用；test target（MemoryStore）不引用 → cfg(not(test)) 避免
/// lib-test 的 dead_code warning。
#[cfg(not(test))]
const SERVICE: &str = "DevWorkbench";

/// 钥匙串 entry 的 username，按 provider id 区分（一家 provider 一个 entry）。
fn entry_username(provider_id: &str) -> String {
    format!("provider:{provider_id}:api_key")
}

/// 抽象钥匙串后端。生产用 keyring（OS 原生 store），测试注入 MemoryStore——避免
/// 单测真的写 Windows Credential Manager / Keychain 污染本机。
pub(crate) trait SecretStore {
    fn store(&self, provider_id: &str, secret: &str) -> Result<(), AppError>;
    fn load(&self, provider_id: &str) -> Result<Option<String>, AppError>;
    fn delete(&self, provider_id: &str) -> Result<(), AppError>;
}

/// 生产后端：keyring v1 API（`Entry::new` / `set_password` / `get_password` /
/// `delete_credential`——4.x 从 `delete_password` 重命名）。无状态：每次 `Entry::new`
/// 直接操作 OS 存储，所以 load / save 用不同实例仍读写同一份 OS 凭据。
#[cfg(not(test))]
struct KeyringStore;

#[cfg(not(test))]
impl SecretStore for KeyringStore {
    fn store(&self, provider_id: &str, secret: &str) -> Result<(), AppError> {
        let entry = keyring::Entry::new(SERVICE, &entry_username(provider_id))
            .map_err(|e| AppError::Config(format!("创建钥匙串 entry 失败: {e}")))?;
        entry
            .set_password(secret)
            .map_err(|e| AppError::Config(format!("写入钥匙串失败: {e}")))
    }
    fn load(&self, provider_id: &str) -> Result<Option<String>, AppError> {
        let entry = keyring::Entry::new(SERVICE, &entry_username(provider_id))
            .map_err(|e| AppError::Config(format!("创建钥匙串 entry 失败: {e}")))?;
        match entry.get_password() {
            Ok(s) => Ok(Some(s)),
            // entry 不存在（NoEntry）/ 后端不可达（NoBackendAccess，如 Linux 无
            // secret-service）→ 视为无 key；上层 fallback 明文或当空处理。
            Err(_) => Ok(None),
        }
    }
    fn delete(&self, provider_id: &str) -> Result<(), AppError> {
        let entry = keyring::Entry::new(SERVICE, &entry_username(provider_id))
            .map_err(|e| AppError::Config(format!("创建钥匙串 entry 失败: {e}")))?;
        // entry 不存在时 delete 也报错；best-effort，统一忽略。
        let _ = entry.delete_credential();
        Ok(())
    }
}

/// 测试后端：进程内 HashMap。生产 KeyringStore 靠 OS 存储持久，测试靠进程内单例
/// （`active_store` 返回 `&'static` 同一实例）保证 save→load 跨调用持久。
#[cfg(test)]
#[derive(Default)]
pub(crate) struct MemoryStore(std::sync::Mutex<std::collections::HashMap<String, String>>);

#[cfg(test)]
impl SecretStore for MemoryStore {
    fn store(&self, provider_id: &str, secret: &str) -> Result<(), AppError> {
        self.0
            .lock()
            .unwrap()
            .insert(entry_username(provider_id), secret.into());
        Ok(())
    }
    fn load(&self, provider_id: &str) -> Result<Option<String>, AppError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(&entry_username(provider_id))
            .cloned())
    }
    fn delete(&self, provider_id: &str) -> Result<(), AppError> {
        self.0.lock().unwrap().remove(&entry_username(provider_id));
        Ok(())
    }
}

/// 取当前后端。生产固定 `KeyringStore`；测试固定 `MemoryStore`（全局单例，跨调用
/// 持久）。返回 `&'static dyn` 以便 load / save 共享同一实例。
fn active_store() -> &'static dyn SecretStore {
    #[cfg(not(test))]
    {
        static STORE: std::sync::OnceLock<KeyringStore> = std::sync::OnceLock::new();
        STORE.get_or_init(|| KeyringStore)
    }
    #[cfg(test)]
    {
        static STORE: std::sync::OnceLock<MemoryStore> = std::sync::OnceLock::new();
        STORE.get_or_init(MemoryStore::default)
    }
}

/// 把"刚从盘读出的 config"规范化成内存可用态：哨兵 → keychain 真实 key（注入内存）；
/// 明文 / 空 → 原样。哨兵绝不留在内存（防 `resolve_provider` 的 `is_empty()` 误判）。
///
/// 纯函数（store 可注入）→ 可单测，不碰真实 OS 钥匙串。
pub(crate) fn hydrate(config: &mut ProvidersConfig, store: &dyn SecretStore) {
    for p in &mut config.providers {
        if p.api_key == KEYCHAIN_SENTINEL {
            // key 在 keychain → 读回注入内存；读不到（entry 被删 / 后端暂时不可达）→ 空。
            p.api_key = store.load(&p.id).unwrap_or(None).unwrap_or_default();
        }
    }
}

/// 把内存 config 转成"要写盘的"config：
/// - 真实 key（非空非哨兵）→ store keychain；成功 → 字段换哨兵；失败 → 明文 fallback
/// - 空 key → 删 keychain 残留 entry（best-effort）；字段保持空
/// - 哨兵（内存态不该出现，防御性）→ 原样
///
/// 纯函数 → 可单测。调用方（`save_providers_config` / load 迁移）拿返回的 disk 写盘。
pub(crate) fn redact(config: &ProvidersConfig, store: &dyn SecretStore) -> ProvidersConfig {
    let mut disk = config.clone();
    for p in &mut disk.providers {
        let key = p.api_key.as_str();
        if key.is_empty() {
            let _ = store.delete(&p.id); // 无 key → 清残留 entry
        } else if key == KEYCHAIN_SENTINEL {
            continue; // 内存态不应有哨兵；防御性跳过
        } else {
            match store.store(&p.id, key) {
                Ok(()) => p.api_key = KEYCHAIN_SENTINEL.into(),
                Err(_) => {} // keychain 不可用 → 明文 fallback（字段保持）
            }
        }
    }
    disk
}

/// `load_providers_config` 用：active store + hydrate。
pub(crate) fn hydrate_active(config: &mut ProvidersConfig) {
    hydrate(config, active_store());
}

/// `save_providers_config` 用：active store + redact。
pub(crate) fn redact_active(config: &ProvidersConfig) -> ProvidersConfig {
    redact(config, active_store())
}
