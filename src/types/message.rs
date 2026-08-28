//! Message types for LLM conversations.
//!
//! `ChatMessage`, `ImageAttachment`, `ImageDetail`, and `ToolCallMessage` are
//! re-exported from `llm-trait`. `MessageRole` is re-exported from
//! `agent-types`. `Message` remains here as it is agent-runtime specific.

pub use agent_types::MessageRole;
pub use llm_trait::message::{ChatMessage, ImageAttachment, ImageDetail, ToolCallMessage};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
}

impl From<&ChatMessage> for Message {
    fn from(cm: &ChatMessage) -> Self {
        match cm {
            ChatMessage::System { content, .. } => Message {
                role: MessageRole::System,
                content: content.clone(),
            },
            ChatMessage::User { content, .. } => Message {
                role: MessageRole::User,
                content: content.clone(),
            },
            ChatMessage::Assistant { content, .. } => Message {
                role: MessageRole::Assistant,
                content: content.clone().unwrap_or_default(),
            },
            ChatMessage::Tool { content, .. } => Message {
                role: MessageRole::Tool,
                content: content.clone(),
            },
            ChatMessage::Custom { role: _, data } => Message {
                role: MessageRole::User,
                content: data.to_string(),
            },
        }
    }
}

/// Callback that transforms the message list before it is sent to the LLM.
///
/// The default implementation filters out [`ChatMessage::Custom`] variants because
/// most providers don't understand application-specific message types. Consumers
/// can override this to inject custom serialization logic for their message types.
pub type ConvertToLlmFn = std::sync::Arc<dyn Fn(&[ChatMessage]) -> Vec<ChatMessage> + Send + Sync>;

/// Default conversion that strips [`ChatMessage::Custom`] messages.
pub fn default_convert_to_llm(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    messages
        .iter()
        .filter(|m| !matches!(m, ChatMessage::Custom { .. }))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_convert_to_llm_filters_custom() {
        let messages = vec![
            ChatMessage::system("You are a helpful assistant."),
            ChatMessage::user("Hello"),
            ChatMessage::Custom {
                role: "artifact".to_string(),
                data: serde_json::json!({"id": "abc123"}),
            },
            ChatMessage::assistant("Hi there!"),
            ChatMessage::Custom {
                role: "notification".to_string(),
                data: serde_json::json!({"level": "info"}),
            },
            ChatMessage::tool("call_1", "result"),
        ];

        let filtered = default_convert_to_llm(&messages);

        assert_eq!(filtered.len(), 4);
        assert!(matches!(filtered[0], ChatMessage::System { .. }));
        assert!(matches!(filtered[1], ChatMessage::User { .. }));
        assert!(matches!(filtered[2], ChatMessage::Assistant { .. }));
        assert!(matches!(filtered[3], ChatMessage::Tool { .. }));
    }

    #[test]
    fn test_default_convert_to_llm_no_custom() {
        let messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("usr"),
            ChatMessage::assistant("asst"),
        ];

        let filtered = default_convert_to_llm(&messages);
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn test_custom_convert_to_llm_preserves_selected() {
        let messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::Custom {
                role: "artifact".to_string(),
                data: serde_json::json!({"id": "x"}),
            },
            ChatMessage::user("usr"),
        ];

        let convert = |msgs: &[ChatMessage]| -> Vec<ChatMessage> {
            msgs.iter()
                .filter(|m| match m {
                    ChatMessage::Custom { role, .. } => role == "artifact",
                    _ => true,
                })
                .cloned()
                .collect()
        };

        let filtered = convert(&messages);
        assert_eq!(filtered.len(), 3);
        assert!(matches!(filtered[0], ChatMessage::System { .. }));
        assert!(matches!(filtered[1], ChatMessage::Custom { .. }));
        assert!(matches!(filtered[2], ChatMessage::User { .. }));

        let messages2 = vec![
            ChatMessage::system("sys"),
            ChatMessage::Custom {
                role: "notification".to_string(),
                data: serde_json::json!({"level": "info"}),
            },
        ];
        let filtered2 = convert(&messages2);
        assert_eq!(filtered2.len(), 1);
        assert!(matches!(filtered2[0], ChatMessage::System { .. }));
    }

    #[test]
    fn test_custom_message_is_ephemeral_false() {
        let custom = ChatMessage::Custom {
            role: "artifact".to_string(),
            data: serde_json::json!({}),
        };
        assert!(!custom.is_ephemeral());
    }

    #[test]
    fn test_custom_message_serialization_roundtrip() {
        let custom = ChatMessage::Custom {
            role: "artifact".to_string(),
            data: serde_json::json!({"id": "test-123", "content": "hello"}),
        };
        let json_str = serde_json::to_string(&custom).unwrap();
        let deserialized: ChatMessage = serde_json::from_str(&json_str).unwrap();
        match deserialized {
            ChatMessage::Custom { role, data } => {
                assert_eq!(role, "artifact");
                assert_eq!(data["id"], "test-123");
                assert_eq!(data["content"], "hello");
            }
            _ => panic!("Expected Custom variant"),
        }
    }
}
