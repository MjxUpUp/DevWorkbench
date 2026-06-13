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

/// A single chat message.
///
/// `tool_calls` is present only on assistant messages.
/// `tool_call_id` is present only on tool messages (correlates back to a ToolCall.id).
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
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning: None,
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
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["tool_calls"][0]["id"], "call_1");
        assert_eq!(v["tool_calls"][0]["function"]["name"], "grep");
    }
}
