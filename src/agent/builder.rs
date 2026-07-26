//! General-purpose AgentBuilder factory — provides default configuration
//! shared across consumers.
//!
//! Returns a pre-configured AgentBuilder; callers then register tools and
//! approval handlers on top.

use std::sync::Arc;

use agent_base::{AgentBuilder, ConsecutiveFailureRecovery, Language, ReasoningConfig, ReasoningEffort};

/// Returns an AgentBuilder with sensible defaults:
/// - English
/// - Medium reasoning effort
/// - Thinking enabled
/// - Consecutive failure recovery (default 3 retries)
/// - Session limits (50/100/50k)
///
/// Callers are responsible for: registering tools, setting the approval
/// handler, setting the system prompt, then calling `.build()`.
pub fn base_agent_builder(llm_client: Arc<dyn agent_base::LlmClient>) -> AgentBuilder {
    AgentBuilder::new(llm_client)
        .language(Language::En)
        .reasoning(ReasoningConfig {
            effort: Some(ReasoningEffort::Medium),
            ..Default::default()
        })
        .enable_thought(true)
        .enable_thinking(true)
        .max_sessions(50)
        .max_turns_per_session(100)
        .max_message_tokens(50_000)
        .max_tool_output_chars(4000)
        .error_recovery(Arc::new(ConsecutiveFailureRecovery::new(3)))
}
