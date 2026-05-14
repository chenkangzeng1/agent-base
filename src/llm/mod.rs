use async_trait::async_trait;
use futures_core::Stream;
use serde_json::Value;
use std::pin::Pin;

use crate::types::{AgentResult, ChatMessage};

mod openai;

pub use openai::OpenAiClient;

#[derive(Clone, Debug)]
pub enum StreamChunk {
    Text(String),
    Thought(String),
    ToolCall(Value),
    Stop,
}

#[derive(Clone, Debug, Default)]
pub struct LlmCapabilities {
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub supports_thinking: bool,
    pub max_context_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        enable_thinking: Option<bool>,
    ) -> AgentResult<Value>;

    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        enable_thinking: Option<bool>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>>;

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities::default()
    }
}
