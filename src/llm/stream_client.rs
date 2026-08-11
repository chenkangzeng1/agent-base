use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::Value;

use super::{LlmCapabilities, LlmClient, ReasoningConfig, StreamChunk};
use crate::types::{AgentResult, ChatMessage, ResponseFormat};

/// Provider-agnostic streaming client trait.
///
/// This is the recommended interface for LLM provider integration.
/// Providers only need to implement [`stream`](StreamClient::stream) —
/// [`chat`](StreamClient::chat) has a default implementation that collects
/// text deltas from the stream.
///
/// This follows the Rust standard library convention:
/// [`Iterator`] only requires `next()`, [`std::io::Read`] only requires `read()`.
///
/// The older [`LlmClient`] trait is still supported via [`LlmClientAdapter`].
#[async_trait]
pub trait StreamClient: Send + Sync {
    /// Stream LLM response chunks from the provider.
    ///
    /// This is the **only required method**. Implementors translate their
    /// provider's SSE/streaming protocol into [`StreamChunk`] events.
    async fn stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        reasoning: Option<&ReasoningConfig>,
        response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>>;

    /// Convenience: collect all [`StreamChunk::Text`] deltas into a single string.
    ///
    /// The default implementation streams and concatenates text chunks.
    /// Providers may override this with a dedicated non-streaming API call
    /// for better latency or cost.
    async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        reasoning: Option<&ReasoningConfig>,
        response_format: Option<&ResponseFormat>,
    ) -> AgentResult<String> {
        let mut stream = self
            .stream(messages, tools, reasoning, response_format)
            .await?;
        let mut text = String::new();
        while let Some(chunk) = stream.next().await {
            match chunk? {
                StreamChunk::Text(t) => text.push_str(&t),
                StreamChunk::Stop { .. } => break,
                _ => {}
            }
        }
        Ok(text)
    }

    /// Return the provider's capabilities.
    fn capabilities(&self) -> LlmCapabilities;

    /// The model name used by this client (e.g. "claude-sonnet", "gpt-4o").
    /// Default: "unknown".
    fn model_name(&self) -> &str {
        "unknown"
    }
}

// ── LlmClient → StreamClient adapter ──

/// Bridges an [`LlmClient`] to the [`StreamClient`] trait.
///
/// Wraps an `Arc<dyn LlmClient>` so existing provider implementations
/// can be used with the new [`StreamClient`]-based engine.
pub struct LlmClientAdapter {
    inner: Arc<dyn LlmClient>,
}

impl LlmClientAdapter {
    /// Wrap an existing [`LlmClient`] for use as a [`StreamClient`].
    pub fn new(client: Arc<dyn LlmClient>) -> Self {
        Self { inner: client }
    }

    /// Get a reference to the inner [`LlmClient`].
    pub fn inner(&self) -> &Arc<dyn LlmClient> {
        &self.inner
    }
}

#[async_trait]
impl StreamClient for LlmClientAdapter {
    async fn stream(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        reasoning: Option<&ReasoningConfig>,
        response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        self.inner
            .chat_stream(messages, tools, reasoning, response_format)
            .await
    }

    /// Delegate to the inner client's `chat()` for efficiency — avoids
    /// streaming when only the final text is needed.
    async fn chat(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        reasoning: Option<&ReasoningConfig>,
        response_format: Option<&ResponseFormat>,
    ) -> AgentResult<String> {
        let result = self
            .inner
            .chat(messages, tools, reasoning, response_format)
            .await?;
        // Extract text from the chat completion response
        let text = result
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|msg| msg.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        Ok(text)
    }

    fn capabilities(&self) -> LlmCapabilities {
        self.inner.capabilities()
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
}

/// Convenience: wrap an `Arc<dyn LlmClient>` as an `Arc<dyn StreamClient>`.
///
/// This is the primary migration path for existing [`LlmClient`] implementations.
/// Use this when you have a legacy client and need to pass it to APIs that
/// expect [`StreamClient`].
pub fn adapt(client: Arc<dyn LlmClient>) -> Arc<dyn StreamClient> {
    Arc::new(LlmClientAdapter::new(client))
}
