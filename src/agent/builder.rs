//! General-purpose AgentBuilder factory — provides default configuration
//! shared across consumers.
//!
//! Returns a pre-configured AgentBuilder; callers then register tools and
//! approval handlers on top.

use std::sync::Arc;

use agent_base::{AgentBuilder, ConsecutiveFailureRecovery, Language, ReasoningConfig, ReasoningEffort};

use crate::agent::compression::SummarizingMiddleware;

/// Returns an AgentBuilder with sensible defaults:
/// - English
/// - Medium reasoning effort
/// - Thinking enabled
/// - Consecutive failure recovery (default 3 retries)
/// - Session limits (50 sessions / 100 turns per session / 50k per-message cap)
/// - Per-run react-loop cap (200 iterations for one user input)
/// - LLM-based context compression for long tool-heavy conversations
///
/// Callers are responsible for: registering tools, setting the approval
/// handler, setting the system prompt, then calling `.build()`.
pub fn base_agent_builder(llm_client: Arc<dyn agent_base::LlmClient>) -> AgentBuilder {
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

    AgentBuilder::new(llm_client.clone())
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
        .middleware(SummarizingMiddleware::new(llm_client))
}
