use std::sync::Arc;

use anyhow::Result;
use agent_base::{AgentBuilder, AgentRuntime, AgentResult, ReasoningEffort, RunOutcome, RuntimeEvent, SafetyConfig, SessionId};

use crate::agent::builder::base_agent_builder;

/// phi-agent configuration (tool-agnostic)
#[derive(Clone)]
pub struct PhiAgentConfig {
    pub model: String,
    pub enable_thinking: bool,
    pub thinking_budget: Option<u64>,
    pub thinking_effort: ReasoningEffort,
    pub safety: SafetyConfig,
}

/// A built Agent instance.
///
/// Wraps AgentRuntime with common operations behind a simpler API.
#[derive(Clone)]
pub struct PhiAgent {
    runtime: AgentRuntime,
    pub config: PhiAgentConfig,
}

impl PhiAgent {
    /// Create a pre-configured AgentBuilder.
    ///
    /// Equivalent to `base_agent_builder(llm_client).system_prompt(system_prompt)`,
    /// after which you register tools, middleware, and approval handlers,
    /// then call `Self::build`.
    pub fn builder(
        llm_client: Arc<dyn agent_base::LlmClient>,
        system_prompt: String,
    ) -> AgentBuilder {
        base_agent_builder(llm_client).system_prompt(system_prompt)
    }

    /// Build from an AgentBuilder.
    pub fn build(builder: AgentBuilder, config: PhiAgentConfig) -> Result<Self> {
        let runtime = builder.build()?;
        Ok(Self { runtime, config })
    }

    /// Create an agent session.
    pub async fn create_session(&self) -> SessionId {
        self.runtime.create_session().await
    }

    /// Execute one turn.
    pub async fn run_turn<F>(
        &self,
        session_id: SessionId,
        query: &str,
        on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        self.runtime.run_turn(session_id, query, on_event).await
    }

    /// Cancel the currently executing turn.
    pub fn cancel(&self) {
        self.runtime.cancel();
    }

    /// Check whether the agent has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.runtime.is_cancelled()
    }

    /// Set the reasoning effort.
    pub async fn set_reasoning_effort(&self, effort: ReasoningEffort) {
        self.runtime.set_reasoning_effort(effort).await;
    }
}
