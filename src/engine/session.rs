use std::collections::HashSet;

use crate::types::{ChatMessage, Message, MessageRole};

use crate::types::SessionId;

#[derive(Clone, Debug, Default)]
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
        self.id
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
}
