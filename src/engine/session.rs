use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::types::{ChatMessage, ImageAttachment, Message, MessageRole, ToolCallMessage};

use crate::types::SessionId;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AgentSession {
    id: Option<SessionId>,
    messages: Vec<Message>,
    chat_messages: Vec<ChatMessage>,
    always_allowed_actions: HashSet<String>,
}

impl AgentSession {
    pub fn new(id: SessionId) -> Self {
        Self {
            id: Some(id),
            messages: Vec::new(),
            chat_messages: Vec::new(),
            always_allowed_actions: HashSet::new(),
        }
    }

    pub fn id(&self) -> Option<SessionId> {
        self.id.clone()
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn chat_messages(&self) -> &[ChatMessage] {
        &self.chat_messages
    }

    pub fn is_action_allowed(&self, action_key: &str) -> bool {
        self.always_allowed_actions.contains(action_key)
    }

    pub fn allow_action(&mut self, action_key: impl Into<String>) {
        self.always_allowed_actions.insert(action_key.into());
    }

    pub fn push_message(&mut self, role: MessageRole, content: impl Into<String>) {
        let content = content.into();
        self.messages.push(Message {
            role: role.clone(),
            content: content.clone(),
        });
        let chat_msg = match role {
            MessageRole::System => ChatMessage::system(content),
            MessageRole::User => ChatMessage::user(content),
            MessageRole::Assistant => ChatMessage::assistant(content),
            MessageRole::Tool => ChatMessage::tool(String::new(), content),
        };
        self.chat_messages.push(chat_msg);
    }

    pub fn push_user_message_with_images(
        &mut self,
        content: impl Into<String>,
        images: Vec<ImageAttachment>,
    ) {
        let content = content.into();
        self.messages.push(Message {
            role: MessageRole::User,
            content: content.clone(),
        });
        self.chat_messages
            .push(ChatMessage::user_with_images(content, images));
    }

    pub fn push_assistant_tool_call(
        &mut self,
        tool_call_id: &str,
        tool_name: &str,
        arguments_json: &str,
    ) {
        self.chat_messages.push(ChatMessage::assistant_tool_call(
            tool_call_id,
            tool_name,
            arguments_json,
        ));
    }

    pub fn push_assistant_tool_calls(&mut self, tool_calls: &[(String, String, String)]) {
        let calls: Vec<ToolCallMessage> = tool_calls
            .iter()
            .map(|(id, name, args)| ToolCallMessage {
                id: id.clone(),
                name: name.clone(),
                arguments: args.clone(),
            })
            .collect();
        self.chat_messages.push(ChatMessage::Assistant {
            content: None,
            reasoning_content: None,
            tool_calls: Some(calls),
        });
    }

    pub fn push_tool_result(&mut self, tool_call_id: &str, content: impl Into<String>) {
        let content = content.into();
        self.messages.push(Message {
            role: MessageRole::Tool,
            content: content.clone(),
        });
        self.chat_messages.push(ChatMessage::tool(
            tool_call_id,
            content,
        ));
    }

    pub fn close_dangling_tool_calls(&mut self, error_summary: &str) {
        let assistant_idx = self.chat_messages.iter().rposition(|m| {
            matches!(m, ChatMessage::Assistant { tool_calls: Some(tc), .. } if !tc.is_empty())
        });

        let Some(assistant_idx) = assistant_idx else {
            return;
        };

        let ChatMessage::Assistant { tool_calls: Some(tc), .. } = &self.chat_messages[assistant_idx] else {
            return;
        };

        let all_ids: Vec<String> = tc.iter().map(|t| t.id.clone()).collect();

        let answered_ids: Vec<String> = self.chat_messages[assistant_idx + 1..]
            .iter()
            .filter_map(|m| match m {
                ChatMessage::Tool { tool_call_id, .. } => Some(tool_call_id.clone()),
                _ => None,
            })
            .collect();

        for id in &all_ids {
            if !answered_ids.iter().any(|a| a == id) {
                self.push_tool_result(id, error_summary);
            }
        }
    }
}
