//! DevWorkbench kernel-core
//!
//! Trait & schema layer — the Rust-native port of eino's `schema` + `components`
//! abstractions. This crate defines the contracts (Message, Stream, Agent, Tool,
//! Retriever, ChatModel) that the rest of the kernel implements. It deliberately
//! has **zero implementations** and **zero heavy dependencies**: no LLM client,
//! no DB, no tokio runtime. This keeps the trait boundary stable and breakable
//! only via explicit API change.
//!
//! ## The dual-mode Agent
//!
//! The central abstraction is [`agent::Agent`], which has two implementations:
//! - **Opaque**: an external CLI process (claude/codex/gemini) whose internal
//!   reason→tool loop is a black box; the kernel only observes stdout/exit.
//! - **Transparent**: a self-built LLM agent (ReactAgent) where the kernel
//!   controls the ChatModel calls and tool execution directly.
//!
//! Both produce the same [`agent::AgentEvent`] stream, so the Graph engine and
//! the frontend render them uniformly.

pub mod agent;
pub mod document;
pub mod error;
pub mod schema;
pub mod traits;

pub use agent::{Agent, AgentCaps, AgentError, AgentEvent, AgentInput, AgentKind, AgentOutcome};
pub use document::Document;
pub use error::Error;
pub use schema::{FunctionCall, Message, Role, ToolCall};
pub use traits::{
    ChatModel, ChatTemplate, ModelOptions, RetrieveOptions, Tool, ToolContext, ToolInfo,
};
