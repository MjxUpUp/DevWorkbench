//! The unified dual-mode Agent trait — the heart of the kernel.
//!
//! Design rationale (see kernel design doc): eino's `TypedAgent.Run →
//! AsyncIterator<Event>` is built for an in-process LLM it controls. We need
//! ONE trait that BOTH a black-box CLI process and a transparent ReactAgent can
//! implement. So the trait is intentionally minimal:
//!
//! - `run()` returns a single `BoxStream<Result<AgentEvent>>`
//! - The stream emits incremental `Token`/`ToolCall`/`FileChanged` events as
//!   the agent works, a `TurnBoundary` between turns, and exactly one terminal
//!   `Done(AgentOutcome)` (or `Error`).
//! - Cancellation is via the caller dropping the stream (transparent agent) or
//!   a separate control channel (opaque agent → sends Ctrl-C to the process).
//!
//! This lets the Graph engine compose both kinds of agent uniformly.

use std::path::PathBuf;

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::Error;

/// Which kind of agent this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    /// An external CLI process (claude/codex/gemini/…) whose internal loop is
    /// opaque to the kernel. The kernel spawns and observes it.
    Opaque,
    /// A self-built agent (ReactAgent) where the kernel controls the ChatModel
    /// calls and tool execution directly.
    Transparent,
}

/// Declared capabilities — lets the graph and the honesty layer know what an
/// agent can actually do (e.g. an opaque agent cannot accept injected tools).
#[derive(Debug, Clone, Default)]
pub struct AgentCaps {
    /// Can the kernel interrupt it mid-run without killing the process?
    pub interruptible: bool,
    /// Can it resume a previous run (CLI --resume / message history replay)?
    pub resumable: bool,
    /// Can the kernel inject tool definitions into it? (only transparent)
    pub injectable_tools: bool,
    /// Does it only read, never write? (safety gating)
    pub read_only: bool,
}

/// Input to an agent run.
#[derive(Debug, Clone)]
pub struct AgentInput {
    /// The natural-language task/prompt.
    pub prompt: String,
    /// Working directory (project root) the agent operates in.
    pub working_dir: Option<String>,
    /// Optional model override (e.g. "glm-4.6", "claude-sonnet-4").
    pub model: Option<String>,
    /// Resume a prior run — opaque agents map this to `--resume <id>`;
    /// transparent agents replay their message history.
    pub resume_from: Option<String>,
}

/// A single event in the agent's output stream.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Incremental text output (a token chunk, or a parsed line of CLI stdout).
    Token(String),
    /// Incremental reasoning/thinking trace from a thinking model (GLM
    /// Interleaved Thinking). Yielded chunk-by-chunk as the model reasons,
    /// typically BEFORE the visible answer tokens. Chat renders this as a
    /// collapsible thinking block, separate from the answer text.
    Reasoning(String),
    /// The agent invoked a tool. Opaque agents emit this best-effort by parsing
    /// CLI output; transparent agents emit it directly.
    ToolCall(ToolCallEvent),
    /// A file was changed on disk (observed via git/watcher, both agent kinds).
    FileChanged(PathBuf),
    /// A turn boundary — a natural interruption point (eino safe-point analog).
    TurnBoundary,
    /// Terminal success. Carries the structured outcome.
    Done(AgentOutcome),
}

/// A tool-call observation.
#[derive(Debug, Clone)]
pub struct ToolCallEvent {
    pub tool: String,
    /// Raw arguments as observed (may be partial for opaque agents).
    pub arguments: String,
    pub status: ToolCallStatus,
    /// The tool's output — the success payload on `Succeeded`, the error/block
    /// text on `Failed`. `None` for `Started`, or when the emitter reports only
    /// status (e.g. an opaque agent that doesn't parse tool output). The
    /// transparent ReactAgent fills this so the chat UI renders the real tool
    /// output instead of a placeholder.
    pub result: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallStatus {
    Started,
    Succeeded,
    Failed,
}

/// The structured result of a completed agent run.
#[derive(Debug, Clone, Default)]
pub struct AgentOutcome {
    pub status: AgentRunStatus,
    /// Files changed during the run (from git diff snapshot).
    pub files_changed: Vec<String>,
    /// Exit code if known (opaque agents).
    pub exit_code: Option<i32>,
    /// Truncated textual summary (tail of output).
    pub output_summary: Option<String>,
    /// Post-hoc honesty audit result (JSON from HonestyVerifier).
    ///
    /// Opaque agents (black-box CLI) fill this: call-level hooks are physically
    /// impossible inside the subprocess, so honesty is enforced *after* the CLI
    /// exits by scanning the uncommitted diff for assertion weakening + sanity-
    /// checking the compile env. Transparent agents (ReactAgent) leave this
    /// `None` — their honesty is enforced at the call level via HookManager,
    /// where each tool invocation can be inspected before it commits.
    pub honesty: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentRunStatus {
    #[default]
    Completed,
    Failed,
    Cancelled,
}

/// The unified agent trait.
#[async_trait]
pub trait Agent: Send + Sync {
    fn kind(&self) -> AgentKind;
    fn capabilities(&self) -> AgentCaps;

    /// Begin a run. The returned stream MUST be driven to completion (or
    /// dropped to cancel). Exactly one terminal event (Done or Error) is
    /// guaranteed; events before it are incremental.
    fn run(
        &self,
        input: AgentInput,
    ) -> Result<BoxStream<'static, Result<AgentEvent, Error>>, Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_outcome_default_is_completed() {
        let o = AgentOutcome::default();
        assert_eq!(o.status, AgentRunStatus::Completed);
        assert!(o.files_changed.is_empty());
    }

    #[test]
    fn agent_caps_default_is_all_false() {
        let c = AgentCaps::default();
        assert!(!c.interruptible);
        assert!(!c.resumable);
        assert!(!c.injectable_tools);
        assert!(!c.read_only);
    }

    #[test]
    fn tool_call_event_carries_optional_result() {
        // Succeeded/Failed carry the real tool output; Started carries None.
        let done = ToolCallEvent {
            tool: "Read".into(),
            arguments: "{}".into(),
            status: ToolCallStatus::Succeeded,
            result: Some("the file contents".into()),
        };
        assert_eq!(done.result.as_deref(), Some("the file contents"));
        let started = ToolCallEvent {
            tool: "Read".into(),
            arguments: "{}".into(),
            status: ToolCallStatus::Started,
            result: None,
        };
        assert!(started.result.is_none());
    }
}
