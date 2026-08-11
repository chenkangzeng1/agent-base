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

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::StreamChunk;
    use crate::types::AgentError;
    use async_trait::async_trait;
    use futures_util::StreamExt;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::task::{Context, Poll};

    // ── Mock clients ──

    /// Mock LlmClient that returns canned JSON responses for `chat()`
    /// and a canned stream for `chat_stream()`.
    struct MockLlmClient {
        chat_response: Mutex<Option<Value>>,
        stream_chunks: Mutex<Option<Vec<AgentResult<StreamChunk>>>>,
        caps: LlmCapabilities,
        model: String,
    }

    impl MockLlmClient {
        fn new() -> Self {
            Self {
                chat_response: Mutex::new(None),
                stream_chunks: Mutex::new(None),
                caps: LlmCapabilities {
                    supports_streaming: true,
                    supports_tools: true,
                    supports_vision: false,
                    supports_thinking: true,
                    max_context_tokens: Some(128_000),
                    max_output_tokens: Some(16_384),
                },
                model: "mock-model".into(),
            }
        }

        fn with_chat_response(response: Value) -> Self {
            Self {
                chat_response: Mutex::new(Some(response)),
                ..Self::new()
            }
        }

        fn with_stream(chunks: Vec<AgentResult<StreamChunk>>) -> Self {
            Self {
                stream_chunks: Mutex::new(Some(chunks)),
                ..Self::new()
            }
        }
    }

    #[async_trait]
    impl LlmClient for MockLlmClient {
        async fn chat(
            &self,
            _messages: &[ChatMessage],
            _tools: &[Value],
            _reasoning: Option<&ReasoningConfig>,
            _response_format: Option<&ResponseFormat>,
        ) -> AgentResult<Value> {
            self.chat_response
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| AgentError::internal("no chat response set"))
        }

        async fn chat_stream(
            &self,
            _messages: &[ChatMessage],
            _tools: &[Value],
            _reasoning: Option<&ReasoningConfig>,
            _response_format: Option<&ResponseFormat>,
        ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
            let chunks: Vec<AgentResult<StreamChunk>> = self
                .stream_chunks
                .lock()
                .unwrap()
                .take()
                .unwrap_or_default();
            Ok(Box::pin(futures_util::stream::iter(chunks)))
        }

        fn capabilities(&self) -> LlmCapabilities {
            self.caps.clone()
        }

        fn model_name(&self) -> &str {
            &self.model
        }
    }

    /// Minimal StreamClient that yields a canned stream for testing the
    /// default `chat()` implementation.
    struct StubStreamClient {
        chunks: Mutex<Option<Vec<AgentResult<StreamChunk>>>>,
        caps: LlmCapabilities,
    }

    impl StubStreamClient {
        fn with_chunks(chunks: Vec<StreamChunk>) -> Self {
            Self {
                chunks: Mutex::new(Some(chunks.into_iter().map(Ok).collect())),
                caps: LlmCapabilities::default(),
            }
        }
    }

    #[async_trait]
    impl StreamClient for StubStreamClient {
        async fn stream(
            &self,
            _messages: &[ChatMessage],
            _tools: &[Value],
            _reasoning: Option<&ReasoningConfig>,
            _response_format: Option<&ResponseFormat>,
        ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
            let chunks: Vec<AgentResult<StreamChunk>> = self
                .chunks
                .lock()
                .unwrap()
                .take()
                .unwrap_or_default();
            Ok(Box::pin(futures_util::stream::iter(chunks)))
        }

        fn capabilities(&self) -> LlmCapabilities {
            self.caps.clone()
        }
    }

    // ── adapt() ──

    #[test]
    fn adapt_wraps_llm_client() {
        let client = Arc::new(MockLlmClient::new());
        let stream_client = adapt(client);
        // adapt() returns an Arc<dyn StreamClient> — just verify it builds.
        let _ = stream_client;
    }

    // ── LlmClientAdapter::chat() ──

    #[tokio::test]
    async fn adapter_chat_extracts_content() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "hello from mock"
                }
            }]
        });
        let client = Arc::new(MockLlmClient::with_chat_response(response));
        let adapter = LlmClientAdapter::new(client);
        let text = adapter
            .chat(&[], &[], None, None)
            .await
            .expect("chat should succeed");
        assert_eq!(text, "hello from mock");
    }

    #[tokio::test]
    async fn adapter_chat_handles_missing_content() {
        // Response with no choices array
        let response = serde_json::json!({"error": "something went wrong"});
        let client = Arc::new(MockLlmClient::with_chat_response(response));
        let adapter = LlmClientAdapter::new(client);
        let text = adapter
            .chat(&[], &[], None, None)
            .await
            .expect("chat should succeed with empty string");
        assert_eq!(text, "");
    }

    #[tokio::test]
    async fn adapter_chat_handles_empty_choices() {
        let response = serde_json::json!({"choices": []});
        let client = Arc::new(MockLlmClient::with_chat_response(response));
        let adapter = LlmClientAdapter::new(client);
        let text = adapter
            .chat(&[], &[], None, None)
            .await
            .expect("chat should succeed with empty string");
        assert_eq!(text, "");
    }

    #[tokio::test]
    async fn adapter_chat_handles_null_content() {
        // Tool-call response: content is null
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{"id": "call_1", "function": {"name": "shell"}}]
                }
            }]
        });
        let client = Arc::new(MockLlmClient::with_chat_response(response));
        let adapter = LlmClientAdapter::new(client);
        let text = adapter
            .chat(&[], &[], None, None)
            .await
            .expect("chat should succeed with empty string");
        assert_eq!(text, "");
    }

    // ── LlmClientAdapter::stream() ──

    #[tokio::test]
    async fn adapter_stream_delegates_to_inner() {
        let chunks = vec![
            Ok(StreamChunk::Text("hello ".into())),
            Ok(StreamChunk::Text("world".into())),
            Ok(StreamChunk::Stop {
                finish_reason: Some("stop".into()),
            }),
        ];
        let client = Arc::new(MockLlmClient::with_stream(chunks));
        let adapter = LlmClientAdapter::new(client);
        let mut stream = adapter
            .stream(&[], &[], None, None)
            .await
            .expect("stream should succeed");
        let mut texts = Vec::new();
        while let Some(chunk) = stream.next().await {
            if let Ok(StreamChunk::Text(t)) = chunk {
                texts.push(t);
            }
        }
        assert_eq!(texts, vec!["hello ", "world"]);
    }

    // ── LlmClientAdapter::capabilities() ──

    #[test]
    fn adapter_capabilities_delegates_to_inner() {
        let client = Arc::new(MockLlmClient::new());
        let caps = client.capabilities();
        let adapter = LlmClientAdapter::new(client);
        assert_eq!(adapter.capabilities().max_context_tokens, caps.max_context_tokens);
        assert_eq!(adapter.capabilities().supports_streaming, caps.supports_streaming);
    }

    // ── LlmClientAdapter::model_name() ──

    #[test]
    fn adapter_model_name_delegates_to_inner() {
        let client = Arc::new(MockLlmClient::new());
        let adapter = LlmClientAdapter::new(client);
        assert_eq!(adapter.model_name(), "mock-model");
    }

    #[test]
    fn default_model_name_is_unknown() {
        let client = StubStreamClient::with_chunks(vec![]);
        assert_eq!(client.model_name(), "unknown");
    }

    // ── LlmClientAdapter::inner() ──

    #[test]
    fn adapter_inner_returns_reference() {
        let client = Arc::new(MockLlmClient::new());
        let adapter = LlmClientAdapter::new(client);
        let inner: &Arc<dyn LlmClient> = adapter.inner();
        assert_eq!(inner.model_name(), "mock-model");
    }

    // ── Default StreamClient::chat() ──

    #[tokio::test]
    async fn default_chat_collects_text_deltas() {
        let chunks = vec![
            StreamChunk::Text("part1".into()),
            StreamChunk::Text("part2".into()),
            StreamChunk::Text("part3".into()),
            StreamChunk::Stop {
                finish_reason: Some("stop".into()),
            },
        ];
        let client = StubStreamClient::with_chunks(chunks);
        let text = client
            .chat(&[], &[], None, None)
            .await
            .expect("chat should succeed");
        assert_eq!(text, "part1part2part3");
    }

    #[tokio::test]
    async fn default_chat_stops_on_stop_chunk() {
        // Text after Stop should be ignored
        let chunks = vec![
            StreamChunk::Text("before".into()),
            StreamChunk::Stop {
                finish_reason: Some("stop".into()),
            },
            StreamChunk::Text("after".into()),
        ];
        let client = StubStreamClient::with_chunks(chunks);
        let text = client
            .chat(&[], &[], None, None)
            .await
            .expect("chat should succeed");
        assert_eq!(text, "before");
    }

    #[tokio::test]
    async fn default_chat_ignores_non_text_chunks() {
        let chunks = vec![
            StreamChunk::Thought("thinking...".into()),
            StreamChunk::Text("visible".into()),
            StreamChunk::ToolCall(serde_json::json!({"name": "shell"})),
            StreamChunk::Stop {
                finish_reason: Some("stop".into()),
            },
        ];
        let client = StubStreamClient::with_chunks(chunks);
        let text = client
            .chat(&[], &[], None, None)
            .await
            .expect("chat should succeed");
        assert_eq!(text, "visible");
    }

    #[tokio::test]
    async fn default_chat_handles_stream_error() {
        let chunks = vec![
            Ok(StreamChunk::Text("before".into())),
            Err(AgentError::internal("stream broke")),
        ];
        let client = StubStreamClient {
            chunks: Mutex::new(Some(chunks)),
            caps: LlmCapabilities::default(),
        };
        let result = client.chat(&[], &[], None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("stream broke"));
    }

    #[tokio::test]
    async fn default_chat_empty_stream_returns_empty() {
        let client = StubStreamClient::with_chunks(vec![]);
        let text = client
            .chat(&[], &[], None, None)
            .await
            .expect("chat should succeed");
        assert_eq!(text, "");
    }

    // ── Send + Sync ──

    #[test]
    fn llm_client_adapter_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LlmClientAdapter>();
    }
}
