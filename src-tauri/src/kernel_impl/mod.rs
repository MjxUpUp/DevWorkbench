//! Trait implementations bridging kernel-core to the existing DevWorkbench
//! subsystems (pty agents, quality gates) plus the self-built transparent
//! agent's tool ecosystem (MCP, Skills, Hooks).

pub mod acp_tool;
pub mod anthropic_chat_model;
pub mod builtin_tools;
pub mod chat_model_shared;
pub mod checkpoint;
pub mod context_compact;
pub mod executor;
pub mod hooks;
pub mod honesty;
pub mod human_gate;
pub mod llm_recovery;
pub mod mission;
pub mod mcp_tool;
pub mod model_router;
pub mod openai_chat_model;
pub mod react_agent;
pub mod resource_budget;
pub mod skill_tool;
pub mod stream_health;
pub mod subagent_spec;
pub mod tool_call_repair;
