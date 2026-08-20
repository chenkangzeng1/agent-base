use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::Value;
use tracing::Span;

use crate::engine::runtime::event_bus::EventBus;
use crate::llm::{ReasoningConfig, StreamChunk, StreamClient, UsageInfo};
use crate::types::{AgentResult, ChatMessage, RuntimeEvent, SessionId};

pub struct LlmEngine {
    client: RwLock<Arc<dyn StreamClient>>,
    event_bus: EventBus,
}

impl LlmEngine {
    pub(crate) fn new(client: Arc<dyn StreamClient>, event_bus: EventBus) -> Self {
        Self {
            client: RwLock::new(client),
            event_bus,
        }
    }

    /// Get a clone of the current LLM client.
    pub fn get_client(&self) -> Arc<dyn StreamClient> {
        self.client.read().unwrap().clone()
    }

    /// Replace the LLM client at runtime (e.g., model switch).
    pub fn set_client(&self, client: Arc<dyn StreamClient>) {
        *self.client.write().unwrap() = client;
    }

    pub async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tool_definitions: &[Value],
        reasoning: Option<&ReasoningConfig>,
        response_format: Option<&crate::types::ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        tracing::info!(
            msg_count = messages.len(),
            tool_count = tool_definitions.len(),
            "LLM chat_stream: sending request to API"
        );
        let result = self
            .get_client()
            .stream(messages, tool_definitions, reasoning, response_format)
            .await;
        match &result {
            Ok(_) => tracing::info!("LLM chat_stream: API response received"),
            Err(e) => tracing::error!(error = %e, "LLM chat_stream: API request failed"),
        }
        result
    }

    pub async fn run_llm_turn_with_retry(
        &self,
        session_id: &SessionId,
        messages: &[ChatMessage],
        tool_definitions: &[Value],
        reasoning: Option<&ReasoningConfig>,
        response_format: Option<&crate::types::ResponseFormat>,
        retry: crate::types::RetryConfig,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        let mut attempt = 1;
        let mut delay_ms = retry.initial_backoff_ms;
        loop {
            match self
                .get_client()
                .stream(messages, tool_definitions, reasoning, response_format)
                .await
            {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    if attempt > retry.max_retries {
                        tracing::error!(session_id = session_id.id, attempts = attempt, error = %e, "LLM stream retry exhausted");
                        return Err(e);
                    }
                    tracing::warn!(session_id = session_id.id, attempt, error = %e, "LLM stream failed, retrying...");
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

        // Skip UserEvents that were already rendered by drain_fn during middleware
        // execution (compression Progress/Started/Completed). drain_fn has its own
        // broadcast subscriber and forwarded these to on_event — processing them
        // again here would duplicate. Only skip UserEvent; keep Checkpoint and others.
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
                            aggregator.usage = Some(usage);
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmCapabilities;
    use crate::types::{AgentError, ResponseFormat, RetryConfig};
    use async_trait::async_trait;
    use std::pin::Pin;

    struct StubClient(&'static str);

    #[async_trait]
    impl StreamClient for StubClient {
        async fn stream(
            &self,
            _messages: &[ChatMessage],
            _tools: &[Value],
            _reasoning: Option<&ReasoningConfig>,
            _response_format: Option<&ResponseFormat>,
        ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
            Ok(Box::pin(futures_util::stream::empty()))
        }

        fn capabilities(&self) -> LlmCapabilities {
            LlmCapabilities::default()
        }

        fn model_name(&self) -> &str {
            self.0
        }
    }

    fn engine(model: &'static str) -> LlmEngine {
        LlmEngine::new(Arc::new(StubClient(model)), EventBus::new(16))
    }

    #[test]
    fn get_client_and_set_client_swap() {
        let engine = engine("model-a");
        assert_eq!(engine.get_client().model_name(), "model-a");
        engine.set_client(Arc::new(StubClient("model-b")));
        assert_eq!(engine.get_client().model_name(), "model-b");
    }

    #[tokio::test]
    async fn process_stream_aggregates_text_usage_and_stop() {
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubClient("x")), bus.clone());
        let chunks = vec![
            Ok(StreamChunk::Text("Hello".into())),
            Ok(StreamChunk::Text(" world".into())),
            Ok(StreamChunk::Usage(UsageInfo {
                prompt_tokens: Some(5),
                completion_tokens: Some(2),
                total_tokens: Some(7),
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
    async fn process_stream_aggregates_tool_calls() {
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubClient("x")), bus.clone());
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
        let engine = LlmEngine::new(Arc::new(StubClient("x")), bus.clone());
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
        let engine = LlmEngine::new(Arc::new(StubClient("x")), bus.clone());
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
        let engine = LlmEngine::new(Arc::new(StubClient("x")), bus.clone());
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

    /// A client whose `stream()` always fails.
    struct AlwaysFail;

    #[async_trait]
    impl StreamClient for AlwaysFail {
        async fn stream(
            &self,
            _messages: &[ChatMessage],
            _tools: &[Value],
            _reasoning: Option<&ReasoningConfig>,
            _response_format: Option<&ResponseFormat>,
        ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
            Err(AgentError::internal("api down"))
        }

        fn capabilities(&self) -> LlmCapabilities {
            LlmCapabilities::default()
        }

        fn model_name(&self) -> &str {
            "always-fail"
        }
    }

    /// A client that fails the first `n` `stream()` calls, then succeeds.
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
    impl StreamClient for FailThenSucceed {
        async fn stream(
            &self,
            _messages: &[ChatMessage],
            _tools: &[Value],
            _reasoning: Option<&ReasoningConfig>,
            _response_format: Option<&ResponseFormat>,
        ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
            let mut remaining = self.remaining.lock().unwrap();
            if *remaining > 0 {
                *remaining -= 1;
                Err(AgentError::internal("transient"))
            } else {
                Ok(Box::pin(futures_util::stream::empty()))
            }
        }

        fn capabilities(&self) -> LlmCapabilities {
            LlmCapabilities::default()
        }

        fn model_name(&self) -> &str {
            "fail-then-succeed"
        }
    }

    #[tokio::test]
    async fn chat_stream_returns_stream_on_ok() {
        let engine = LlmEngine::new(Arc::new(StubClient("x")), EventBus::new(16));
        let mut stream = engine.chat_stream(&[], &[], None, None).await.unwrap();
        let next = futures_util::StreamExt::next(&mut stream).await;
        assert!(next.is_none());
    }

    #[tokio::test]
    async fn chat_stream_forwards_error() {
        let engine = LlmEngine::new(Arc::new(AlwaysFail), EventBus::new(16));
        let err = engine
            .chat_stream(&[], &[], None, None)
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
            .run_llm_turn_with_retry(&SessionId::new(1), &[], &[], None, None, cfg)
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
            .run_llm_turn_with_retry(&SessionId::new(1), &[], &[], None, None, cfg)
            .await
            .err()
            .expect("should be exhausted");
        assert!(err.to_string().contains("transient"));
    }

    #[tokio::test]
    async fn process_stream_tool_call_missing_delta_continues() {
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubClient("x")), bus.clone());
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
        let engine = LlmEngine::new(Arc::new(StubClient("x")), bus.clone());
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
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0]["id"].as_str(), Some(""));
        assert_eq!(result.tool_calls[0]["function"]["name"].as_str(), Some(""));
        assert_eq!(
            result.tool_calls[0]["function"]["arguments"].as_str(),
            Some("{}")
        );
    }

    #[tokio::test]
    async fn process_stream_stop_none_finish_reason_ignored() {
        let bus = EventBus::new(16);
        let engine = LlmEngine::new(Arc::new(StubClient("x")), bus.clone());
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
        let engine = LlmEngine::new(Arc::new(StubClient("x")), bus.clone());
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
        let engine = LlmEngine::new(Arc::new(StubClient("x")), bus.clone());
        let mut rx = bus.subscribe();
        engine.emit_text_delta(&SessionId::new(1), "hello".into());
        let ev = rx.try_recv().expect("event should be emitted");
        assert!(matches!(ev, RuntimeEvent::TextDelta { text, .. } if text == "hello"));
    }
}
