//! Core message schema — the universal currency passed between kernel components.
//!
//! Mirrors eino's `schema.Message`: one struct carries every chat-message shape
//! (system/user/assistant/tool) and both text + tool-call payloads. Kept minimal
//! in Phase 0 (no multimodal parts); multimodal `Content` enum lands when a
//! component actually needs it (Phase 2+).

use serde::{Deserialize, Serialize};

/// Chat role. Matches eino `RoleType` / OpenAI roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A function call requested by the assistant.
///
/// `arguments` is a JSON-encoded string (not a parsed Value) to match how
/// LLMs return streaming tool-call fragments — the JSON arrives in pieces and
/// must be concatenated before parsing. This mirrors eino's `ToolCall.Function`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Correlation id — tool-result messages reference this back.
    pub id: String,
    /// Usually "function".
    #[serde(default, rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// JSON-encoded arguments string (may arrive fragmented in streams).
    pub arguments: String,
}

/// Metadata on a compact-boundary marker message (CCB parity:
/// `SystemCompactBoundaryMessage.compactMetadata`). Carried by a [`Message`]
/// whose [`Message::compact_boundary`] is `Some(_)` and `role` is
/// [`Role::System`]: it marks where an auto/manual compaction replaced part of
/// the conversation history with a summary.
///
/// The boundary is a META message — wire serializers drop System-role messages
/// (`anthropic_chat_model` / `openai_chat_model` filter them out), so it never
/// reaches the model. It exists so `maybe_compact` can locate the LAST boundary
/// and summarize only what came after it, avoiding re-compaction of already-
/// summarized history on resume (the "summary of summary" drift — defect ③'s
/// cousin). CCB does the same via `getMessagesAfterCompactBoundary`.
///
/// `preserved_count` is DW's adaptation of CCB `preservedSegment` (headUuid/
/// tailUuid/anchorUuid): DW has no per-message uuids, so the verbatim-preserved
/// tail is recorded as a count rather than a uuid range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactBoundaryMeta {
    /// `"auto"` | `"manual"` — what triggered the compaction (CCB `trigger`).
    pub trigger: String,
    /// Estimated tokens just before compaction ran (CCB `preTokens`).
    pub pre_tokens: usize,
    /// How many trailing messages were preserved verbatim across this
    /// compaction (CCB `preservedSegment` size; DW uses a count, not uuids).
    pub preserved_count: usize,
}

/// A single chat message.
///
/// `tool_calls` is present only on assistant messages.
/// `tool_call_id` is present only on tool messages (correlates back to a ToolCall.id).
#[must_use = "Message construction has no side effect; use the returned value"]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Reasoning/thinking-model trace (e.g. DeepSeek-R1, GLM thinking). Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// Cryptographic signature over `reasoning` (Anthropic `signature_delta`).
    /// Required to *preserve* a thinking block across turns: Anthropic/GLM
    /// reject a replayed thinking block whose signature is missing or tampered.
    /// Present only when the model emitted one AND `reasoning` is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_signature: Option<String>,
    /// Compact-boundary marker. `Some(_)` ONLY on a System-role meta message
    /// that marks a compaction boundary (see [`CompactBoundaryMeta`]). Wire
    /// serializers filter System messages, so this never reaches the model;
    /// it lets `maybe_compact` find the last boundary and skip already-summarized
    /// history on resume. `None` for every real conversation message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_boundary: Option<CompactBoundaryMeta>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning: None,
            reasoning_signature: None,
            compact_boundary: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning: None,
            reasoning_signature: None,
            compact_boundary: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning: None,
            reasoning_signature: None,
            compact_boundary: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_message_roundtrips_with_lowercase_role() {
        let m = Message::system("you are a coding agent");
        let json = serde_json::to_string(&m).unwrap();
        assert!(
            json.contains(r#""role":"system""#),
            "role must serialize lowercase; got: {json}"
        );
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.role, Role::System);
        assert_eq!(back.content, "you are a coding agent");
    }

    #[test]
    fn empty_tool_calls_omitted_from_json() {
        let m = Message::user("hi");
        let json = serde_json::to_string(&m).unwrap();
        assert!(
            !json.contains("tool_calls"),
            "empty tool_calls must be omitted; got: {json}"
        );
        assert!(
            !json.contains("tool_call_id"),
            "None tool_call_id must be omitted; got: {json}"
        );
    }

    #[test]
    fn assistant_message_with_tool_calls_serializes() {
        let m = Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "grep".into(),
                    arguments: r#"{"q":"foo"}"#.into(),
                },
            }],
            tool_call_id: None,
            reasoning: None,
            reasoning_signature: None,
            compact_boundary: None,
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["tool_calls"][0]["id"], "call_1");
        assert_eq!(v["tool_calls"][0]["function"]["name"], "grep");
    }
}
