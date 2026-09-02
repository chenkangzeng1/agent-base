use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::Value;
use tracing::Span;

use crate::engine::runtime::event_bus::EventBus;
use crate::llm::{ReasoningConfig, StreamChunk, UsageInfo};
use crate::types::{AgentResult, ChatMessage, RuntimeEvent, SessionId};

/// Build a `ChatRequest` from individual parameters.
fn build_chat_request(
    messages: &[ChatMessage],
    tool_definitions: &[Value],
    reasoning: Option<&ReasoningConfig>,
    response_format: Option<&crate::types::ResponseFormat>,
    thinking_disabled: bool,
) -> llm_trait::ChatRequest {
    let mut request = llm_trait::ChatRequest::new(messages.to_vec());
    if !tool_definitions.is_empty() {
        request = request.with_tools(tool_definitions.to_vec());
    }
    // Only include reasoning if not disabled
    // This is a workaround for LLM providers that ignore budget_tokens limit
    if let Some(r) = reasoning {
        if !thinking_disabled {
            request = request.with_reasoning(r.clone());
        } else {
            tracing::info!(
                "thinking disabled for rest of run due to too many reasoning-only responses"
            );
        }
    }
    request.response_format = response_format.cloned();
    request
}

/// Convert a `ChatStream` (new) to the old stream type consumed by `process_stream`.
fn chat_stream_to_old(
    chat_stream: llm_trait::ChatStream,
) -> Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>> {
    let inner = chat_stream.into_inner();
    Box::pin(inner.map(|item| item.map_err(Into::into)))
}

pub struct LlmEngine {
    provider: RwLock<Arc<dyn llm_trait::LlmProvider>>,
    event_bus: EventBus,
}

impl LlmEngine {
    pub(crate) fn new(provider: Arc<dyn llm_trait::LlmProvider>, event_bus: EventBus) -> Self {
        Self {
            provider: RwLock::new(provider),
            event_bus,
        }
    }

    /// Get a clone of the current LLM provider.
    pub fn get_provider(&self) -> Arc<dyn llm_trait::LlmProvider> {
        self.provider.read().unwrap().clone()
    }

    /// Replace the LLM provider at runtime (e.g., model switch).
    pub fn set_provider(&self, provider: Arc<dyn llm_trait::LlmProvider>) {
        *self.provider.write().unwrap() = provider;
    }

    /// Replace the LLM client at runtime (e.g., model switch).
    ///
    /// Accepts an `Arc<dyn llm_trait::LlmProvider>`.
    pub fn set_client(&self, client: Arc<dyn llm_trait::LlmProvider>) {
        self.set_provider(client);
    }

    pub async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tool_definitions: &[Value],
        reasoning: Option<&ReasoningConfig>,
        response_format: Option<&crate::types::ResponseFormat>,
        thinking_disabled: bool,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        tracing::info!(
            msg_count = messages.len(),
            tool_count = tool_definitions.len(),
            thinking_disabled,
            "LLM chat_stream: sending request to API"
        );
        let request = build_chat_request(
            messages,
            tool_definitions,
            reasoning,
            response_format,
            thinking_disabled,
        );
        let result = self.get_provider().stream(request).await;
        match &result {
            Ok(_) => tracing::info!("LLM chat_stream: API response received"),
            Err(e) => tracing::error!(error = %e, "LLM chat_stream: API request failed"),
        }
        result.map(chat_stream_to_old).map_err(Into::into)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn run_llm_turn_with_retry(
        &self,
        session_id: &SessionId,
        messages: &[ChatMessage],
        tool_definitions: &[Value],
        reasoning: Option<&ReasoningConfig>,
        response_format: Option<&crate::types::ResponseFormat>,
        retry: crate::types::RetryConfig,
        thinking_disabled: bool,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        let request = build_chat_request(
            messages,
            tool_definitions,
            reasoning,
            response_format,
            thinking_disabled,
        );
        let mut attempt = 1;
        let mut delay_ms = retry.initial_backoff_ms;
        loop {
            match self.get_provider().stream(request.clone()).await {
                Ok(chat_stream) => return Ok(chat_stream_to_old(chat_stream)),
                Err(e) => {
                    let agent_err: crate::types::AgentError = e.into();
                    if attempt > retry.max_retries {
                        tracing::error!(session_id = session_id.id, attempts = attempt, error = %agent_err, "LLM stream retry exhausted");
                        return Err(agent_err);
                    }
                    tracing::warn!(session_id = session_id.id, attempt, error = %agent_err, "LLM stream failed, retrying...");
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    delay_ms = (delay_ms as f64 * retry.backoff_multiplier) as u64;
                    if delay_ms > retry.max_backoff_ms {
                        delay_ms = retry.max_backoff_ms;
                    }
                    attempt += 1;
                }
            }
        }
    }

    pub(crate) async fn process_stream<F>(
        &self,
        session_id: &SessionId,
        mut stream: Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>,
        span: Span,
        event_rx: &mut tokio::sync::broadcast::Receiver<RuntimeEvent>,
        on_event: Arc<std::sync::Mutex<F>>,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> AgentResult<LlmTurnResult>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let _enter = span.enter();
        let start = std::time::Instant::now();
        let mut first_token = true;
        let mut aggregator = StreamAggregator::new();
        tracing::info!(session_id = session_id.id, "LLM process_stream start");

        // Skip UserEvents that were already rendered by ctx.emit() during middleware
        // execution (compression Progress/Started/Completed). ctx.emit() sends to
        // both the renderer callback and the event bus — the bus copy arrives here
        // and must be skipped to avoid double-rendering.
        loop {
            match event_rx.try_recv() {
                Ok(RuntimeEvent::UserEvent { .. }) => continue,
                Ok(event) => {
                    // Non-UserEvent (e.g. Checkpoint) — push back by processing
                    // and let the main loop handle the next ones normally.
                    if let Ok(mut cb) = on_event.lock() {
                        cb(event)?;
                    }
                    continue;
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }

        loop {
            tokio::select! {
                recv_result = event_rx.recv() => {
                    match recv_result {
                        Ok(event) => {
                            if let Ok(mut cb) = on_event.lock() {
                                cb(event)?;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = cancel_token.cancelled() => {
                    tracing::info!(session_id = session_id.id, "LLM stream cancelled");
                    return Err(crate::types::AgentError::Cancelled);
                }
                maybe_chunk = stream.next() => {
                    let Some(chunk) = maybe_chunk else {
                        break;
                    };

                    match chunk {
                        Ok(StreamChunk::Text(text)) => {
                            if first_token {
                                let ttft = start.elapsed();
                                aggregator.ttft_ms = ttft.as_millis() as u64;
                                tracing::info!(session_id = session_id.id, ?ttft, "LLM first token received");
                                first_token = false;
                            }
                            if !text.is_empty() && !aggregator.is_tool_call {
                                self.event_bus.emit(RuntimeEvent::TextDelta {
                                    session_id: session_id.clone(),
                                    text: text.clone(),
                                    agent_id: None,
                                    trace_id: None,
                                });
                            }
                            aggregator.full_text.push_str(&text);
                        }
                        Ok(StreamChunk::Thought(text)) => {
                            tracing::debug!(session_id = session_id.id, len = text.len(), "llm thought chunk");
                            aggregator.reasoning_text.push_str(&text);
                            if !text.is_empty() && !aggregator.is_tool_call {
                                self.event_bus.emit(RuntimeEvent::ThoughtDelta {
                                    session_id: session_id.clone(),
                                    text,
                                    agent_id: None,
                                    trace_id: None,
                                });
                            }
                        }
                        Ok(StreamChunk::ToolCall(choice)) => {
                            if !aggregator.is_tool_call {
                                tracing::info!(session_id = session_id.id, "LLM stream: first tool_call chunk received");
                            }
                            aggregator.is_tool_call = true;
                            let Some(tool_calls) = choice
                                .get("delta")
                                .and_then(|d| d.get("tool_calls"))
                                .and_then(Value::as_array)
                            else {
                                tracing::debug!("tool_calls missing or not an array in LLM response");
                                continue;
                            };

                            for tool_call in tool_calls {
                                let idx = tool_call
                                    .get("index")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(0) as usize;
                                let entry = aggregator.partials.entry(idx).or_insert_with(|| (String::new(), String::new(), String::new()));
                                if let Some(id) = tool_call.get("id").and_then(Value::as_str)
                                    && !id.is_empty() {
                                        entry.0 = id.to_string();
                                    }
                                if let Some(func) = tool_call.get("function") {
                                    if let Some(name) = func.get("name").and_then(Value::as_str)
                                        && !name.is_empty() {
                                            entry.1 = name.to_string();
                                        }
                                    if let Some(args) = func.get("arguments").and_then(Value::as_str) {
                                        entry.2.push_str(args);
                                    }
                                }
                            }
                        }
                        Ok(StreamChunk::Usage(usage)) => {
                            // Usage arrives as partial events on some
                            // protocols (Anthropic splits prompt/output
                            // across message_start / message_delta), so fold
                            // field-by-field instead of replacing the whole
                            // struct — the same overwrite hazard already
                            // handled for finish_reason below.
                            tracing::debug!(
                                session_id = session_id.id,
                                input = ?usage.prompt_tokens,
                                output = ?usage.completion_tokens,
                                reasoning = ?usage.reasoning_tokens,
                                "LLM usage chunk received"
                            );
                            match &mut aggregator.usage {
                                Some(existing) => existing.merge(&usage),
                                None => aggregator.usage = Some(usage),
                            }
                        }
                        Ok(StreamChunk::Stop { finish_reason }) => {
                            // Only capture the first non-None finish_reason.
                            // OpenAI emits [DONE] after the final chunk, which
                            // would overwrite a "length" finish_reason with None,
                            // silently disabling the truncation guard.
                            if finish_reason.is_some() {
                                aggregator.finish_reason = finish_reason;
                            }
                        }
                        Err(e) => {
                            tracing::error!(session_id = session_id.id, error = %e, "LLM stream error");
                            return Err(e);
                        }
                        Ok(StreamChunk::ThinkingSignature(sig)) => {
                            aggregator.thinking_signature = Some(sig);
                        }
                        Ok(StreamChunk::Error(e)) => {
                            tracing::error!(session_id = session_id.id, error = %e, "LLM stream protocol error");
                            return Err(crate::types::AgentError::Llm(e));
                        }
                    }

                    {
                        let mut cb = on_event.lock().unwrap();
                        EventBus::drain_async_events(event_rx, &mut *cb)?;
                    }
                }
            }
        }

        let total_elapsed = start.elapsed();
        let _ttft_ms = aggregator.ttft_ms;

        // Reasoning is a separate channel from the final answer. We NEVER promote
        // reasoning_content into `content`: some models (e.g. qwen3.7-max) answer
        // entirely in the reasoning channel, but papering over that masks a stuck
        // agent as a completed run. Instead we classify and let the react loop route
        // it — `reasoning_only` (reasoning but no text and no tool call) is always a
        // degenerate state the loop nudges and eventually fails, never a fake answer.
        let full_text = aggregator.full_text;
        let reasoning_len = aggregator.reasoning_text.len();
        let reasoning_only = full_text.is_empty() && reasoning_len > 0 && !aggregator.is_tool_call;
        if reasoning_only {
            tracing::warn!(
                session_id = session_id.id,
                reasoning_len,
                "content empty but reasoning_content present — model produced reasoning only; \
                 not promoting it to the answer, the react loop will nudge/fail"
            );
        }

        tracing::info!(
            session_id = session_id.id,
            text_len = full_text.len(),
            reasoning_len,
            tool_calls = aggregator.partials.len(),
            elapsed_ms = total_elapsed.as_millis(),
            "LLM stream done"
        );

        let tool_calls = aggregator
            .partials
            .into_iter()
            // Drop phantom buckets: a fragment that carried an index but never
            // a function name is not a real tool call. Some OpenAI-compatible
            // providers (e.g. mimo) emit an empty `{index, name:""}` fragment;
            // kept, it lands in assistant history as a name-less tool_call and
            // the NEXT request is rejected with HTTP 400 "missing a function
            // name". The stream is fully drained here, so an empty name means
            // no name ever arrived — this is permanent, not a late fragment.
            .filter(|(_idx, (_id, name, _args))| !name.is_empty())
            .map(|(idx, (id, name, args))| {
                let mut tc = serde_json::Map::new();
                tc.insert("index".to_string(), idx.into());
                tc.insert("id".to_string(), id.into());
                tc.insert("type".to_string(), "function".into());
                let mut func = serde_json::Map::new();
                func.insert("name".to_string(), name.into());
                func.insert("arguments".to_string(), args.into());
                tc.insert("function".to_string(), func.into());
                serde_json::Value::Object(tc)
            })
            .collect::<Vec<_>>();

        Ok(LlmTurnResult {
            full_text,
            reasoning_text: aggregator.reasoning_text,
            is_tool_call: aggregator.is_tool_call,
            tool_calls,
            usage: aggregator.usage,
            finish_reason: aggregator.finish_reason,
            ttft_ms: aggregator.ttft_ms,
            llm_duration_ms: total_elapsed.as_millis() as u64,
            reasoning_only,
            thinking_signature: aggregator.thinking_signature,
        })
    }

    pub fn emit_text_delta(&self, session_id: &SessionId, text: String) {
        self.event_bus.emit(RuntimeEvent::TextDelta {
            session_id: session_id.clone(),
            text,
            agent_id: None,
            trace_id: None,
        });
    }
}

pub struct LlmTurnResult {
    pub full_text: String,
    /// Reasoning/thinking content accumulated from Thought stream chunks.
    /// Populated when the model uses thinking mode (e.g. qwen3.7-max).
    pub reasoning_text: String,
    pub is_tool_call: bool,
    pub tool_calls: Vec<Value>,
    pub usage: Option<UsageInfo>,
    /// Provider stop reason (e.g. "stop", "length", "tool_calls", "end_turn").
    /// `"length"` means the response hit the token limit — tool call arguments may be truncated.
    pub finish_reason: Option<String>,
    /// Time to first token in milliseconds (user-perceived latency).
    pub ttft_ms: u64,
    /// LLM stream duration in milliseconds (from stream start to end).
    pub llm_duration_ms: u64,
    /// True when the model emitted reasoning_content but no `content` and no tool
    /// call. This is always a degenerate state (the model "thought" but neither
    /// committed to a tool call nor produced an answer); the react loop nudges and
    /// eventually fails rather than promoting the reasoning into `full_text`.
    pub reasoning_only: bool,
    /// Thinking signature from the provider (e.g. Anthropic extended thinking).
    #[allow(dead_code)]
    pub thinking_signature: Option<String>,
}

struct StreamAggregator {
    pub full_text: String,
    /// Accumulated reasoning/thinking content (from StreamChunk::Thought).
    /// Kept separate from `full_text`; never promoted into the final answer.
    pub reasoning_text: String,
    pub is_tool_call: bool,
    pub partials: std::collections::HashMap<usize, (String, String, String)>,
    pub usage: Option<UsageInfo>,
    /// Provider stop reason captured from StreamChunk::Stop.
    pub finish_reason: Option<String>,
    /// Time to first token in milliseconds.
    pub ttft_ms: u64,
    /// Thinking signature from StreamChunk::ThinkingSignature.
    pub thinking_signature: Option<String>,
}

impl StreamAggregator {
    fn new() -> Self {
        Self {
            full_text: String::new(),
            reasoning_text: String::new(),
            is_tool_call: false,
            partials: std::collections::HashMap::new(),
            usage: None,
            finish_reason: None,
            ttft_ms: 0,
            thinking_signature: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentError, RetryConfig};
    use async_trait::async_trait;
    use llm_trait::{Capabilities, ChatRequest, ChatStream, LlmError, LlmProvider, ProviderInfo};
    use std::pin::Pin;

    struct StubProvider(&'static str);

    #[async_trait]
    impl LlmProvider for StubProvider {
        async fn stream(&self, _request: ChatRequest) -> Result<ChatStream, LlmError> {
            Ok(ChatStream::new(Box::pin(futures_util::stream::empty())))
        }

        async fn chat(&self, _request: ChatRequest) -> Result<llm_trait::ChatResponse, LlmError> {
            Err(LlmError::Llm("not implemented".into()))
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }

        fn info(&self) -> ProviderInfo {
            ProviderInfo {
                name: "stub".to_string(),
                model: self.0.to_string(),
                version: None,
            }
        }
    }

    fn engine(model: &'static str) -> LlmEngine {
        LlmEngine::new(Arc::new(StubProvider(model)), EventBus::new(16))
    }

    #[test]
    fn get_provider_and_set_provider_swap() {
        let engine = engine("model-a");
        assert_eq!(engine.get_provider().info().model, "model-a");
        engine.set_provider(Arc::new(StubProvider("model-b")));
        assert_eq!(engine.get_provider().info().model, "model-b");
    }

    #[test]
    fn get_provider_returns_provider() {
        let engine = engine("model-a");
        let provider = engine.get_provider();
        assert_eq!(provider.info().model, "model-a");
    }

    #[tokio::test]
    async fn process_stream_aggregates_text_usage_and_stop() {
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubProvider("x")), bus.clone());
        let chunks = vec![
            Ok(StreamChunk::Text("Hello".into())),
            Ok(StreamChunk::Text(" world".into())),
            Ok(StreamChunk::Usage(UsageInfo {
                prompt_tokens: Some(5),
                completion_tokens: Some(2),
                total_tokens: Some(7),
                reasoning_tokens: None,
            })),
            Ok(StreamChunk::Stop {
                finish_reason: Some("stop".into()),
            }),
        ];
        let stream = Box::pin(futures_util::stream::iter(chunks));
        let mut rx = bus.subscribe();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = engine
            .process_stream(
                &SessionId::new(1),
                stream,
                tracing::Span::none(),
                &mut rx,
                std::sync::Arc::new(std::sync::Mutex::new(move |e| {
                    events_clone.lock().unwrap().push(e);
                    Ok(())
                })),
                &cancel,
            )
            .await
            .unwrap();

        assert_eq!(result.full_text, "Hello world");
        assert!(!result.is_tool_call);
        assert_eq!(result.finish_reason.as_deref(), Some("stop"));
        let usage = result.usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(5));
        assert_eq!(usage.completion_tokens, Some(2));
        assert_eq!(usage.total_tokens, Some(7));
        assert!(result.llm_duration_ms >= result.ttft_ms);

        let events = events.lock().unwrap();
        let texts: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                RuntimeEvent::TextDelta { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["Hello".to_string(), " world".to_string()]);
    }

    #[tokio::test]
    async fn process_stream_merges_partial_usage_events() {
        // Anthropic streams split usage across two events: message_start
        // carries prompt_tokens only, message_delta carries prompt (optional)
        // plus the final completion count. Replacing the struct wholesale
        // made every Anthropic session record input_tokens = 0.
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubProvider("x")), bus.clone());
        let chunks = vec![
            Ok(StreamChunk::Usage(UsageInfo {
                prompt_tokens: Some(2),
                completion_tokens: Some(0),
                total_tokens: None,
                reasoning_tokens: None,
            })),
            Ok(StreamChunk::Usage(UsageInfo {
                prompt_tokens: Some(63),
                completion_tokens: Some(26),
                total_tokens: None,
                reasoning_tokens: None,
            })),
            Ok(StreamChunk::Stop {
                finish_reason: Some("end_turn".into()),
            }),
        ];
        let stream = Box::pin(futures_util::stream::iter(chunks));
        let mut rx = bus.subscribe();
        let noop = std::sync::Arc::new(std::sync::Mutex::new(|_e: RuntimeEvent| Ok(())));
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = engine
            .process_stream(
                &SessionId::new(1),
                stream,
                tracing::Span::none(),
                &mut rx,
                noop,
                &cancel,
            )
            .await
            .unwrap();

        let usage = result.usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(63), "later Some wins");
        assert_eq!(usage.completion_tokens, Some(26));
    }

    #[tokio::test]
    async fn process_stream_keeps_usage_when_delta_omits_prompt() {
        // Real Anthropic: message_delta has no input_tokens at all; the
        // message_start value must survive the merge rather than be zeroed.
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubProvider("x")), bus.clone());
        let chunks = vec![
            Ok(StreamChunk::Usage(UsageInfo {
                prompt_tokens: Some(5),
                completion_tokens: Some(1),
                total_tokens: None,
                reasoning_tokens: None,
            })),
            Ok(StreamChunk::Usage(UsageInfo {
                prompt_tokens: None,
                completion_tokens: Some(26),
                total_tokens: None,
                reasoning_tokens: None,
            })),
            Ok(StreamChunk::Stop {
                finish_reason: Some("end_turn".into()),
            }),
        ];
        let stream = Box::pin(futures_util::stream::iter(chunks));
        let mut rx = bus.subscribe();
        let noop = std::sync::Arc::new(std::sync::Mutex::new(|_e: RuntimeEvent| Ok(())));
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = engine
            .process_stream(
                &SessionId::new(1),
                stream,
                tracing::Span::none(),
                &mut rx,
                noop,
                &cancel,
            )
            .await
            .unwrap();

        let usage = result.usage.unwrap();
        assert_eq!(
            usage.prompt_tokens,
            Some(5),
            "prompt survives a partial delta"
        );
        assert_eq!(usage.completion_tokens, Some(26));
    }

    #[tokio::test]
    async fn process_stream_aggregates_tool_calls() {
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubProvider("x")), bus.clone());
        let chunks = vec![
            Ok(StreamChunk::ToolCall(serde_json::json!({
                "delta": { "tool_calls": [
                    { "index": 0, "id": "call_1", "function": { "name": "shell", "arguments": "{\"cmd\":" } }
                ] }
            }))),
            Ok(StreamChunk::ToolCall(serde_json::json!({
                "delta": { "tool_calls": [
                    { "index": 0, "function": { "arguments": "\"ls\"" } }
                ] }
            }))),
        ];
        let stream = Box::pin(futures_util::stream::iter(chunks));
        let mut rx = bus.subscribe();
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = engine
            .process_stream(
                &SessionId::new(1),
                stream,
                tracing::Span::none(),
                &mut rx,
                std::sync::Arc::new(std::sync::Mutex::new(|_e| Ok(()))),
                &cancel,
            )
            .await
            .unwrap();

        assert!(result.is_tool_call);
        assert!(result.full_text.is_empty());
        assert_eq!(result.tool_calls.len(), 1);
        let tc = &result.tool_calls[0];
        assert_eq!(tc["id"].as_str(), Some("call_1"));
        assert_eq!(tc["type"].as_str(), Some("function"));
        assert_eq!(tc["function"]["name"].as_str(), Some("shell"));
        assert_eq!(
            tc["function"]["arguments"].as_str(),
            Some("{\"cmd\":\"ls\"")
        );
    }

    #[tokio::test]
    async fn process_stream_flags_reasoning_only_without_promoting() {
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubProvider("x")), bus.clone());
        let chunks = vec![Ok(StreamChunk::Thought("deep thought".into()))];
        let stream = Box::pin(futures_util::stream::iter(chunks));
        let mut rx = bus.subscribe();
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = engine
            .process_stream(
                &SessionId::new(1),
                stream,
                tracing::Span::none(),
                &mut rx,
                std::sync::Arc::new(std::sync::Mutex::new(|_e| Ok(()))),
                &cancel,
            )
            .await
            .unwrap();

        // Reasoning must NOT be promoted into the answer text.
        assert_eq!(result.full_text, "");
        assert_eq!(result.reasoning_text, "deep thought");
        assert!(!result.is_tool_call);
        assert!(result.reasoning_only, "reasoning-only should be flagged");
    }

    #[tokio::test]
    async fn process_stream_propagates_error() {
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubProvider("x")), bus.clone());
        let chunks = vec![
            Ok(StreamChunk::Text("partial".into())),
            Err(AgentError::internal("boom")),
        ];
        let stream = Box::pin(futures_util::stream::iter(chunks));
        let mut rx = bus.subscribe();
        let cancel = tokio_util::sync::CancellationToken::new();
        let err = engine
            .process_stream(
                &SessionId::new(1),
                stream,
                tracing::Span::none(),
                &mut rx,
                std::sync::Arc::new(std::sync::Mutex::new(|_e| Ok(()))),
                &cancel,
            )
            .await
            .err()
            .expect("stream should error");
        assert!(err.to_string().contains("boom"));
    }

    #[tokio::test]
    async fn process_stream_cancelled() {
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubProvider("x")), bus.clone());
        let stream: Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>> =
            Box::pin(futures_util::stream::pending());
        let mut rx = bus.subscribe();
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        let err = engine
            .process_stream(
                &SessionId::new(1),
                stream,
                tracing::Span::none(),
                &mut rx,
                std::sync::Arc::new(std::sync::Mutex::new(|_e| Ok(()))),
                &cancel,
            )
            .await
            .err()
            .expect("stream should error");
        assert!(matches!(err, AgentError::Cancelled));
    }

    // ── B5: chat_stream + retry + remaining process_stream branches ────────

    /// A provider whose `stream()` always fails.
    struct AlwaysFail;

    #[async_trait]
    impl LlmProvider for AlwaysFail {
        async fn stream(&self, _request: ChatRequest) -> Result<ChatStream, LlmError> {
            Err(LlmError::Llm("api down".into()))
        }

        async fn chat(&self, _request: ChatRequest) -> Result<llm_trait::ChatResponse, LlmError> {
            Err(LlmError::Llm("api down".into()))
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }

        fn info(&self) -> ProviderInfo {
            ProviderInfo {
                name: "always-fail".to_string(),
                model: "always-fail".to_string(),
                version: None,
            }
        }
    }

    /// A provider that fails the first `n` `stream()` calls, then succeeds.
    struct FailThenSucceed {
        remaining: std::sync::Mutex<usize>,
    }

    impl FailThenSucceed {
        fn new(failures: usize) -> Self {
            Self {
                remaining: std::sync::Mutex::new(failures),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for FailThenSucceed {
        async fn stream(&self, _request: ChatRequest) -> Result<ChatStream, LlmError> {
            let mut remaining = self.remaining.lock().unwrap();
            if *remaining > 0 {
                *remaining -= 1;
                Err(LlmError::Llm("transient".into()))
            } else {
                Ok(ChatStream::new(Box::pin(futures_util::stream::empty())))
            }
        }

        async fn chat(&self, _request: ChatRequest) -> Result<llm_trait::ChatResponse, LlmError> {
            Err(LlmError::Llm("not implemented".into()))
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }

        fn info(&self) -> ProviderInfo {
            ProviderInfo {
                name: "fail-then-succeed".to_string(),
                model: "fail-then-succeed".to_string(),
                version: None,
            }
        }
    }

    #[tokio::test]
    async fn chat_stream_returns_stream_on_ok() {
        let engine = LlmEngine::new(Arc::new(StubProvider("x")), EventBus::new(16));
        let mut stream = engine
            .chat_stream(&[], &[], None, None, false)
            .await
            .unwrap();
        let next = futures_util::StreamExt::next(&mut stream).await;
        assert!(next.is_none());
    }

    #[tokio::test]
    async fn chat_stream_with_thinking_disabled() {
        let engine = LlmEngine::new(Arc::new(StubProvider("x")), EventBus::new(16));
        // Test with thinking_disabled=true — should still work, just without reasoning
        let mut stream = engine
            .chat_stream(&[], &[], None, None, true)
            .await
            .unwrap();
        let next = futures_util::StreamExt::next(&mut stream).await;
        assert!(next.is_none());
    }

    #[tokio::test]
    async fn chat_stream_forwards_error() {
        let engine = LlmEngine::new(Arc::new(AlwaysFail), EventBus::new(16));
        let err = engine
            .chat_stream(&[], &[], None, None, false)
            .await
            .err()
            .expect("should fail");
        assert!(err.to_string().contains("api down"));
    }

    #[tokio::test]
    async fn run_llm_turn_retries_then_succeeds() {
        let engine = LlmEngine::new(Arc::new(FailThenSucceed::new(2)), EventBus::new(16));
        let cfg = RetryConfig {
            max_retries: 3,
            initial_backoff_ms: 1,
            max_backoff_ms: 1000,
            backoff_multiplier: 2.0,
            jitter: false,
        };
        let _stream = engine
            .run_llm_turn_with_retry(&SessionId::new(1), &[], &[], None, None, cfg, false)
            .await
            .expect("should succeed after retries");
    }

    #[tokio::test]
    async fn run_llm_turn_retries_exhausted() {
        let engine = LlmEngine::new(Arc::new(FailThenSucceed::new(100)), EventBus::new(16));
        let cfg = RetryConfig {
            max_retries: 2,
            initial_backoff_ms: 1,
            max_backoff_ms: 1,
            backoff_multiplier: 2.0,
            jitter: false,
        };
        let err = engine
            .run_llm_turn_with_retry(&SessionId::new(1), &[], &[], None, None, cfg, false)
            .await
            .err()
            .expect("should be exhausted");
        assert!(err.to_string().contains("transient"));
    }

    #[tokio::test]
    async fn process_stream_tool_call_missing_delta_continues() {
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubProvider("x")), bus.clone());
        let chunks = vec![
            Ok(StreamChunk::ToolCall(
                serde_json::json!({ "no_delta": true }),
            )),
            Ok(StreamChunk::Stop {
                finish_reason: Some("stop".into()),
            }),
        ];
        let stream = Box::pin(futures_util::stream::iter(chunks));
        let mut rx = bus.subscribe();
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = engine
            .process_stream(
                &SessionId::new(1),
                stream,
                tracing::Span::none(),
                &mut rx,
                std::sync::Arc::new(std::sync::Mutex::new(|_e| Ok(()))),
                &cancel,
            )
            .await
            .unwrap();
        assert!(result.is_tool_call);
        assert!(result.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn process_stream_tool_call_empty_id_and_name_ignored() {
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubProvider("x")), bus.clone());
        let chunks = vec![Ok(StreamChunk::ToolCall(serde_json::json!({
            "delta": { "tool_calls": [
                { "index": 0, "id": "", "function": { "name": "", "arguments": "{}" } }
            ] }
        })))];
        let stream = Box::pin(futures_util::stream::iter(chunks));
        let mut rx = bus.subscribe();
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = engine
            .process_stream(
                &SessionId::new(1),
                stream,
                tracing::Span::none(),
                &mut rx,
                std::sync::Arc::new(std::sync::Mutex::new(|_e| Ok(()))),
                &cancel,
            )
            .await
            .unwrap();
        // A bucket that carries an index but never a function name is a
        // phantom fragment some OpenAI-compatible providers emit. It must NOT
        // surface as a tool call — kept, it becomes a name-less assistant
        // tool_call and the NEXT request is rejected HTTP 400 "missing a
        // function name". The stream is fully drained here, so an empty name
        // at assembly time is permanent, not a name that arrives later.
        assert_eq!(result.tool_calls.len(), 0);
    }

    #[tokio::test]
    async fn process_stream_stop_none_finish_reason_ignored() {
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubProvider("x")), bus.clone());
        let chunks = vec![
            Ok(StreamChunk::Stop {
                finish_reason: Some("length".into()),
            }),
            Ok(StreamChunk::Stop {
                finish_reason: None,
            }),
        ];
        let stream = Box::pin(futures_util::stream::iter(chunks));
        let mut rx = bus.subscribe();
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = engine
            .process_stream(
                &SessionId::new(1),
                stream,
                tracing::Span::none(),
                &mut rx,
                std::sync::Arc::new(std::sync::Mutex::new(|_e| Ok(()))),
                &cancel,
            )
            .await
            .unwrap();
        assert_eq!(result.finish_reason.as_deref(), Some("length"));
    }

    #[tokio::test]
    async fn process_stream_text_and_thought_while_tool_call_skip_emit() {
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubProvider("x")), bus.clone());
        let chunks = vec![
            Ok(StreamChunk::ToolCall(serde_json::json!({
                "delta": { "tool_calls": [
                    { "index": 0, "id": "c1", "function": { "name": "t", "arguments": "{}" } }
                ] }
            }))),
            Ok(StreamChunk::Text("ignored".into())),
            Ok(StreamChunk::Thought("thinking".into())),
        ];
        let stream = Box::pin(futures_util::stream::iter(chunks));
        let mut rx = bus.subscribe();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = engine
            .process_stream(
                &SessionId::new(1),
                stream,
                tracing::Span::none(),
                &mut rx,
                std::sync::Arc::new(std::sync::Mutex::new(move |e| {
                    events_clone.lock().unwrap().push(e);
                    Ok(())
                })),
                &cancel,
            )
            .await
            .unwrap();

        // Text/Thought are accumulated but NOT emitted once a tool call started.
        assert_eq!(result.full_text, "ignored");
        assert_eq!(result.reasoning_text, "thinking");
        let events = events.lock().unwrap();
        assert!(
            events.iter().all(|e| !matches!(
                e,
                RuntimeEvent::TextDelta { .. } | RuntimeEvent::ThoughtDelta { .. }
            )),
            "no Text/Thought deltas expected, got {events:?}"
        );
    }

    #[test]
    fn emit_text_delta_emits_event() {
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubProvider("x")), bus.clone());
        let mut rx = bus.subscribe();
        engine.emit_text_delta(&SessionId::new(1), "hello".into());
        let ev = rx.try_recv().expect("event should be emitted");
        assert!(matches!(ev, RuntimeEvent::TextDelta { text, .. } if text == "hello"));
    }
}
