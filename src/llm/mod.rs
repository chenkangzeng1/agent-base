use async_trait::async_trait;
use futures_core::Stream;
use serde_json::Value;
use std::pin::Pin;

use crate::types::{AgentResult, ChatMessage, ResponseFormat};

mod anthropic;
mod openai;
mod registry;

pub use anthropic::AnthropicClient;
pub use openai::OpenAiClient;
pub use registry::{LlmClientBuilder, LlmProvider};

#[derive(Clone, Debug)]
pub enum StreamChunk {
    Text(String),
    Thought(String),
    ToolCall(Value),
    Usage(UsageInfo),
    Stop,
}

#[derive(Clone, Debug, Default)]
pub struct UsageInfo {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
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
        response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value>;

    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        enable_thinking: Option<bool>,
        response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>>;

    fn capabilities(&self) -> LlmCapabilities;
}
