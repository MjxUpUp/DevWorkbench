//! C1 — ACP (Agent Client Protocol) **bidirectional** support for the kernel.
//! The OWOz design doc (C1) makes both directions mandatory:
//!
//! > Dev Workbench Rust 内核应同时实现 ACP client（驱动外部编码 agent）+ ACP server
//! > （被 IDE / 其他 agent 调用）。
//!
//! - [`client`] — the kernel DRIVES an external ACP-speaking coding agent
//!   (`npx @zed-industries/codex-acp`, Claude Code via ACP, …) as a delegate,
//!   mirroring deer-flow's `tools/builtins/invoke_acp_agent_tool.py`. Surfaced
//!   to the kernel agent as [`crate::kernel_impl::acp_tool::AcpAgentTool`]
//!   (`dispatch_acp_agent`). Blueprint: the crate's `yolo_one_shot_client.rs`.
//! - [`server`] — the kernel is ITSELF served as an ACP agent an IDE / another
//!   agent drives over stdio (`initialize` / `session/new` / `session/prompt`),
//!   bridging the kernel [`kernel_core::AgentEvent`] stream to ACP
//!   `session/update` notifications. Blueprint: the crate's `simple_agent.rs`.
//!
//! Each half splits a pure, unit-tested protocol-mapping layer from a live
//! stdio driver that is structurally verified against the crate's own example
//! (the driver needs a real peer to exercise end to end).

pub mod client;
pub mod server;

pub use client::{extract_update_text, run_acp_agent, AcpError, AcpRunResult};
pub use server::{map_event_to_update, serve_stdio, EventBridge};
