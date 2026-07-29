use serde::{Deserialize, Serialize};

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
        }
    }
}
