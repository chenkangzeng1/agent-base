use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::types::{ChatMessage, ImageAttachment, Message, MessageRole, ToolCallMessage};

use crate::types::SessionId;

/// Run-level state tracking for the react loop.
///
/// Manages counters and flags that track the current run's progress.
/// All fields are reset at the start of each run (when a new user message arrives).
///
/// # Backward compatibility
///
/// Before this struct existed, `nudge_count` and `turn_tool_calls` were flat
/// fields on `AgentSession`.  The [`RawAgentSession`] deserialization shim
/// migrates them automatically.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RunState {
    /// Number of tool calls already executed in the current turn.
    /// Reset to 0 at the start of each turn (when a new user message arrives).
    /// Used by `TurnToolLimitMiddleware` to enforce per-turn tool call limits.
    pub turn_tool_calls: usize,
    /// Whether any tools were called in the current run.
    /// Reset to false at the start of each run. Used by the completion judge
    /// to determine if tools were used before a text-only response.
    pub run_has_tool_calls: bool,
    /// Number of consecutive LLM turns that produced only reasoning_content
    /// (no text, no tool call). Reset to 0 when a normal response or tool call
    /// is produced. Used by the react loop to fail instead of looping forever
    /// on a reasoning-model runaway.
    pub reasoning_only_strikes: usize,
    /// Number of consecutive LLM turns that produced a completely empty response
    /// (no text, no reasoning, no tool call). Reset to 0 when a normal response
    /// or tool call is produced. Used by the react loop to retry a bounded
    /// number of times, then fail instead of looping forever.
    pub empty_response_strikes: usize,
    /// Number of tool-enforcement nudges issued in the current turn.
    /// Reset to 0 at the start of each turn (when a new user message arrives).
    /// Used by `ToolEnforcementMiddleware` to cap nudge attempts per turn.
    pub nudge_count: usize,
}

impl RunState {
    /// Reset all run-level state for a new run (when a new user message arrives).
    pub fn reset_for_new_run(&mut self) {
        self.turn_tool_calls = 0;
        self.run_has_tool_calls = false;
        self.reasoning_only_strikes = 0;
        self.empty_response_strikes = 0;
        self.nudge_count = 0;
    }

    /// Record tool calls (branch 3: tool calls).
    /// Resets reasoning_only_strikes and empty_response_strikes.
    pub fn record_tool_calls(&mut self, n: usize) {
        self.turn_tool_calls += n;
        self.run_has_tool_calls = true;
        self.reasoning_only_strikes = 0;
        self.empty_response_strikes = 0;
    }

    /// Record reasoning-only response (branch 1).
    /// Resets empty_response_strikes.
    /// Returns the new strike count.
    pub fn record_reasoning_only(&mut self) -> usize {
        self.empty_response_strikes = 0;
        self.reasoning_only_strikes += 1;
        self.reasoning_only_strikes
    }

    /// Record empty response (branch 2).
    /// Resets reasoning_only_strikes.
    /// Returns the new strike count.
    pub fn record_empty_response(&mut self) -> usize {
        self.reasoning_only_strikes = 0;
        self.empty_response_strikes += 1;
        self.empty_response_strikes
    }
}

/// Deserialization shim for backward compatibility.
///
/// Before `RunState` was introduced, `nudge_count`, `turn_tool_calls`,
/// `reasoning_only_strikes`, and `empty_response_strikes` were flat fields
/// on `AgentSession`.  This struct accepts **both** the old flat format and
/// the new nested `run_state` format, migrating legacy data on the fly.
#[derive(Deserialize)]
struct RawAgentSession {
    id: Option<SessionId>,
    chat_messages: Vec<ChatMessage>,
    always_allowed_actions: HashSet<String>,
    total_tool_calls: usize,

    // ── new format (preferred) ──
    run_state: Option<RunState>,

    // ── legacy flat fields (fallback) ──
    nudge_count: Option<usize>,
    turn_tool_calls: Option<usize>,
    reasoning_only_strikes: Option<usize>,
    empty_response_strikes: Option<usize>,
}

impl From<RawAgentSession> for AgentSession {
    fn from(raw: RawAgentSession) -> Self {
        let run_state = raw.run_state.unwrap_or_else(|| RunState {
            nudge_count: raw.nudge_count.unwrap_or(0),
            turn_tool_calls: raw.turn_tool_calls.unwrap_or(0),
            reasoning_only_strikes: raw.reasoning_only_strikes.unwrap_or(0),
            empty_response_strikes: raw.empty_response_strikes.unwrap_or(0),
            ..RunState::default()
        });
        Self {
            id: raw.id,
            chat_messages: raw.chat_messages,
            always_allowed_actions: raw.always_allowed_actions,
            total_tool_calls: raw.total_tool_calls,
            run_state,
        }
    }
}

/// Stable identity + rolling state of a single chat thread.
///
/// `Default` returns a fresh session; load persisted state via serde.
/// Backward-compatible with the old flat-field format (pre-`RunState` migration).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(from = "RawAgentSession")]
pub struct AgentSession {
    id: Option<SessionId>,
    /// LLM API format messages, sent directly to the provider.
    /// This is the single source of truth for the conversation state.
    chat_messages: Vec<ChatMessage>,
    always_allowed_actions: HashSet<String>,
    /// Total number of tool calls made in this session (across all turns).
    /// Used by middleware for decisions like "first_turn_only" enforcement.
    pub total_tool_calls: usize,
    /// Run-level state tracking for the react loop.
    pub run_state: RunState,
}

impl AgentSession {
    pub fn new(id: SessionId) -> Self {
        Self {
            id: Some(id),
            chat_messages: Vec::new(),
            always_allowed_actions: HashSet::new(),
            total_tool_calls: 0,
            run_state: RunState::default(),
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
                ChatMessage::Assistant { content: None, .. } => None,
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
        content: Option<String>,
    ) {
        let calls: Vec<ToolCallMessage> = tool_calls
            .iter()
            .map(|(id, name, args)| {
                // Validate that arguments are valid JSON.
                // If not (e.g., truncated by token limit), wrap in an error object
                // so the API doesn't reject the entire request with 400.
                let valid_args = if serde_json::from_str::<serde_json::Value>(args).is_ok() {
                    args.clone()
                } else {
                    tracing::warn!(
                        tool_name = %name,
                        args_len = args.len(),
                        "tool call arguments are not valid JSON (possibly truncated), wrapping in error object"
                    );
                    // Find a safe char boundary at or before byte 200 to avoid
                    // panicking on multi-byte UTF-8 chars (CJK, emoji, etc.).
                    let max_preview = 200;
                    let safe_end = if args.len() <= max_preview {
                        args.len()
                    } else {
                        args.char_indices()
                            .find(|(i, _)| *i >= max_preview)
                            .map(|(i, _)| i)
                            .unwrap_or(args.len())
                    };
                    serde_json::json!({
                        "error": "tool_call_arguments_truncated",
                        "original_args_preview": &args[..safe_end],
                        "message": "The tool call arguments were truncated or invalid. Please retry with complete arguments."
                    })
                    .to_string()
                };
                ToolCallMessage {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: valid_args,
                }
            })
            .collect();
        self.chat_messages.push(ChatMessage::Assistant {
            content,
            reasoning_content: reasoning,
            tool_calls: Some(calls),
            thinking_signature: None,
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
            tracing::debug!(
                removed,
                remaining = self.chat_messages.len(),
                "ephemeral messages cleaned up"
            );
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
            .filter_map(|(i, m)| {
                if matches!(m, ChatMessage::User { .. }) {
                    Some(i)
                } else {
                    None
                }
            })
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
        let assistant_idx = self.chat_messages.iter().rposition(
            |m| matches!(m, ChatMessage::Assistant { tool_calls: Some(tc), .. } if !tc.is_empty()),
        );

        let Some(assistant_idx) = assistant_idx else {
            return;
        };

        let ChatMessage::Assistant {
            tool_calls: Some(tc),
            ..
        } = &self.chat_messages[assistant_idx]
        else {
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
                    tool_calls: Some(tc),
                    ..
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
                tool_calls: Some(tc),
                ..
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
        s.push_assistant_tool_calls(&[("id1".into(), "tool".into(), "{}".into())], None, None);
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
        assert!(
            matches!(s.chat_messages()[1], ChatMessage::User { ref content, .. } if content == "u2")
        );
    }

    #[test]
    fn test_trim_oldest_turns_with_tool_calls() {
        let mut s = make_session();
        // Turn 1 with tool call
        s.push_message(MessageRole::User, "u1");
        s.push_assistant_tool_calls(&[("id1".into(), "t".into(), "{}".into())], None, None);
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
        s.push_assistant_tool_calls(&[("id1".into(), "t".into(), "{}".into())], None, None);
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

    // ── B5: remaining session lifecycle paths ──────────────────────────────

    #[test]
    fn test_id_and_action_allowlist() {
        let mut s = make_session();
        assert_eq!(s.id(), Some(SessionId::new(1)));
        assert!(!s.is_action_allowed("approve:rm"));
        s.allow_action("approve:rm");
        assert!(s.is_action_allowed("approve:rm"));
        assert!(!s.is_action_allowed("approve:shell"));
    }

    #[test]
    fn test_chat_messages_mut() {
        let mut s = make_session();
        s.chat_messages_mut().push(ChatMessage::user("direct"));
        assert_eq!(s.chat_messages().len(), 1);
    }

    #[test]
    fn test_push_message_tool_role() {
        let mut s = make_session();
        s.push_message(MessageRole::Tool, "result");
        assert!(matches!(s.chat_messages()[0], ChatMessage::Tool { .. }));
    }

    #[test]
    fn test_push_assistant_with_reasoning() {
        let mut s = make_session();
        s.push_assistant_with_reasoning("answer", "thinking");
        match &s.chat_messages()[0] {
            ChatMessage::Assistant {
                content,
                reasoning_content,
                ..
            } => {
                assert_eq!(content.as_deref(), Some("answer"));
                assert_eq!(reasoning_content.as_deref(), Some("thinking"));
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn test_push_user_message_with_images() {
        let mut s = make_session();
        s.push_user_message_with_images(
            "look",
            vec![ImageAttachment::Url {
                url: "http://x".into(),
                detail: None,
            }],
        );
        match &s.chat_messages()[0] {
            ChatMessage::User { images, .. } => assert_eq!(images.len(), 1),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn test_push_assistant_tool_call_singular() {
        let mut s = make_session();
        s.push_assistant_tool_call("call_1", "bash", "{}");
        match &s.chat_messages()[0] {
            ChatMessage::Assistant {
                tool_calls: Some(tc),
                ..
            } => {
                assert_eq!(tc.len(), 1);
                assert_eq!(tc[0].id, "call_1");
                assert_eq!(tc[0].name, "bash");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn test_simple_messages_filters_empty_content_tool_calls() {
        let mut s = make_session();
        s.chat_messages_mut().push(ChatMessage::Assistant {
            content: Some(String::new()),
            reasoning_content: None,
            tool_calls: Some(vec![ToolCallMessage {
                id: "c".into(),
                name: "t".into(),
                arguments: "{}".into(),
            }]),
            thinking_signature: None,
        });
        assert!(s.simple_messages().is_empty());
    }

    #[test]
    fn test_remove_ephemeral_messages() {
        let mut s = make_session();
        s.push_message(MessageRole::System, "keep");
        s.chat_messages_mut()
            .push(ChatMessage::user_ephemeral("temp"));
        s.chat_messages_mut()
            .push(ChatMessage::system_ephemeral("temp2"));
        s.push_message(MessageRole::User, "keep2");
        assert_eq!(s.chat_messages().len(), 4);
        s.remove_ephemeral_messages();
        assert_eq!(s.chat_messages().len(), 2);
        assert!(s.chat_messages().iter().all(|m| !m.is_ephemeral()));
    }

    #[test]
    fn test_close_dangling_tool_calls_noop_without_tool_call() {
        let mut s = make_session();
        s.push_message(MessageRole::User, "hi");
        s.push_message(MessageRole::Assistant, "hi");
        s.close_dangling_tool_calls("failed");
        assert_eq!(s.chat_messages().len(), 2);
    }

    #[test]
    fn test_close_dangling_tool_calls_adds_missing_results() {
        let mut s = make_session();
        s.push_message(MessageRole::User, "do");
        s.push_assistant_tool_calls(
            &[
                ("c1".into(), "t".into(), "{}".into()),
                ("c2".into(), "t".into(), "{}".into()),
            ],
            None,
            None,
        );
        s.push_tool_result("c1", "ok"); // only c1 answered
        s.close_dangling_tool_calls("failed");

        let tool_results: Vec<(String, String)> = s
            .chat_messages()
            .iter()
            .filter_map(|m| match m {
                ChatMessage::Tool {
                    tool_call_id,
                    name: _,
                    content,
                } => Some((tool_call_id.clone(), content.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(tool_results.len(), 2);
        assert!(
            tool_results
                .iter()
                .any(|(id, c)| id == "c2" && c == "failed")
        );
    }

    #[test]
    fn test_set_chat_messages_recalculates_total_tool_calls() {
        let mut s = make_session();
        let msgs = vec![
            ChatMessage::user("do"),
            ChatMessage::assistant_tool_call("c1", "t", "{}"),
            ChatMessage::tool("c1", "result"),
        ];
        s.set_chat_messages(msgs).unwrap();
        assert_eq!(s.total_tool_calls, 1);
    }
}

#[cfg(test)]
mod validate_tests {
    use super::*;

    #[test]
    fn test_valid_simple_sequence() {
        let msgs = vec![ChatMessage::user("hello"), ChatMessage::assistant("hi")];
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
                thinking_signature: None,
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
        let msgs = vec![ChatMessage::user("hello"), ChatMessage::assistant("hi")];
        assert!(s.set_chat_messages(msgs.clone()).is_ok());
        assert_eq!(s.chat_messages().len(), 2);
    }

    #[test]
    fn test_set_chat_messages_invalid() {
        let mut s = make_session();
        let msgs = vec![ChatMessage::tool("call_1", "orphaned")];
        assert!(s.set_chat_messages(msgs).is_err());
    }

    // ── RunState tests ────────────────────────────────────────────────────

    #[test]
    fn run_state_default() {
        let rs = RunState::default();
        assert_eq!(rs.turn_tool_calls, 0);
        assert!(!rs.run_has_tool_calls);
        assert_eq!(rs.reasoning_only_strikes, 0);
        assert_eq!(rs.empty_response_strikes, 0);
        assert_eq!(rs.nudge_count, 0);
    }

    #[test]
    fn run_state_reset_for_new_run() {
        let mut rs = RunState {
            turn_tool_calls: 5,
            run_has_tool_calls: true,
            reasoning_only_strikes: 2,
            empty_response_strikes: 1,
            nudge_count: 3,
        };

        rs.reset_for_new_run();

        assert_eq!(rs.turn_tool_calls, 0);
        assert!(!rs.run_has_tool_calls);
        assert_eq!(rs.reasoning_only_strikes, 0);
        assert_eq!(rs.empty_response_strikes, 0);
        assert_eq!(rs.nudge_count, 0);
    }

    #[test]
    fn run_state_record_tool_calls() {
        let mut rs = RunState {
            reasoning_only_strikes: 2,
            empty_response_strikes: 1,
            ..RunState::default()
        };

        rs.record_tool_calls(3);

        assert_eq!(rs.turn_tool_calls, 3);
        assert!(rs.run_has_tool_calls);
        assert_eq!(rs.reasoning_only_strikes, 0); // reset
        assert_eq!(rs.empty_response_strikes, 0); // reset
    }

    #[test]
    fn run_state_record_tool_calls_accumulates() {
        let mut rs = RunState::default();
        rs.record_tool_calls(2);
        rs.record_tool_calls(3);

        assert_eq!(rs.turn_tool_calls, 5);
        assert!(rs.run_has_tool_calls);
    }

    #[test]
    fn run_state_record_reasoning_only() {
        let mut rs = RunState {
            empty_response_strikes: 2,
            ..RunState::default()
        };

        let strikes = rs.record_reasoning_only();

        assert_eq!(strikes, 1);
        assert_eq!(rs.reasoning_only_strikes, 1);
        assert_eq!(rs.empty_response_strikes, 0); // reset
    }

    #[test]
    fn run_state_record_reasoning_only_consecutive() {
        let mut rs = RunState::default();

        assert_eq!(rs.record_reasoning_only(), 1);
        assert_eq!(rs.record_reasoning_only(), 2);
        assert_eq!(rs.record_reasoning_only(), 3);
    }

    #[test]
    fn run_state_record_empty_response() {
        let mut rs = RunState {
            reasoning_only_strikes: 2,
            ..RunState::default()
        };

        let strikes = rs.record_empty_response();

        assert_eq!(strikes, 1);
        assert_eq!(rs.empty_response_strikes, 1);
        assert_eq!(rs.reasoning_only_strikes, 0); // reset
    }

    #[test]
    fn run_state_record_empty_response_consecutive() {
        let mut rs = RunState::default();

        assert_eq!(rs.record_empty_response(), 1);
        assert_eq!(rs.record_empty_response(), 2);
        assert_eq!(rs.record_empty_response(), 3);
    }

    #[test]
    fn run_state_branch_cross_reset() {
        // Simulate: reasoning only → tool calls → reasoning only
        let mut rs = RunState::default();

        // Branch 1: reasoning only
        rs.record_reasoning_only();
        assert_eq!(rs.reasoning_only_strikes, 1);

        // Branch 3: tool calls (should reset reasoning_only_strikes)
        rs.record_tool_calls(2);
        assert_eq!(rs.reasoning_only_strikes, 0);
        assert_eq!(rs.turn_tool_calls, 2);

        // Branch 1 again: reasoning only (should start from 1, not 2)
        let strikes = rs.record_reasoning_only();
        assert_eq!(strikes, 1);
    }

    #[test]
    fn run_state_empty_to_reasoning_reset() {
        // Simulate: empty → empty → reasoning only (should reset empty strikes)
        let mut rs = RunState::default();

        rs.record_empty_response();
        rs.record_empty_response();
        assert_eq!(rs.empty_response_strikes, 2);

        // Branch 1: reasoning only (should reset empty_response_strikes)
        rs.record_reasoning_only();
        assert_eq!(rs.empty_response_strikes, 0);
        assert_eq!(rs.reasoning_only_strikes, 1);
    }

    // ── Backward-compatible deserialization ────────────────────────────────

    #[test]
    fn deserialize_legacy_flat_fields() {
        // Old format: nudge_count, turn_tool_calls, etc. as flat fields
        let json = r#"{
            "id": null,
            "chat_messages": [],
            "always_allowed_actions": [],
            "total_tool_calls": 5,
            "nudge_count": 3,
            "turn_tool_calls": 2,
            "reasoning_only_strikes": 1,
            "empty_response_strikes": 0
        }"#;
        let session: AgentSession = serde_json::from_str(json).unwrap();
        assert_eq!(session.run_state.nudge_count, 3);
        assert_eq!(session.run_state.turn_tool_calls, 2);
        assert_eq!(session.run_state.reasoning_only_strikes, 1);
        assert_eq!(session.run_state.empty_response_strikes, 0);
        assert!(!session.run_state.run_has_tool_calls); // default
    }

    #[test]
    fn deserialize_new_run_state_format() {
        // New format: nested run_state
        let json = r#"{
            "id": null,
            "chat_messages": [],
            "always_allowed_actions": [],
            "total_tool_calls": 5,
            "run_state": {
                "turn_tool_calls": 4,
                "run_has_tool_calls": true,
                "reasoning_only_strikes": 0,
                "empty_response_strikes": 1,
                "nudge_count": 2
            }
        }"#;
        let session: AgentSession = serde_json::from_str(json).unwrap();
        assert_eq!(session.run_state.turn_tool_calls, 4);
        assert!(session.run_state.run_has_tool_calls);
        assert_eq!(session.run_state.empty_response_strikes, 1);
        assert_eq!(session.run_state.nudge_count, 2);
    }

    #[test]
    fn deserialize_run_state_takes_precedence_over_flat() {
        // When both are present, run_state wins
        let json = r#"{
            "id": null,
            "chat_messages": [],
            "always_allowed_actions": [],
            "total_tool_calls": 0,
            "run_state": {
                "turn_tool_calls": 10,
                "run_has_tool_calls": true,
                "reasoning_only_strikes": 0,
                "empty_response_strikes": 0,
                "nudge_count": 0
            },
            "nudge_count": 99,
            "turn_tool_calls": 99
        }"#;
        let session: AgentSession = serde_json::from_str(json).unwrap();
        assert_eq!(session.run_state.turn_tool_calls, 10); // run_state wins
        assert_eq!(session.run_state.nudge_count, 0); // run_state wins
    }

    #[test]
    fn deserialize_legacy_missing_optional_fields() {
        // Old format with some fields missing (defaults to 0)
        let json = r#"{
            "id": null,
            "chat_messages": [],
            "always_allowed_actions": [],
            "total_tool_calls": 0,
            "nudge_count": 1
        }"#;
        let session: AgentSession = serde_json::from_str(json).unwrap();
        assert_eq!(session.run_state.nudge_count, 1);
        assert_eq!(session.run_state.turn_tool_calls, 0); // missing → 0
        assert_eq!(session.run_state.reasoning_only_strikes, 0);
        assert_eq!(session.run_state.empty_response_strikes, 0);
    }

    #[test]
    fn roundtrip_preserves_run_state() {
        let mut session = AgentSession::new(SessionId::new(1));
        session.run_state.nudge_count = 5;
        session.run_state.turn_tool_calls = 3;
        session.run_state.run_has_tool_calls = true;
        session.run_state.reasoning_only_strikes = 2;

        let json = serde_json::to_string(&session).unwrap();
        let restored: AgentSession = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.run_state.nudge_count, 5);
        assert_eq!(restored.run_state.turn_tool_calls, 3);
        assert!(restored.run_state.run_has_tool_calls);
        assert_eq!(restored.run_state.reasoning_only_strikes, 2);
    }

    #[test]
    fn push_assistant_tool_calls_validates_json_args() {
        let mut s = make_session();

        // Valid JSON args should pass through unchanged
        let valid_args = r#"{"path": "src/main.rs", "content": "fn main() {}"}"#;
        s.push_assistant_tool_calls(
            &[("id1".into(), "write_file".into(), valid_args.into())],
            None,
            None,
        );
        if let ChatMessage::Assistant { tool_calls: Some(ref tc), .. } = s.chat_messages[0] {
            assert_eq!(tc[0].arguments, valid_args);
        } else {
            panic!("expected Assistant message with tool_calls");
        }

        // Truncated (invalid JSON) args should be wrapped in an error object
        let truncated_args = r#"{"path": "src/ui/markdown.rs", "content": "#;
        s.push_assistant_tool_calls(
            &[("id2".into(), "write_file".into(), truncated_args.into())],
            None,
            None,
        );
        if let ChatMessage::Assistant { tool_calls: Some(ref tc), .. } = s.chat_messages[1] {
            // The arguments should now be valid JSON (the error wrapper)
            let parsed: serde_json::Value = serde_json::from_str(&tc[0].arguments)
                .expect("wrapped arguments should be valid JSON");
            assert_eq!(parsed["error"], "tool_call_arguments_truncated");
            assert!(parsed["message"].as_str().unwrap().contains("truncated"));
        } else {
            panic!("expected Assistant message with tool_calls");
        }
    }

    #[test]
    fn push_assistant_tool_calls_truncated_multibyte_no_panic() {
        // Bug-2: Invalid JSON args with multi-byte UTF-8 chars near byte 200
        // cause a panic at char boundary when slicing &args[..args.len().min(200)].
        let mut s = make_session();

        // Build invalid JSON with CJK chars that straddle the 200-byte boundary.
        // "あ" = 3 bytes in UTF-8. Repeating ~70 times = ~210 bytes, then add invalid suffix.
        let mut bad_args = "あ".repeat(70); // 70 * 3 = 210 bytes
        bad_args.push_str("truncated");    // makes it invalid JSON

        // This must NOT panic — the preview slice should respect char boundaries.
        s.push_assistant_tool_calls(
            &[("id1".into(), "tool".into(), bad_args.into())],
            None,
            None,
        );

        if let ChatMessage::Assistant { tool_calls: Some(ref tc), .. } = s.chat_messages[0] {
            // Should be wrapped in error object (valid JSON)
            let parsed: serde_json::Value = serde_json::from_str(&tc[0].arguments)
                .expect("wrapped arguments should be valid JSON even with multibyte chars");
            assert_eq!(parsed["error"], "tool_call_arguments_truncated");
        } else {
            panic!("expected Assistant message with tool_calls");
        }
    }
}

#[cfg(test)]
mod proptest_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        // ── RunState property tests ─────────────────────────────────────────

        #[test]
        fn reset_for_new_run_zeros_all_fields(
            turn_tool_calls in 0usize..1000,
            run_has_tool_calls in proptest::bool::ANY,
            reasoning_only_strikes in 0usize..100,
            empty_response_strikes in 0usize..100,
            nudge_count in 0usize..100,
        ) {
            let mut rs = RunState {
                turn_tool_calls,
                run_has_tool_calls,
                reasoning_only_strikes,
                empty_response_strikes,
                nudge_count,
            };
            rs.reset_for_new_run();
            assert_eq!(rs.turn_tool_calls, 0);
            assert!(!rs.run_has_tool_calls);
            assert_eq!(rs.reasoning_only_strikes, 0);
            assert_eq!(rs.empty_response_strikes, 0);
            assert_eq!(rs.nudge_count, 0);
        }

        #[test]
        fn record_tool_calls_accumulates(n in 0usize..100) {
            let mut rs = RunState::default();
            rs.record_tool_calls(n);
            assert_eq!(rs.turn_tool_calls, n);
            assert!(rs.run_has_tool_calls);
            assert_eq!(rs.reasoning_only_strikes, 0);
            assert_eq!(rs.empty_response_strikes, 0);
        }

        #[test]
        fn record_reasoning_only_increments(count in 1usize..50) {
            let mut rs = RunState::default();
            for i in 1..=count {
                let strikes = rs.record_reasoning_only();
                assert_eq!(strikes, i);
                assert_eq!(rs.empty_response_strikes, 0);
            }
        }

        #[test]
        fn record_empty_response_increments(count in 1usize..50) {
            let mut rs = RunState::default();
            for i in 1..=count {
                let strikes = rs.record_empty_response();
                assert_eq!(strikes, i);
                assert_eq!(rs.reasoning_only_strikes, 0);
            }
        }

        // ── push_assistant_tool_calls property tests ────────────────────────

        #[test]
        fn push_assistant_tool_calls_valid_json_unchanged(args in r"\{[^{}]{0,200}\}") {
            // Only test strings that are actually valid JSON objects
            if serde_json::from_str::<serde_json::Value>(&args).is_err() {
                return Ok(());
            }
            let mut s = make_session();
            s.push_assistant_tool_calls(
                &[("id".into(), "tool".into(), args.clone())],
                None,
                None,
            );
            if let ChatMessage::Assistant { tool_calls: Some(ref tc), .. } = s.chat_messages()[0] {
                assert_eq!(tc[0].arguments, args);
            } else {
                panic!("expected Assistant with tool_calls");
            }
        }

        #[test]
        fn push_assistant_tool_calls_invalid_json_wrapped_safely(
            bad_args in "[a-z\u{4e00}-\u{9fff}]{0,300}"
        ) {
            // Skip if it happens to be valid JSON
            if serde_json::from_str::<serde_json::Value>(&bad_args).is_ok() {
                return Ok(());
            }
            let mut s = make_session();
            s.push_assistant_tool_calls(
                &[("id".into(), "tool".into(), bad_args)],
                None,
                None,
            );
            if let ChatMessage::Assistant { tool_calls: Some(ref tc), .. } = s.chat_messages()[0] {
                let parsed: serde_json::Value = serde_json::from_str(&tc[0].arguments)
                    .expect("wrapped args must be valid JSON");
                assert_eq!(parsed["error"], "tool_call_arguments_truncated");
            } else {
                panic!("expected Assistant with tool_calls");
            }
        }

        // ── trim_oldest_turns property tests ────────────────────────────────

        #[test]
        fn trim_oldest_turns_never_exceeds_max(turns in 1usize..20, max in 1usize..20) {
            let mut s = make_session();
            for i in 0..turns {
                s.push_message(MessageRole::User, format!("u{}", i));
                s.push_message(MessageRole::Assistant, format!("a{}", i));
            }
            s.trim_oldest_turns(max);
            assert!(s.turn_count() <= max || turns <= max);
        }

        // ── validate_message_sequence property tests ────────────────────────

        #[test]
        fn validate_simple_user_assistant_always_passes(count in 1usize..20) {
            let mut msgs = Vec::new();
            for i in 0..count {
                msgs.push(ChatMessage::user(format!("msg{}", i)));
                msgs.push(ChatMessage::assistant(format!("reply{}", i)));
            }
            assert!(validate_message_sequence(&msgs).is_ok());
        }
    }
}
