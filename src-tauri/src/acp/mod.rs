//! C1 — ACP (Agent Client Protocol) client: drive EXTERNAL ACP-speaking coding
//! agents (`npx @zed-industries/codex-acp`, Claude Code via ACP, …) over stdio
//! JSON-RPC from the kernel, mirroring deer-flow's `tools/builtins/
//! invoke_acp_agent_tool.py`. This is the first — verifiable — half of the
//! bidirectional ACP support the OWOz design doc (C1) makes mandatory:
//!
//! > Dev Workbench Rust 内核应同时实现 ACP client（驱动外部编码 agent）+ ACP server
//! > （被 IDE / 其他 agent 调用）。
//!
//! The client half reuses the existing "external coding agent" mental model
//! (claude/codex/gemini spawned as CLIs in [`crate::agents::pty`]): the kernel
//! agent delegates a self-contained sub-task to a DIFFERENT coding agent it
//! cannot itself become, via the [`crate::kernel_impl::acp_tool::AcpAgentTool`]
//! (`dispatch_acp_agent`). The server half — exposing THIS kernel as an
//! ACP-servable agent (a stdio JSON-RPC binary an IDE like Zed drives) — is a
//! separate, larger effort and remains TODO.
//!
//! Blueprint: `agent-client-protocol` crate (Zed's reference SDK) `examples/
//! yolo_one_shot_client.rs` — initialize → new session → prompt → accumulate
//! `session/update` notifications.

pub mod client;

pub use client::{extract_update_text, run_acp_agent, AcpError, AcpRunResult};
