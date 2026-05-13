use std::collections::HashSet;

use crate::types::{Message, MessageRole};
use serde_json::{Value, json};

use crate::types::SessionId;

#[derive(Clone, Debug, Default)]
pub struct AgentSession {
    id: Option<SessionId>,
    messages: Vec<Message>,
    raw_messages: Vec<Value>,
    always_allowed_actions: HashSet<String>,
}

impl AgentSession {
    pub fn new(id: SessionId) -> Self {
        Self {
            id: Some(id),
            messages: Vec::new(),
            raw_messages: Vec::new(),
            always_allowed_actions: HashSet::new(),
        }
    }

    pub fn id(&self) -> Option<SessionId> {
        self.id
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn raw_messages(&self) -> &[Value] {
        &self.raw_messages
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
        self.raw_messages.push(json!({
            "role": role.as_api_role(),
            "content": content
        }));
    }

    pub fn push_assistant_tool_call(
        &mut self,
        tool_call_id: &str,
        tool_name: &str,
        arguments_json: &str,
    ) {
        self.raw_messages.push(json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": tool_call_id,
                "type": "function",
                "function": {
                    "name": tool_name,
                    "arguments": arguments_json
                }
            }]
        }));
    }

    pub fn push_tool_result(&mut self, tool_call_id: &str, content: impl Into<String>) {
        let content = content.into();
        self.messages.push(Message {
            role: MessageRole::Tool,
            content: content.clone(),
        });
        self.raw_messages.push(json!({
            "role": "tool",
            "tool_call_id": tool_call_id,
            "content": content
        }));
    }
}
