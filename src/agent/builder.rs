//! General-purpose AgentBuilder factory — provides default configuration
//! shared across consumers.
//!
//! Returns a pre-configured [`agent_works::AgentBuilder`]; callers then register
//! tools and approval handlers on top.

use std::sync::Arc;

use agent_base::{ConsecutiveFailureRecovery, Language, ReasoningConfig, ReasoningEffort};

use crate::agent::compression::SummarizingMiddleware;

/// Returns an [`agent_works::AgentBuilder`] with sensible defaults:
/// - English
/// - Medium reasoning effort
/// - Thinking enabled
/// - Consecutive failure recovery (default 3 retries)
/// - Session limits (50 sessions / 100 turns per session / 50k per-message cap)
/// - Per-run react-loop cap (200 iterations for one user input)
/// - LLM-based context compression for long tool-heavy conversations
/// - Multi-agent support enabled by default (6 tools: spawn/send/followup/wait/list/close)
/// - Skill support enabled by default
///
/// Callers are responsible for: registering additional tools, setting the approval
/// handler, setting the system prompt, then calling `.build()`.
///
/// To disable multi-agent: `.without_multi_agent()`
/// To disable skills: compile with `--no-default-features` and exclude `skill`.
pub fn base_agent_builder(llm_client: Arc<dyn agent_base::LlmClient>) -> agent_works::AgentBuilder {
    // Tool-output cap (default 4000 chars). Tune via PHI_MAX_TOOL_OUTPUT_CHARS for large
    // outputs (HTML, base64 images, long lists). Truncated results carry an explicit
    // "...(truncated)" suffix plus structured TruncationInfo from agent-base.
    let max_tool_output_chars = match std::env::var("PHI_MAX_TOOL_OUTPUT_CHARS") {
        Ok(value) => match value.trim().parse::<usize>() {
            Ok(n) => n,
            Err(_) => {
                tracing::warn!(
                    value = %value,
                    "PHI_MAX_TOOL_OUTPUT_CHARS is not a valid integer; falling back to default 4000"
                );
                4000
            },
        },
        Err(_) => 4000,
    };

    let mut builder = agent_works::AgentBuilder::new(llm_client.clone())
        .language(Language::En)
        .reasoning(ReasoningConfig { effort: Some(ReasoningEffort::Medium), ..Default::default() })
        .enable_thought(true)
        .enable_thinking(true)
        .max_sessions(50)
        .max_turns_per_session(100)
        .execution_max_turns(200)
        .max_message_tokens(50_000)
        .max_tool_output_chars(max_tool_output_chars)
        .error_recovery(Arc::new(ConsecutiveFailureRecovery::new(3)))
        // Summarise the earlier part of long conversations so per-call LLM context
        // stays bounded (see compression.rs). Override via the returned builder, or
        // build your own AgentBuilder to opt out.
        .middleware(SummarizingMiddleware::new(llm_client));

    // ── Multi-agent (enabled by default) ──
    #[cfg(feature = "multi-agent")]
    {
        use agent_works::multi_agent::MultiAgentConfig;
        builder =
            builder.with_multi_agent(MultiAgentConfig::default()).with_multi_agent_tool_factory(Arc::new(|runtime| {
                phi_kernel_tools::multi_agent::create_all_tools(runtime)
            }));
    }

    builder
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures_core::Stream;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    struct StubClient;
    struct EmptyStream;

    impl Stream for EmptyStream {
        type Item = agent_base::AgentResult<agent_base::StreamChunk>;
        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Poll::Ready(None)
        }
    }

    #[async_trait]
    impl agent_base::LlmClient for StubClient {
        async fn chat(
            &self,
            _messages: &[agent_base::ChatMessage],
            _tools: &[serde_json::Value],
            _reasoning: Option<&agent_base::ReasoningConfig>,
            _response_format: Option<&agent_base::ResponseFormat>,
        ) -> agent_base::AgentResult<serde_json::Value> {
            Ok(serde_json::json!({"choices":[{"message":{"content":"stub"}}]}))
        }
        async fn chat_stream(
            &self,
            _messages: &[agent_base::ChatMessage],
            _tools: &[serde_json::Value],
            _reasoning: Option<&agent_base::ReasoningConfig>,
            _response_format: Option<&agent_base::ResponseFormat>,
        ) -> agent_base::AgentResult<Pin<Box<dyn Stream<Item = agent_base::AgentResult<agent_base::StreamChunk>> + Send>>>
        {
            Ok(Box::pin(EmptyStream))
        }
        fn capabilities(&self) -> agent_base::LlmCapabilities {
            agent_base::LlmCapabilities {
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_thinking: true,
                max_context_tokens: Some(128_000),
                max_output_tokens: Some(16_384),
            }
        }
    }

    #[test]
    fn test_max_tool_output_chars_default() {
        unsafe { std::env::remove_var("PHI_MAX_TOOL_OUTPUT_CHARS") };
        let builder = base_agent_builder(Arc::new(StubClient));
        let _ = builder;
    }

    #[test]
    fn test_max_tool_output_chars_custom() {
        unsafe { std::env::set_var("PHI_MAX_TOOL_OUTPUT_CHARS", "8000") };
        let builder = base_agent_builder(Arc::new(StubClient));
        let _ = builder;
        unsafe { std::env::remove_var("PHI_MAX_TOOL_OUTPUT_CHARS") };
    }

    #[test]
    fn test_max_tool_output_chars_invalid_fallback() {
        unsafe { std::env::set_var("PHI_MAX_TOOL_OUTPUT_CHARS", "not-a-number") };
        let builder = base_agent_builder(Arc::new(StubClient));
        let _ = builder;
        unsafe { std::env::remove_var("PHI_MAX_TOOL_OUTPUT_CHARS") };
    }

    /// Verify that the default `base_agent_builder()` registers the 6 multi-agent tools.
    #[cfg(feature = "multi-agent")]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_base_agent_builder_registers_multi_agent_tools() {
        let builder = base_agent_builder(Arc::new(StubClient))
            .system_prompt("test");

        let runtime = builder.build().unwrap();

        let tools = tokio::task::block_in_place(|| {
            let tools = runtime.tools_mut();
            let guard = tools.blocking_read();
            guard.metadatas().into_iter().map(|m| m.name).collect::<Vec<String>>()
        });

        assert!(tools.contains(&"spawn_agent".to_string()), "expected spawn_agent tool");
        assert!(tools.contains(&"send_message".to_string()), "expected send_message tool");
        assert!(tools.contains(&"followup_task".to_string()), "expected followup_task tool");
        assert!(tools.contains(&"wait_agent".to_string()), "expected wait_agent tool");
        assert!(tools.contains(&"list_agents".to_string()), "expected list_agents tool");
        assert!(tools.contains(&"close_agent".to_string()), "expected close_agent tool");
    }

    /// Verify that `.without_multi_agent()` on the returned builder removes the tools.
    #[cfg(feature = "multi-agent")]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_base_agent_builder_without_multi_agent() {
        let builder = base_agent_builder(Arc::new(StubClient))
            .system_prompt("test")
            .without_multi_agent();

        let runtime = builder.build().unwrap();

        let tools = tokio::task::block_in_place(|| {
            let tools = runtime.tools_mut();
            let guard = tools.blocking_read();
            guard.metadatas().into_iter().map(|m| m.name).collect::<Vec<String>>()
        });

        assert!(!tools.contains(&"spawn_agent".to_string()), "spawn_agent should not be registered");
        assert!(!tools.contains(&"list_agents".to_string()), "list_agents should not be registered");
    }
}
