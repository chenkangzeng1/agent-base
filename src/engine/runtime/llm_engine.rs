use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::Value;
use tracing::Span;

use crate::llm::{LlmClient, ReasoningConfig, StreamChunk, UsageInfo};
use crate::types::{AgentEvent, AgentResult, ChatMessage, SessionId};
use crate::engine::runtime::event_bus::EventBus;

pub struct LlmEngine {
    pub client: Arc<dyn LlmClient>,
    event_bus: EventBus,
}

impl LlmEngine {
    pub fn new(client: Arc<dyn LlmClient>, event_bus: EventBus) -> Self {
        Self { client, event_bus }
    }

    pub async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tool_definitions: &[Value],
        reasoning: Option<&ReasoningConfig>,
        response_format: Option<&crate::types::ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        self.client
            .chat_stream(messages, tool_definitions, reasoning, response_format)
            .await
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
                .client
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

    pub async fn process_stream<F>(
        &self,
        session_id: &SessionId,
        mut stream: Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>,
        span: Span,
        event_rx: &mut tokio::sync::broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<LlmTurnResult>
    where
        F: FnMut(AgentEvent) -> AgentResult<()> + Send,
    {
        let _enter = span.enter();
        let start = std::time::Instant::now();
        let mut first_token = true;
        let mut aggregator = StreamAggregator::new();
        tracing::debug!(session_id = session_id.id, "process stream start");

        loop {
            tokio::select! {
                recv_result = event_rx.recv() => {
                    match recv_result {
                        Ok(event) => on_event(event)?,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                maybe_chunk = stream.next() => {
                    let Some(chunk) = maybe_chunk else {
                        break;
                    };

                    match chunk {
                        Ok(StreamChunk::Text(text)) => {
                            if first_token {
                                let ttft = start.elapsed();
                                tracing::debug!(session_id = session_id.id, ?ttft, "llm first token");
                                first_token = false;
                            }
                            if !text.is_empty() && !aggregator.is_tool_call {
                                self.event_bus.emit(AgentEvent::TextDelta {
                                    session_id: session_id.clone(),
                                    text: text.clone(),
                                });
                            }
                            aggregator.full_text.push_str(&text);
                        }
                        Ok(StreamChunk::Thought(text)) => {
                            tracing::debug!(session_id = session_id.id, len = text.len(), "llm thought chunk");
                            if !text.is_empty() && !aggregator.is_tool_call {
                                self.event_bus.emit(AgentEvent::ThoughtDelta {
                                    session_id: session_id.clone(),
                                    text,
                                });
                            }
                        }
                        Ok(StreamChunk::ToolCall(choice)) => {
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
        tracing::debug!(session_id = session_id.id, text_len = aggregator.full_text.len(), tool_calls = aggregator.partials.len(), elapsed_ms = total_elapsed.as_millis(), "llm stream done");

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
            full_text: aggregator.full_text,
            is_tool_call: aggregator.is_tool_call,
            tool_calls,
            usage: aggregator.usage,
        })
    }

    pub fn emit_text_delta(&self, session_id: &SessionId, text: String) {
        self.event_bus.emit(AgentEvent::TextDelta {
            session_id: session_id.clone(),
            text,
        });
    }
}

pub struct LlmTurnResult {
    pub full_text: String,
    pub is_tool_call: bool,
    pub tool_calls: Vec<Value>,
    pub usage: Option<UsageInfo>,
}

struct StreamAggregator {
    pub full_text: String,
    pub is_tool_call: bool,
    pub partials: std::collections::HashMap<usize, (String, String, String)>,
    pub usage: Option<UsageInfo>,
}

impl StreamAggregator {
    fn new() -> Self {
        Self {
            full_text: String::new(),
            is_tool_call: false,
            partials: std::collections::HashMap::new(),
            usage: None,
        }
    }
}
