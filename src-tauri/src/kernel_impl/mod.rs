//! Trait implementations bridging kernel-core/kernel-compose to the existing
//! DevWorkbench subsystems (pty agents, knowledge store, quality gates) plus
//! the self-built transparent agent's tool ecosystem (MCP, Skills, Hooks).

pub mod executor;
pub mod hooks;
pub mod honesty;
pub mod mcp_tool;
pub mod react_agent;
pub mod retriever;
pub mod skill_tool;
pub mod unified_context;
