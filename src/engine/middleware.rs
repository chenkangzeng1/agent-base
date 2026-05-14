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
