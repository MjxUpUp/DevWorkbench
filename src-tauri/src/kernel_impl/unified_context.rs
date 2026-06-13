//! UnifiedContext — the cross-CLI memory + conversation projection layer.
//!
//! This is the kernel's answer to the post-ZCode-3.0 vacuum: when a developer
//! switches between claude / codex / gemini / a self-built agent on the SAME
//! project, each CLI keeps its own opaque session (claude's JSONL, codex's
//! sqlite, …) and the context is fragmented.
//!
//! UnifiedContext holds ONE canonical project memory + the current conversation,
//! and `project_for(kind)` renders it into the shape each consumer expects:
//!
//! - Opaque CLIs: a Markdown context block injected into the prompt (the same
//!   mechanism `knowledge/injector` already uses).
//! - Transparent agents: the raw `Vec<Message>` history.
//!
//! Memory entries mirror the user's proven `~/.claude/projects/*/memory/*.md`
//! format (title + content + provenance), so it's familiar and portable.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use kernel_core::Message;

/// One structured memory entry — mirrors the user's Claude `memory/*.md` files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub title: String,
    pub content: String,
    /// Where it came from (session id, file, manual).
    pub source: String,
    pub created_at: String,
}

/// The kind of consumer the context is being projected for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumerKind {
    /// External CLI — gets a Markdown injection block.
    ClaudeCode,
    Codex,
    GeminiCli,
    /// Transparent agent — gets raw messages.
    Transparent,
}

/// The canonical project context shared across all agents.
#[derive(Debug, Clone, Default)]
pub struct UnifiedContext {
    pub project_path: PathBuf,
    pub memory: Vec<MemoryEntry>,
    /// The running conversation (agent-agnostic).
    pub conversation: Vec<Message>,
    /// Per-CLI resume handles (e.g. claude's session id for `--resume`).
    pub cli_resume_ids: HashMap<String, String>,
}

/// The rendered form handed to a specific consumer.
pub struct ProjectedContext {
    /// For opaque CLIs: the full prompt with context injected.
    pub prompt: Option<String>,
    /// For transparent agents: the message history.
    pub messages: Option<Vec<Message>>,
    /// Resume handle if the CLI supports it.
    pub resume_from: Option<String>,
}

impl UnifiedContext {
    pub fn new(project_path: impl Into<PathBuf>) -> Self {
        Self {
            project_path: project_path.into(),
            ..Default::default()
        }
    }

    pub fn remember(&mut self, entry: MemoryEntry) {
        // Dedup by title — replace if same title exists.
        if let Some(existing) = self.memory.iter_mut().find(|m| m.title == entry.title) {
            *existing = entry;
        } else {
            self.memory.push(entry);
        }
    }

    pub fn add_message(&mut self, msg: Message) {
        self.conversation.push(msg);
    }

    /// Render the context for a specific consumer kind. The `task` is the
    /// user's current ask; memory + recent conversation are layered in.
    pub fn project_for(&self, kind: ConsumerKind, task: &str) -> ProjectedContext {
        let memory_block = self.render_memory_block();
        let resume_from = match kind {
            ConsumerKind::ClaudeCode => self.cli_resume_ids.get("claude").cloned(),
            ConsumerKind::Codex => self.cli_resume_ids.get("codex").cloned(),
            ConsumerKind::GeminiCli => self.cli_resume_ids.get("gemini").cloned(),
            ConsumerKind::Transparent => None,
        };

        match kind {
            ConsumerKind::Transparent => {
                let mut msgs = Vec::new();
                if !memory_block.is_empty() {
                    msgs.push(Message::system(memory_block));
                }
                msgs.extend(self.conversation.iter().cloned());
                if msgs.iter().all(|m| !m.content.contains(task)) {
                    msgs.push(Message::user(task));
                }
                ProjectedContext {
                    prompt: None,
                    messages: Some(msgs),
                    resume_from,
                }
            }
            _ => {
                // Opaque CLI: inject memory as a Markdown block in the prompt.
                let prompt = if memory_block.is_empty() {
                    task.to_string()
                } else {
                    format!("{memory_block}\n\n---\n\n{task}")
                };
                ProjectedContext {
                    prompt: Some(prompt),
                    messages: None,
                    resume_from,
                }
            }
        }
    }

    fn render_memory_block(&self) -> String {
        if self.memory.is_empty() {
            return String::new();
        }
        let mut out = String::from("## Project Memory Context\n\n");
        for entry in &self.memory {
            out.push_str(&format!("### {}\n{}\n\n", entry.title, entry.content));
        }
        out.push_str("## End Project Memory Context");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_core::Role;

    #[test]
    fn memory_dedup_replaces_same_title() {
        let mut ctx = UnifiedContext::new("/proj");
        ctx.remember(MemoryEntry {
            id: "1".into(),
            title: "auth bug".into(),
            content: "old".into(),
            source: "s1".into(),
            created_at: "t".into(),
        });
        ctx.remember(MemoryEntry {
            id: "2".into(),
            title: "auth bug".into(),
            content: "new".into(),
            source: "s2".into(),
            created_at: "t".into(),
        });
        assert_eq!(ctx.memory.len(), 1);
        assert_eq!(ctx.memory[0].content, "new");
    }

    #[test]
    fn opaque_projection_injects_memory_block() {
        let mut ctx = UnifiedContext::new("/proj");
        ctx.remember(MemoryEntry {
            id: "1".into(),
            title: "layout".into(),
            content: "use zcode single-column".into(),
            source: "s1".into(),
            created_at: "t".into(),
        });
        let p = ctx.project_for(ConsumerKind::ClaudeCode, "fix sidebar");
        let prompt = p.prompt.unwrap();
        assert!(prompt.contains("Project Memory Context"));
        assert!(prompt.contains("fix sidebar"));
    }

    #[test]
    fn transparent_projection_returns_messages() {
        let mut ctx = UnifiedContext::new("/proj");
        ctx.remember(MemoryEntry {
            id: "1".into(),
            title: "t".into(),
            content: "c".into(),
            source: "s".into(),
            created_at: "t".into(),
        });
        let p = ctx.project_for(ConsumerKind::Transparent, "do task");
        let msgs = p.messages.unwrap();
        assert_eq!(msgs[0].role, Role::System);
        assert!(msgs.iter().any(|m| m.role == Role::User && m.content.contains("do task")));
    }

    #[test]
    fn empty_memory_produces_no_block_for_opaque() {
        let ctx = UnifiedContext::new("/proj");
        let p = ctx.project_for(ConsumerKind::Codex, "just this");
        assert_eq!(p.prompt.unwrap(), "just this");
    }
}
