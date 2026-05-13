use async_trait::async_trait;
use futures_core::Stream;
use serde_json::Value;
use std::pin::Pin;

use crate::types::AgentResult;

mod openai;

pub use openai::OpenAiClient;

#[derive(Clone, Debug)]
pub enum StreamChunk {
    Text(String),
    Thought(String),
    ToolCall(Value),
    Stop,
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn chat(
        &self,
        messages: &[Value],
        tools: &[Value],
        enable_thinking: Option<bool>,
    ) -> AgentResult<Value>;

    async fn chat_stream(
        &self,
        messages: &[Value],
        tools: &[Value],
        enable_thinking: Option<bool>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>>;
}
