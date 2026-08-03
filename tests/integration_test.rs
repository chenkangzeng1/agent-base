//! Integration tests for phi-agent.
//!
//! These tests cover the public API without requiring a real LLM connection.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use agent_base::{
    AgentResult, ChatMessage, LlmCapabilities, LlmClient, ReasoningConfig, ReasoningEffort, ResponseFormat, StreamChunk,
};
use async_trait::async_trait;
use futures_core::Stream;
use phi_agent::{
    PhiAgentConfig, SafetyConfig, base_agent_builder, build_system_prompt, build_system_prompt_cn, resolve_llm_config,
    session::validate_session_id,
};
use serde_json::Value;

// ── Mock LLM client ──

/// An empty stream that immediately returns `Poll::Ready(None)`.
struct EmptyStream;

impl Stream for EmptyStream {
    type Item = AgentResult<StreamChunk>;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}

struct MockLlmClient;

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Value> {
        Ok(serde_json::json!({
            "choices": [{"message": {"content": "mock response"}}]
        }))
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[Value],
        _reasoning: Option<&ReasoningConfig>,
        _response_format: Option<&ResponseFormat>,
    ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
        // Return an empty stream — good enough for construction tests
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

// ── Agent builder ──

#[test]
fn test_base_agent_builder_constructs() {
    let client = Arc::new(MockLlmClient);
    let builder = base_agent_builder(client)
        .system_prompt("You are a helpful assistant.")
        .register_tool(agent_base::UpdatePlanTool::new());
    let config = PhiAgentConfig {
        model: "test-model".into(),
        enable_thinking: false,
        thinking_budget: None,
        thinking_effort: ReasoningEffort::Medium,
        safety: SafetyConfig::default(),
        max_turns: Some(100),
    };
    let agent = phi_agent::PhiAgent::build(builder, config);
    assert!(agent.is_ok(), "Agent should build successfully");
}

// ── System prompt ──

#[test]
fn test_build_system_prompt_non_empty() {
    let prompt = build_system_prompt();
    assert!(!prompt.is_empty(), "System prompt should not be empty");
    assert!(prompt.len() > 100, "System prompt should be substantial");
}

#[test]
fn test_build_system_prompt_cn_non_empty() {
    let prompt = build_system_prompt_cn();
    assert!(!prompt.is_empty(), "Chinese system prompt should not be empty");
}

// ── Config ──

#[test]
fn test_resolve_llm_config_with_env() {
    // Without env vars set, should fall back gracefully
    let result = resolve_llm_config(None, None);
    // May error if no env vars — that's expected behavior
    // The important thing is it doesn't panic
    let _ = result;
}

// ── Session ──

#[test]
fn test_validate_session_id_valid() {
    assert!(validate_session_id("my-session-123").is_ok());
    assert!(validate_session_id("test_456").is_ok());
    assert!(validate_session_id("a").is_ok());
}

#[test]
fn test_validate_session_id_invalid() {
    assert!(validate_session_id("").is_err());
    assert!(validate_session_id("my session").is_err());
    assert!(validate_session_id("../etc").is_err());
    assert!(validate_session_id("path/traversal").is_err());
}

// ── PhiAgentConfig ──

#[test]
fn test_config_default_values() {
    let config = PhiAgentConfig {
        model: "opus".into(),
        enable_thinking: true,
        thinking_budget: Some(32000),
        thinking_effort: ReasoningEffort::High,
        safety: SafetyConfig::default(),
        max_turns: None,
    };
    assert_eq!(config.model, "opus");
    assert!(config.enable_thinking);
    assert_eq!(config.thinking_budget, Some(32000));
}
