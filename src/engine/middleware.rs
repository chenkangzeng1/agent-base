use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::types::{AgentResult, AgentEvent, ChatMessage, SessionId};

#[derive(Clone)]
pub struct UserMessageCtx {
    pub session_id: SessionId,
    pub user_input: String,
    pub event_bus: broadcast::Sender<AgentEvent>,
}

#[derive(Clone)]
pub struct PreLlmCtx {
    pub session_id: SessionId,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<Value>,
    pub event_bus: broadcast::Sender<AgentEvent>,
}

#[derive(Clone)]
pub struct PostLlmCtx {
    pub session_id: SessionId,
    pub full_text: String,
    pub is_tool_call: bool,
    pub tool_calls: Vec<(String, String, String)>,
    pub event_bus: broadcast::Sender<AgentEvent>,
    pub available_tools: Vec<String>,
    pub turn_count: u32,
    pub total_tool_calls: usize,
    pub skip_push: bool,
    pub follow_up_message: Option<String>,
}

#[async_trait]
pub trait Middleware: Send + Sync {
    async fn on_user_message(&self, _ctx: &mut UserMessageCtx) -> AgentResult<()> {
        Ok(())
    }

    async fn on_pre_llm(&self, _ctx: &mut PreLlmCtx) -> AgentResult<()> {
        Ok(())
    }

    async fn on_post_llm(&self, _ctx: &mut PostLlmCtx) -> AgentResult<()> {
        Ok(())
    }
}

pub(crate) type MiddlewareRef = Arc<dyn Middleware>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SessionId;

    #[test]
    fn test_post_llm_ctx_new_fields_defaults() {
        let (tx, _rx) = broadcast::channel(1);
        let ctx = PostLlmCtx {
            session_id: SessionId { id: 1, external_id: None },
            full_text: "test".to_string(),
            is_tool_call: false,
            tool_calls: vec![],
            event_bus: tx,
            available_tools: vec![],
            turn_count: 0,
            total_tool_calls: 0,
            skip_push: false,
            follow_up_message: None,
        };
        assert!(ctx.available_tools.is_empty());
        assert_eq!(ctx.turn_count, 0);
        assert_eq!(ctx.total_tool_calls, 0);
        assert!(!ctx.skip_push);
        assert!(ctx.follow_up_message.is_none());
    }

    #[test]
    fn test_post_llm_ctx_skip_push_follow_up_set() {
        let (tx, _rx) = broadcast::channel(1);
        let ctx = PostLlmCtx {
            session_id: SessionId { id: 2, external_id: None },
            full_text: "I will execute...".to_string(),
            is_tool_call: false,
            tool_calls: vec![],
            event_bus: tx,
            available_tools: vec!["echo".to_string()],
            turn_count: 1,
            total_tool_calls: 0,
            skip_push: true,
            follow_up_message: Some("Please call tools now.".to_string()),
        };
        assert!(ctx.skip_push);
        assert_eq!(ctx.follow_up_message, Some("Please call tools now.".to_string()));
        assert_eq!(ctx.available_tools, vec!["echo".to_string()]);
        assert_eq!(ctx.total_tool_calls, 0);
    }

    #[test]
    fn test_post_llm_ctx_clone_preserves_new_fields() {
        let (tx, _rx) = broadcast::channel(1);
        let ctx = PostLlmCtx {
            session_id: SessionId { id: 3, external_id: None },
            full_text: "hello".to_string(),
            is_tool_call: false,
            tool_calls: vec![],
            event_bus: tx,
            available_tools: vec!["add".to_string(), "subtract".to_string()],
            turn_count: 5,
            total_tool_calls: 3,
            skip_push: true,
            follow_up_message: Some("nudge".to_string()),
        };
        let cloned = ctx.clone();
        assert_eq!(cloned.available_tools, vec!["add", "subtract"]);
        assert_eq!(cloned.turn_count, 5);
        assert_eq!(cloned.total_tool_calls, 3);
        assert!(cloned.skip_push);
        assert_eq!(cloned.follow_up_message, Some("nudge".to_string()));
    }
}
