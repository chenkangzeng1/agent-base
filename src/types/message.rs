use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl MessageRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }
    }

    pub fn as_api_role(&self) -> &'static str {
        self.as_str()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ChatMessage {
    System {
        content: String,
        /// 临时消息：turn 结束后从内存清理，持久化时跳过。
        #[serde(default, skip_serializing)]
        ephemeral: bool,
    },
    User {
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImageAttachment>,
        /// 临时消息：turn 结束后从内存清理，持久化时跳过。
        #[serde(default, skip_serializing)]
        ephemeral: bool,
    },
    Assistant {
        content: Option<String>,
        reasoning_content: Option<String>,
        tool_calls: Option<Vec<ToolCallMessage>>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
    /// Application-defined message type for extensibility.
    ///
    /// Consumers can inject custom message types (e.g., artifacts, notifications)
    /// into the conversation transcript. Custom messages are preserved in the
    /// transcript but filtered out by the default [`convert_to_llm`] callback
    /// before being sent to the LLM provider.
    Custom {
        role: String,
        data: serde_json::Value,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallMessage {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ImageAttachment {
    Url {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },
    Base64 {
        data: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ImageDetail {
    Low,
    High,
    Auto,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
            ephemeral: false,
        }
    }

    /// 创建临时 system 消息：turn 结束后自动清理，不持久化。
    pub fn system_ephemeral(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
            ephemeral: true,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
            images: Vec::new(),
            ephemeral: false,
        }
    }

    /// 创建临时 user 消息：turn 结束后自动清理，不持久化。
    pub fn user_ephemeral(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
            images: Vec::new(),
            ephemeral: true,
        }
    }

    pub fn user_with_images(content: impl Into<String>, images: Vec<ImageAttachment>) -> Self {
        Self::User {
            content: content.into(),
            images,
            ephemeral: false,
        }
    }

    /// 是否为临时消息（turn 结束后自动清理，不持久化）。
    pub fn is_ephemeral(&self) -> bool {
        match self {
            Self::System { ephemeral, .. } => *ephemeral,
            Self::User { ephemeral, .. } => *ephemeral,
            _ => false,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant {
            content: Some(content.into()),
            reasoning_content: None,
            tool_calls: None,
        }
    }

    pub fn assistant_with_reasoning(
        content: impl Into<String>,
        reasoning: impl Into<String>,
    ) -> Self {
        Self::Assistant {
            content: Some(content.into()),
            reasoning_content: Some(reasoning.into()),
            tool_calls: None,
        }
    }

    pub fn assistant_tool_call(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self::Assistant {
            content: None,
            reasoning_content: None,
            tool_calls: Some(vec![ToolCallMessage {
                id: tool_call_id.into(),
                name: tool_name.into(),
                arguments: arguments.into(),
            }]),
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::Tool {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
        }
    }
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
pub type ConvertToLlmFn = Arc<dyn Fn(&[ChatMessage]) -> Vec<ChatMessage> + Send + Sync>;

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

        // Custom messages are removed, regular messages are preserved
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

        // Custom callback that preserves only artifacts, not notifications
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

        // With a notification-only message
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
