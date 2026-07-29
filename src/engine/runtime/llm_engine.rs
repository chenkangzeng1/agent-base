use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::Value;
use tracing::Span;

use crate::engine::runtime::event_bus::EventBus;
use crate::llm::{LlmClient, ReasoningConfig, StreamChunk, UsageInfo};
use crate::types::{AgentResult, ChatMessage, RuntimeEvent, SessionId};

pub struct LlmEngine {
    client: RwLock<Arc<dyn LlmClient>>,
    event_bus: EventBus,
}

impl LlmEngine {
    pub(crate) fn new(client: Arc<dyn LlmClient>, event_bus: EventBus) -> Self {
        Self {
            client: RwLock::new(client),
            event_bus,
        }
    }

    /// Get a clone of the current LLM client.
    pub fn get_client(&self) -> Arc<dyn LlmClient> {
        self.client.read().unwrap().clone()
    }

    /// Replace the LLM client at runtime (e.g., model switch).
    pub fn set_client(&self, client: Arc<dyn LlmClient>) {
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
            .chat_stream(messages, tool_definitions, reasoning, response_format)
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
                .chat_stream(messages, tool_definitions, reasoning, response_format)
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
        on_event: &mut F,
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

        loop {
            tokio::select! {
                recv_result = event_rx.recv() => {
                    match recv_result {
                        Ok(event) => on_event(RuntimeEvent::from(event))?,
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
                                tracing::info!(session_id = session_id.id, ?ttft, "LLM first token received");
                                first_token = false;
                            }
                            if !text.is_empty() && !aggregator.is_tool_call {
                                self.event_bus.emit(RuntimeEvent::TextDelta {
                                    session_id: session_id.clone(),
                                    text: text.clone(),
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
                                if let Some(id) = tool_call.get("id").and_then(Value::as_str) {
                                    if !id.is_empty() {
                                        entry.0 = id.to_string();
                                    }
                                }
                                if let Some(func) = tool_call.get("function") {
                                    if let Some(name) = func.get("name").and_then(Value::as_str) {
                                        if !name.is_empty() {
                                            entry.1 = name.to_string();
                                        }
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
                        Ok(StreamChunk::Stop) => {}
                        Err(e) => {
                            tracing::error!(session_id = session_id.id, error = %e, "LLM stream error");
                            return Err(e);
                        }
                    }

                    EventBus::drain_async_events(event_rx, on_event)?;
                }
            }
        }

        let total_elapsed = start.elapsed();

        // Fallback: some models (e.g. qwen3.7-max) put the final answer in
        // reasoning_content instead of content when thinking is enabled.
        // If we got reasoning but no text and no tool calls, use reasoning as the response.
        // NOTE: this usually means tools were not provided — the model "thinks" about calling
        // a tool but can't, so it only produces reasoning. Check your tool configuration.
        let mut full_text = aggregator.full_text;
        let reasoning_len = aggregator.reasoning_text.len();
        if full_text.is_empty() && reasoning_len > 0 && !aggregator.is_tool_call {
            tracing::warn!(
                session_id = session_id.id,
                reasoning_len,
                "content empty but reasoning_content present — this usually means no tools were defined. \
                 Using reasoning as fallback response. Check that tools are correctly passed to the LLM."
            );
            // Use reasoning as the response text so react_loop doesn't treat it as empty.
            // Do NOT emit TextDelta here — the frontend already received the content as
            // ThoughtDelta events during streaming. Emitting TextDelta would duplicate it.
            // The session push in react_loop will persist the content for reload.
            full_text = aggregator.reasoning_text.clone();
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
        })
    }

    pub fn emit_text_delta(&self, session_id: &SessionId, text: String) {
        self.event_bus.emit(RuntimeEvent::TextDelta {
            session_id: session_id.clone(),
            text,
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
}

struct StreamAggregator {
    pub full_text: String,
    /// Accumulated reasoning/thinking content (from StreamChunk::Thought).
    /// Some models (e.g. qwen3.7-max) put the final answer in reasoning_content
    /// instead of content when thinking is enabled.
    pub reasoning_text: String,
    pub is_tool_call: bool,
    pub partials: std::collections::HashMap<usize, (String, String, String)>,
    pub usage: Option<UsageInfo>,
}

impl StreamAggregator {
    fn new() -> Self {
        Self {
            full_text: String::new(),
            reasoning_text: String::new(),
            is_tool_call: false,
            partials: std::collections::HashMap::new(),
            usage: None,
        }
    }
}
