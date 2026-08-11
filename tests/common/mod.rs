//! Shared test utilities for phi-agent integration tests.
//!
//! Provides a mock LLM client, stream stubs, and helper functions
//! used across multiple test files.

#![allow(dead_code)]

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use agent_base::{AgentResult, ChatMessage, LlmCapabilities, LlmClient, ReasoningConfig, ResponseFormat, StreamChunk};
use async_trait::async_trait;
use futures_core::Stream;
use phi_agent::bridge::server::ProtocolServer;
use phi_agent::{
    ApprovalMode, AutoApprovalHandler, SafetyConfig, TurnFactMiddleware, TurnToolLimitMiddleware, base_agent_builder,
    build_system_prompt,
};
use serde_json::{Value, json};

// ── EmptyStream — stubs chat_stream() ─────────────────────────────────

pub struct EmptyStream;

impl Stream for EmptyStream {
    type Item = AgentResult<StreamChunk>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}

// ── MockLlmClient — configurable LLM stub ─────────────────────────────

/// A mock LLM client that can be programmed to return tool calls or text.
pub struct MockLlmClient {
    pub tool_call_response: tokio::sync::Mutex<Option<(String, String)>>,
    pub text_response: String,
}

impl MockLlmClient {
    pub fn new() -> Self {
        Self { tool_call_response: tokio::sync::Mutex::new(None), text_response: "mock response".to_string() }
    }

    pub async fn set_tool_call(&self, name: &str, args: &Value) {
        *self.tool_call_response.lock().await = Some((name.to_string(), args.to_string()));
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
        let tc = self.tool_call_response.lock().await.take();
        if let Some((name, args)) = tc {
            Ok(json!({
                "choices": [{
                    "message": {
                        "tool_calls": [{
                            "id": "call-test-1",
                            "type": "function",
                            "function": {"name": name, "arguments": args}
                        }]
                    }
                }]
            }))
        } else {
            Ok(json!({
                "choices": [{"message": {"content": self.text_response.clone()}}]
            }))
        }
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        Ok(Box::pin(EmptyStream))
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_thinking: true,
            max_context_tokens: Some(128_000),
            max_output_tokens: Some(16_384),
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

pub fn build_server(mock: Arc<dyn agent_base::StreamClient>) -> ProtocolServer {
    let builder = base_agent_builder(mock)
        .system_prompt(build_system_prompt())
        .approval_handler(Arc::new(AutoApprovalHandler::new(ApprovalMode::Auto)))
        .middleware(TurnFactMiddleware::new())
        .middleware(TurnToolLimitMiddleware::from_config(&SafetyConfig::default()));
    ProtocolServer::from_builder(builder).expect("build server")
}

pub fn event_type(event: &phi_agent::RuntimeEvent) -> &'static str {
    match event {
        phi_agent::RuntimeEvent::TextDelta { .. } => "text_delta",
        phi_agent::RuntimeEvent::ThoughtDelta { .. } => "thought_delta",
        phi_agent::RuntimeEvent::ToolCallStarted { .. } => "tool_call_started",
        phi_agent::RuntimeEvent::ToolCallFinished { .. } => "tool_call_finished",
        phi_agent::RuntimeEvent::RunFinished { .. } => "run_finished",
        phi_agent::RuntimeEvent::RunCancelled { .. } => "run_cancelled",
        _ => "other",
    }
}

pub async fn collect_events(event_rx: &mut tokio::sync::broadcast::Receiver<phi_agent::RuntimeEvent>) -> Vec<String> {
    let mut events = Vec::new();
    loop {
        tokio::select! {
            event_result = event_rx.recv() => {
                match event_result {
                    Ok(event) => {
                        let typ = event_type(&event);
                        events.push(typ.to_string());
                        if typ == "run_finished" || typ == "run_cancelled" {
                            return events;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return events,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                return events;
            }
        }
    }
}
