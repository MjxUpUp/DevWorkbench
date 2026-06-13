//! DevWorkbench kernel-core
//!
//! Trait & schema layer — the Rust-native port of eino's `schema` + `components`
//! abstractions. This crate defines the contracts (Message, Stream, Agent, Tool,
//! Retriever, ChatModel) that the rest of the kernel implements. It deliberately
//! has **zero implementations** and **zero heavy dependencies**: no LLM client,
//! no DB, no tokio runtime. This keeps the trait boundary stable and breakable
//! only via explicit API change.
//!
//! Phase 0 (this file): establish the crate, the module skeleton, and the
//! foundational `Message`/`Role` schema so the workspace compiles.
//! Phase 1: fill in the Agent/Tool/ChatModel/Retriever traits + AgentEvent.

pub mod schema;

pub use schema::{Message, Role, ToolCall};
