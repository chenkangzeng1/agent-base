use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::types::{ChatMessage, ImageAttachment, Message, MessageRole, ToolCallMessage};

use crate::types::SessionId;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AgentSession {
    id: Option<SessionId>,
    /// LLM API format messages, sent directly to the provider.
    /// This is the single source of truth for the conversation state.
    chat_messages: Vec<ChatMessage>,
    always_allowed_actions: HashSet<String>,
    /// Total number of tool calls made in this session (across all turns).
    /// Used by middleware for decisions like "first_turn_only" enforcement.
    pub total_tool_calls: usize,
    /// Number of tool-enforcement nudges issued in the current turn.
    /// Reset to 0 at the start of each turn (when a new user message arrives).
    /// Used by `ToolEnforcementMiddleware` to cap nudge attempts per turn.
    pub nudge_count: usize,
    /// Number of tool calls already executed in the current turn.
    /// Reset to 0 at the start of each turn (when a new user message arrives).
    /// Used by `TurnToolLimitMiddleware` to enforce per-turn tool call limits.
    pub turn_tool_calls: usize,
}

impl AgentSession {
    pub fn new(id: SessionId) -> Self {
        Self {
            id: Some(id),
            chat_messages: Vec::new(),
            always_allowed_actions: HashSet::new(),
            total_tool_calls: 0,
            nudge_count: 0,
            turn_tool_calls: 0,
        }
    }

    pub fn id(&self) -> Option<SessionId> {
        self.id.clone()
    }

    /// Derive a simplified `Vec<Message>` view from the canonical `chat_messages`.
    /// Assistant messages that contain only tool_calls (no text content) are
    /// skipped, since they have no corresponding simplified representation.
    pub fn simple_messages(&self) -> Vec<Message> {
        self.chat_messages
            .iter()
            .filter_map(|cm| match cm {
                ChatMessage::Assistant {
                    content: None, ..
                } => None,
                ChatMessage::Assistant {
                    content: Some(c),
                    tool_calls: Some(tc),
                    ..
                } if c.is_empty() && !tc.is_empty() => None,
                _ => Some(Message::from(cm)),
            })
            .collect()
    }

    pub fn chat_messages(&self) -> &[ChatMessage] {
        &self.chat_messages
    }

    /// 可变引用，仅用于需要直接操作消息的高级场景。
    pub fn chat_messages_mut(&mut self) -> &mut Vec<ChatMessage> {
        &mut self.chat_messages
    }

    pub fn is_action_allowed(&self, action_key: &str) -> bool {
        self.always_allowed_actions.contains(action_key)
    }

    pub fn allow_action(&mut self, action_key: impl Into<String>) {
        self.always_allowed_actions.insert(action_key.into());
    }

    pub fn push_message(&mut self, role: MessageRole, content: impl Into<String>) {
        let content = content.into();
        let chat_msg = match role {
            MessageRole::System => ChatMessage::system(content),
            MessageRole::User => ChatMessage::user(content),
            MessageRole::Assistant => ChatMessage::assistant(content),
            MessageRole::Tool => ChatMessage::tool(String::new(), content),
        };
        self.chat_messages.push(chat_msg);
    }

    /// Push an assistant message with reasoning/thinking content preserved.
    /// This allows the LLM to see its own prior reasoning in subsequent turns,
    /// preventing it from re-deriving the same conclusions every turn.
    pub fn push_assistant_with_reasoning(
        &mut self,
        content: impl Into<String>,
        reasoning: impl Into<String>,
    ) {
        self.chat_messages
            .push(ChatMessage::assistant_with_reasoning(content, reasoning));
    }

    pub fn push_user_message_with_images(
        &mut self,
        content: impl Into<String>,
        images: Vec<ImageAttachment>,
    ) {
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

    pub fn push_assistant_tool_calls(
        &mut self,
        tool_calls: &[(String, String, String)],
        reasoning: Option<String>,
    ) {
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
            reasoning_content: reasoning,
            tool_calls: Some(calls),
        });
    }

    pub fn push_tool_result(&mut self, tool_call_id: &str, content: impl Into<String>) {
        self.chat_messages
            .push(ChatMessage::tool(tool_call_id, content));
    }

    /// 移除所有临时消息（ephemeral=true）。
    ///
    /// 在 turn 结束时调用，确保注入的临时内容不残留到下一轮。
    pub fn remove_ephemeral_messages(&mut self) {
        let before = self.chat_messages.len();
        self.chat_messages.retain(|m| !m.is_ephemeral());
        let removed = before - self.chat_messages.len();
        if removed > 0 {
            tracing::debug!(removed, remaining = self.chat_messages.len(), "ephemeral messages cleaned up");
        }
    }

    /// Count the number of conversation turns.
    /// A turn starts with a User message and includes subsequent Assistant/Tool messages.
    pub fn turn_count(&self) -> usize {
        self.chat_messages
            .iter()
            .filter(|m| matches!(m, ChatMessage::User { .. }))
            .count()
    }

    /// Remove the oldest turns from the front until turn count ≤ max_turns.
    /// Preserves the System message at index 0 if present.
    pub fn trim_oldest_turns(&mut self, max_turns: usize) {
        let current_turns = self.turn_count();
        if current_turns <= max_turns {
            return;
        }
        let turns_to_remove = current_turns - max_turns;

        // Find User message positions (turn boundaries) in chat_messages
        let user_positions: Vec<usize> = self
            .chat_messages
            .iter()
            .enumerate()
            .filter_map(|(i, m)| if matches!(m, ChatMessage::User { .. }) { Some(i) } else { None })
            .collect();

        if user_positions.len() <= turns_to_remove {
            return;
        }

        // Preserve system prefix: count leading System messages
        let system_prefix = self
            .chat_messages
            .iter()
            .take_while(|m| matches!(m, ChatMessage::System { .. }))
            .count();

        // Drain from system_prefix up to the start of the (turns_to_remove + 1)-th turn
        let drain_end = user_positions[turns_to_remove];
        if system_prefix >= drain_end {
            return; // nothing to drain after system messages
        }

        self.chat_messages.drain(system_prefix..drain_end);
    }

    /// Remove the last message from `chat_messages`.
    /// Used by the max_message_tokens safety valve to discard oversized messages.
    pub fn pop_last_message(&mut self) {
        self.chat_messages.pop();
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

    /// Replace chat messages — only for persistence restore.
    /// Validates message sequence before replacing.
    ///
    /// 仅供持久化恢复使用。调用方必须保证 messages 序列合法。
    pub fn set_chat_messages(&mut self, messages: Vec<ChatMessage>) -> Result<(), String> {
        validate_message_sequence(&messages)?;
        // Recalculate total_tool_calls from the incoming messages so middleware
        // decisions (e.g. first_turn_only enforcement) see the correct count.
        self.total_tool_calls = messages
            .iter()
            .filter_map(|m| match m {
                ChatMessage::Assistant {
                    tool_calls: Some(tc), ..
                } => Some(tc.len()),
                _ => None,
            })
            .sum();
        self.chat_messages = messages;
        Ok(())
    }
}

/// Validate that a chat message sequence is well-formed for LLM API consumption.
///
/// Checks:
/// - No Tool message without a preceding Assistant with matching tool_call
/// - No duplicate Tool messages for the same tool_call_id
/// - All tool_calls in an Assistant batch must be answered before the next Assistant batch
/// - No unanswered tool calls at the end of the sequence
pub fn validate_message_sequence(messages: &[ChatMessage]) -> Result<(), String> {
    let mut pending_tool_call_ids: HashSet<String> = HashSet::new();

    for (i, msg) in messages.iter().enumerate() {
        match msg {
            ChatMessage::Tool { tool_call_id, .. } => {
                if pending_tool_call_ids.is_empty() {
                    return Err(format!(
                        "message[{}]: Tool message with call_id '{}' has no preceding tool_call",
                        i, tool_call_id
                    ));
                }
                // Remove the ID on match — also detects duplicates (second remove returns false)
                if !pending_tool_call_ids.remove(tool_call_id) {
                    return Err(format!(
                        "message[{}]: Tool message with call_id '{}' does not match any pending tool_call (already answered or unknown)",
                        i, tool_call_id
                    ));
                }
            }
            ChatMessage::Assistant {
                tool_calls: Some(tc), ..
            } => {
                // Previous batch must be fully answered before a new batch starts
                if !pending_tool_call_ids.is_empty() {
                    return Err(format!(
                        "message[{}]: Assistant message with new tool_calls appears before pending calls were answered: {:?}",
                        i, pending_tool_call_ids
                    ));
                }
                pending_tool_call_ids = tc.iter().map(|t| t.id.clone()).collect();
            }
            _ => {}
        }
    }

    // All tool calls must be answered by the end of the sequence
    if !pending_tool_call_ids.is_empty() {
        return Err(format!(
            "message sequence ends with unanswered tool calls: {:?}",
            pending_tool_call_ids
        ));
    }

    Ok(())
}

#[cfg(test)]
fn make_session() -> AgentSession {
    AgentSession::new(SessionId::new(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_turn_count_empty() {
        let s = make_session();
        assert_eq!(s.turn_count(), 0);
    }

    #[test]
    fn test_turn_count_with_system_and_user() {
        let mut s = make_session();
        s.push_message(MessageRole::System, "system");
        assert_eq!(s.turn_count(), 0);
        s.push_message(MessageRole::User, "hello");
        assert_eq!(s.turn_count(), 1);
        s.push_message(MessageRole::Assistant, "hi");
        assert_eq!(s.turn_count(), 1);
        s.push_message(MessageRole::User, "bye");
        assert_eq!(s.turn_count(), 2);
    }

    #[test]
    fn test_turn_count_with_tool_calls() {
        let mut s = make_session();
        s.push_message(MessageRole::User, "do something");
        s.push_assistant_tool_calls(&[("id1".into(), "tool".into(), "{}".into())], None);
        s.push_tool_result("id1", "result");
        s.push_message(MessageRole::Assistant, "done");
        // One user turn: User -> Assistant(tool_calls) -> Tool -> Assistant(text)
        assert_eq!(s.turn_count(), 1);
    }

    #[test]
    fn test_trim_oldest_turns_noop() {
        let mut s = make_session();
        s.push_message(MessageRole::User, "hello");
        s.push_message(MessageRole::Assistant, "hi");
        s.trim_oldest_turns(5);
        assert_eq!(s.turn_count(), 1);
        assert_eq!(s.chat_messages().len(), 2);
    }

    #[test]
    fn test_trim_oldest_turns_removes_old() {
        let mut s = make_session();
        s.push_message(MessageRole::System, "sys");
        // Turn 1
        s.push_message(MessageRole::User, "u1");
        s.push_message(MessageRole::Assistant, "a1");
        // Turn 2
        s.push_message(MessageRole::User, "u2");
        s.push_message(MessageRole::Assistant, "a2");
        // Turn 3
        s.push_message(MessageRole::User, "u3");
        s.push_message(MessageRole::Assistant, "a3");

        s.trim_oldest_turns(2);
        assert_eq!(s.turn_count(), 2);
        // System message preserved
        assert!(matches!(s.chat_messages()[0], ChatMessage::System { .. }));
        // Oldest user message is u2
        assert!(matches!(s.chat_messages()[1], ChatMessage::User { ref content, .. } if content == "u2"));
    }

    #[test]
    fn test_trim_oldest_turns_with_tool_calls() {
        let mut s = make_session();
        // Turn 1 with tool call
        s.push_message(MessageRole::User, "u1");
        s.push_assistant_tool_calls(&[("id1".into(), "t".into(), "{}".into())], None);
        s.push_tool_result("id1", "r1");
        s.push_message(MessageRole::Assistant, "a1");
        // Turn 2
        s.push_message(MessageRole::User, "u2");
        s.push_message(MessageRole::Assistant, "a2");

        let msg_before = s.simple_messages().len();
        let chat_before = s.chat_messages().len();
        s.trim_oldest_turns(1);
        assert_eq!(s.turn_count(), 1);
        // chat_messages should have lost 4 entries (User, Assistant(tool), Tool, Assistant(text))
        assert_eq!(s.chat_messages().len(), chat_before - 4);
        // simple_messages (derived from chat_messages, tool_calls-only filtered) loses 3 entries
        assert_eq!(s.simple_messages().len(), msg_before - 3);
    }

    #[test]
    fn test_pop_last_message_text() {
        let mut s = make_session();
        s.push_message(MessageRole::User, "hello");
        s.push_message(MessageRole::Assistant, "hi");
        assert_eq!(s.chat_messages().len(), 2);
        s.pop_last_message();
        assert_eq!(s.chat_messages().len(), 1);
        assert_eq!(s.simple_messages().len(), 1);
    }

    #[test]
    fn test_pop_last_message_tool_calls_only() {
        let mut s = make_session();
        s.push_message(MessageRole::User, "do it");
        s.push_assistant_tool_calls(&[("id1".into(), "t".into(), "{}".into())], None);
        assert_eq!(s.chat_messages().len(), 2);
        assert_eq!(s.simple_messages().len(), 1); // only User in simple_messages (tool_calls-only filtered)
        s.pop_last_message();
        assert_eq!(s.chat_messages().len(), 1);
        assert_eq!(s.simple_messages().len(), 1); // simple_messages unchanged (still just User)
    }

    #[test]
    fn test_pop_last_message_empty_session() {
        let mut s = make_session();
        s.pop_last_message(); // should not panic
        assert_eq!(s.chat_messages().len(), 0);
    }
}

#[cfg(test)]
mod validate_tests {
    use super::*;

    #[test]
    fn test_valid_simple_sequence() {
        let msgs = vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi"),
        ];
        assert!(validate_message_sequence(&msgs).is_ok());
    }

    #[test]
    fn test_valid_tool_call_sequence() {
        let msgs = vec![
            ChatMessage::user("run command"),
            ChatMessage::assistant_tool_call("call_1", "bash", r#"{"cmd":"ls"}"#),
            ChatMessage::tool("call_1", "file1 file2"),
            ChatMessage::assistant("done"),
        ];
        assert!(validate_message_sequence(&msgs).is_ok());
    }

    #[test]
    fn test_valid_multi_tool_call_sequence() {
        let msgs = vec![
            ChatMessage::user("run commands"),
            ChatMessage::Assistant {
                content: None,
                reasoning_content: None,
                tool_calls: Some(vec![
                    crate::types::ToolCallMessage {
                        id: "call_1".into(),
                        name: "bash".into(),
                        arguments: "{}".into(),
                    },
                    crate::types::ToolCallMessage {
                        id: "call_2".into(),
                        name: "read".into(),
                        arguments: "{}".into(),
                    },
                ]),
            },
            ChatMessage::tool("call_1", "result1"),
            ChatMessage::tool("call_2", "result2"),
            ChatMessage::assistant("done"),
        ];
        assert!(validate_message_sequence(&msgs).is_ok());
    }

    #[test]
    fn test_orphaned_tool_result() {
        let msgs = vec![
            ChatMessage::user("hello"),
            ChatMessage::tool("call_1", "orphaned result"),
        ];
        let err = validate_message_sequence(&msgs).unwrap_err();
        assert!(err.contains("no preceding tool_call"));
    }

    #[test]
    fn test_mismatched_tool_call_id() {
        let msgs = vec![
            ChatMessage::user("run"),
            ChatMessage::assistant_tool_call("call_1", "bash", "{}"),
            ChatMessage::tool("call_2", "wrong id"),
        ];
        let err = validate_message_sequence(&msgs).unwrap_err();
        assert!(err.contains("does not match"));
    }

    #[test]
    fn test_set_chat_messages_valid() {
        let mut s = make_session();
        let msgs = vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi"),
        ];
        assert!(s.set_chat_messages(msgs.clone()).is_ok());
        assert_eq!(s.chat_messages().len(), 2);
    }

    #[test]
    fn test_set_chat_messages_invalid() {
        let mut s = make_session();
        let msgs = vec![
            ChatMessage::tool("call_1", "orphaned"),
        ];
        assert!(s.set_chat_messages(msgs).is_err());
    }
}
