//! Trait implementations bridging kernel-core/kernel-compose to the existing
//! DevWorkbench subsystems (pty agents, knowledge store, quality gates) plus
//! the self-built transparent agent's tool ecosystem (MCP, Skills, Hooks).

pub mod builtin_tools;
pub mod checkpoint;
pub mod context_compact;
pub mod executor;
pub mod hooks;
pub mod honesty;
pub mod llm_recovery;
pub mod mcp_tool;
pub mod model_router;
pub mod opaque_agent;
pub mod react_agent;
pub mod skill_tool;
pub mod subagent_spec;
