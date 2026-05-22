use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::broadcast;
use tracing::info_span;

use crate::llm::StreamChunk;
use crate::types::{AgentResult, AgentEvent, ChatMessage, SessionId};

use super::AgentRuntime;

type LlmStream = Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>;

pub(super) struct StreamAggregator {
    is_tool_call: bool,
    full_text: String,
    partials: HashMap<usize, (String, String, String)>,
    tool_calls: Vec<(String, String, String)>,
}

impl StreamAggregator {
    pub(super) fn new() -> Self {
        Self {
            is_tool_call: false,
            full_text: String::new(),
            partials: HashMap::new(),
            tool_calls: Vec::new(),
        }
    }

    fn finalize_tool_calls(&mut self) {
        if self.partials.is_empty() {
            return;
        }
        let mut indices: Vec<_> = self.partials.keys().copied().collect();
        indices.sort();
        self.tool_calls = indices
            .into_iter()
            .filter_map(|i| self.partials.remove(&i))
            .collect();
    }

    pub(super) fn into_parts(mut self) -> (String, bool, Vec<(String, String, String)>) {
        self.finalize_tool_calls();
        (self.full_text, self.is_tool_call, self.tool_calls)
    }
}

impl AgentRuntime {
    pub(super) async fn execute_llm_turn<F>(
        &self,
        session_id: &SessionId,
        messages: &[ChatMessage],
        tool_definitions: &[Value],
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<StreamAggregator>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let _span = info_span!("llm_turn", session_id = session_id.id).entered();
        tracing::debug!(session_id = session_id.id, msg_count = messages.len(), tool_count = tool_definitions.len(), "calling LLM");
        drop(_span);

        eprintln!("[agent] LLM turn start: session={}, messages={}, tools={}", session_id.id, messages.len(), tool_definitions.len());

        let stream = self
            .call_llm_with_retry(messages, tool_definitions, session_id, event_rx, on_event)
            .await?;

        let mut aggregator = StreamAggregator::new();

        Self::consume_stream(stream, &mut aggregator, session_id, event_rx, on_event, self).await?;

        eprintln!("[agent] LLM turn done: session={}, text_len={}, is_tool_call={}, tool_calls={}",
            session_id.id, aggregator.full_text.len(), aggregator.is_tool_call, aggregator.tool_calls.len());

        Ok(aggregator)
    }

    async fn call_llm_with_retry<F>(
        &self,
        messages: &[ChatMessage],
        tool_definitions: &[Value],
        session_id: &SessionId,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<LlmStream>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let retry = match &self.config.llm_retry {
            Some(r) => r.clone(),
            None => {
                return self
                    .client
                    .chat_stream(
                        messages,
                        tool_definitions,
                        self.config.reasoning.as_ref(),
                        self.config.response_format.as_ref(),
                    )
                    .await;
            }
        };

        let mut attempt: u32 = 0;
        let mut backoff_ms = retry.initial_backoff_ms;

        loop {
            match self
                .client
                .chat_stream(
                    messages,
                    tool_definitions,
                    self.config.reasoning.as_ref(),
                    self.config.response_format.as_ref(),
                )
                .await
            {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    attempt += 1;
                    if attempt > retry.max_retries || !e.is_retryable() {
                        tracing::warn!(session_id = session_id.id, attempt, error = %e, "LLM call failed after retries");
                        return Err(e);
                    }

                    let jitter = if retry.jitter {
                        (attempt as u64 * 37 + 13) % (backoff_ms / 4 + 1)
                    } else {
                        0
                    };

                    tracing::warn!(session_id = session_id.id, attempt, max_retries = retry.max_retries, backoff_ms = backoff_ms + jitter, "LLM call retrying");

                    let _ = self.event_bus.send(AgentEvent::Custom {
                        session_id: session_id.clone(),
                        payload: serde_json::json!({
                            "type": "llm_retry",
                            "attempt": attempt,
                            "max_retries": retry.max_retries,
                            "backoff_ms": backoff_ms + jitter,
                            "error": e.to_string(),
                        }),
                    });

                    Self::drain_async_events(event_rx, on_event)?;

                    tokio::time::sleep(Duration::from_millis(backoff_ms + jitter)).await;
                    backoff_ms =
                        ((backoff_ms as f64) * retry.backoff_multiplier).min(retry.max_backoff_ms as f64) as u64;
                }
            }
        }
    }

    async fn consume_stream<F>(
        mut stream: impl futures_core::Stream<Item = AgentResult<StreamChunk>> + Unpin,
        aggregator: &mut StreamAggregator,
        session_id: &SessionId,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
        runtime: &Self,
    ) -> AgentResult<()>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let start = std::time::Instant::now();
        let mut first_token = true;

        loop {
            tokio::select! {
                recv_result = event_rx.recv() => {
                    match recv_result {
                        Ok(event) => on_event(event)?,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                maybe_chunk = stream.next() => {
                    let Some(chunk_result) = maybe_chunk else {
                        break;
                    };

                    if first_token {
                        let ttft = start.elapsed();
                        eprintln!("[agent] LLM first token: session={}, ttft={:?}", session_id.id, ttft);
                        first_token = false;
                    }
                    let chunk = chunk_result?;
                    match chunk {
                        StreamChunk::Text(text) => {
                            if !text.is_empty() && !aggregator.is_tool_call {
                                aggregator.full_text.push_str(&text);
                                runtime.emit_event(AgentEvent::TextDelta { session_id: session_id.clone(), text });
                            }
                        }
                        StreamChunk::Thought(text) => {
                            eprintln!("[agent] LLM thought chunk: session={}, len={}", session_id.id, text.len());
                            if !text.is_empty() && !aggregator.is_tool_call {
                                if runtime.config.enable_thought {
                                    runtime.emit_event(AgentEvent::ThoughtDelta { session_id: session_id.clone(), text });
                                } else {
                                    eprintln!("[agent] LLM thought IGNORED: enable_thought=false");
                                }
                            }
                        }
                        StreamChunk::ToolCall(choice) => {
                            aggregator.is_tool_call = true;
                            if let Some(tool_calls) = choice
                                .get("delta")
                                .and_then(|d| d.get("tool_calls"))
                                .and_then(Value::as_array)
                            {
                                for tool_call in tool_calls {
                                    let idx = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
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
                        }
                        StreamChunk::Usage(_) => {}
                        StreamChunk::Stop => break,
                    }
                    Self::drain_async_events(event_rx, on_event)?;
                }
            }
        }

        let total_elapsed = start.elapsed();
        eprintln!("[agent] LLM stream done: session={}, text_len={}, elapsed_ms={}", session_id.id, aggregator.full_text.len(), total_elapsed.as_millis());
        Ok(())
    }
}
