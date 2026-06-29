pub mod mcp;
pub mod adapters;
pub mod providers;
// B1: API Key 的 OS 钥匙串后端抽象（keyring v1 + 明文 fallback）。providers.rs
// 的 load/save 经此把 key 注入/转哨兵；pub(crate) 仅 crate 内部用。
pub(crate) mod secrets;
