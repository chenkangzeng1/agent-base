//! Integration tests for phi-agent.
//!
//! These tests cover the public API without requiring a real LLM connection.

mod common;
use common::EmptyStream;

use std::pin::Pin;
use std::sync::Arc;

use agent_base::{
    AgentResult, ChatMessage, LlmCapabilities, LlmClient, ReasoningConfig, ReasoningEffort, ResponseFormat,
    StreamChunk, Tool, ToolContext, ToolControlFlow, ToolOutput,
};
use async_trait::async_trait;
use futures_core::Stream;
use phi_agent::{
    PhiAgentConfig, SafetyConfig, base_agent_builder, build_system_prompt, build_system_prompt_cn, resolve_llm_config,
    session::validate_session_id,
};
use serde_json::Value;

// ── Mock LLM client ──

/// A simple mock LLM client that always returns "mock response".
struct SimpleMockLlmClient;

#[async_trait]
impl LlmClient for SimpleMockLlmClient {
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
    let client = Arc::new(SimpleMockLlmClient);
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

// ── Tool metadata ──

struct CustomTool;

#[async_trait]
impl Tool for CustomTool {
    fn name(&self) -> &'static str {
        "custom_tool"
    }

    fn definition(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "custom_tool",
                "description": "A user-defined custom tool",
                "parameters": { "type": "object", "properties": {} }
            }
        })
    }

    async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        Ok(ToolOutput { summary: "ok".into(), raw: None, control_flow: ToolControlFlow::Continue, truncation: None })
    }
}

#[test]
fn test_list_tools_returns_metadata() {
    let client = Arc::new(SimpleMockLlmClient);
    let builder = base_agent_builder(client)
        .system_prompt("You are a helpful assistant.")
        .register_tool(agent_base::UpdatePlanTool::new())
        .register_tool(CustomTool);
    let config = PhiAgentConfig {
        model: "test-model".into(),
        enable_thinking: false,
        thinking_budget: None,
        thinking_effort: ReasoningEffort::Medium,
        safety: SafetyConfig::default(),
        max_turns: Some(100),
    };
    let agent = phi_agent::PhiAgent::build(builder, config).expect("build agent");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let tools = rt.block_on(agent.list_tools());

    assert!(!tools.is_empty(), "should return at least one tool");

    // Crate-backed tool
    let update_plan = tools.iter().find(|t| t.name == "update_plan").expect("update_plan should be registered");
    assert_eq!(update_plan.origin, "agent-base", "framework tool should report its crate origin");
    assert!(!update_plan.version.is_empty(), "framework tool should report a version");
    assert!(update_plan.version != "unknown", "framework tool version should not be 'unknown'");

    // Custom tool
    let custom = tools.iter().find(|t| t.name == "custom_tool").expect("custom_tool should be registered");
    assert_eq!(custom.origin, "custom", "user-defined tool origin should be 'custom'");
    assert_eq!(custom.version, "unknown", "user-defined tool version should be 'unknown'");
    assert!(custom.description.contains("user-defined"), "description should come from definition");
}

// ── PhiAgent lifecycle ──

fn build_test_agent() -> phi_agent::PhiAgent {
    let client = Arc::new(SimpleMockLlmClient);
    let builder = base_agent_builder(client).system_prompt("You are a helpful assistant.");
    let config = PhiAgentConfig {
        model: "test-model".into(),
        enable_thinking: false,
        thinking_budget: None,
        thinking_effort: ReasoningEffort::Medium,
        safety: SafetyConfig::default(),
        max_turns: Some(10),
    };
    phi_agent::PhiAgent::build(builder, config).expect("build agent")
}

#[test]
fn test_phi_agent_create_session() {
    let agent = build_test_agent();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let sid = rt.block_on(agent.create_session());
    assert!(sid.id > 0);
}

#[test]
fn test_phi_agent_is_cancelled_initially_false() {
    let agent = build_test_agent();
    assert!(!agent.is_cancelled());
}

#[test]
fn test_phi_agent_set_reasoning_effort() {
    let agent = build_test_agent();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(agent.set_reasoning_effort(ReasoningEffort::Low));
    // Should not panic
}

#[test]
fn test_phi_agent_list_tools_sorted() {
    let client = Arc::new(SimpleMockLlmClient);
    let builder = base_agent_builder(client)
        .system_prompt("You are a helpful assistant.")
        .register_tool(agent_base::UpdatePlanTool::new())
        .register_tool(CustomTool);
    let config = PhiAgentConfig {
        model: "test-model".into(),
        enable_thinking: false,
        thinking_budget: None,
        thinking_effort: ReasoningEffort::Medium,
        safety: SafetyConfig::default(),
        max_turns: Some(10),
    };
    let agent = phi_agent::PhiAgent::build(builder, config).expect("build agent");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let tools = rt.block_on(agent.list_tools());
    // Should be sorted by name
    for i in 1..tools.len() {
        assert!(
            tools[i - 1].name <= tools[i].name,
            "tools should be sorted: {} > {}",
            tools[i - 1].name,
            tools[i].name
        );
    }
}

// ── Phase 1: AgentError propagation test ──

/// Verify that phi-agent functions can be used with `?` in an
/// `anyhow::Result` context — AgentError implements std::error::Error
/// so the conversion is automatic.
#[test]
fn test_agent_error_converts_to_anyhow() {
    let err = validate_session_id("");
    assert!(err.is_err());

    // AgentError implements std::error::Error, so anyhow::Error::from
    // works automatically
    let anyhow_err: anyhow::Error = err.unwrap_err().into();
    assert!(anyhow_err.to_string().contains("Session ID"));
}
