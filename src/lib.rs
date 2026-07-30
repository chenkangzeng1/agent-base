//! phi-agent: General-purpose AI Agent framework
//!
//! Built on agent-base and agent-works, providing builder factory, renderer,
//! config resolution, session management, and other infrastructure.
//! **Does not bundle any tools** — tools are injected by consumers.

pub mod agent;
pub mod cli;
pub mod config;
pub mod event_log;
pub mod prompt;
pub mod render;
pub mod session;

// ── Common agent-base types ──
// Only re-export the types consumers use most often.
// For the full type set, import directly from agent-base.
pub use agent_base::{
    AgentBuilder, AgentError, AgentResult, AgentRuntime, ApprovalHandler, ConsecutiveFailureRecovery, LlmClient,
    OpenAiClient, PlanItem, PlanStepStatus, ReasoningConfig, ReasoningEffort, RunOutcome, RuntimeEvent, SafetyConfig,
    SessionId, Tool, ToolContext, ToolControlFlow, ToolOutput, TurnFactMiddleware, TurnToolLimitMiddleware,
    UpdatePlanTool,
};

// ── phi-telemetry (metrics types and storage) ──
#[cfg(feature = "telemetry")]
pub use phi_telemetry::{
    SessionMetrics, SessionOutcome, SessionSummary, TurnMetrics, TurnOutcome, list_all_metrics, load_metrics,
    save_metrics, try_load_metrics,
};

// ── agent-works ──
pub use agent_works::focus::{Context as FocusContext, Focus, FocusError, FocusInput, FocusOutput};

// ── phi-agent types ──
pub use agent::{PhiAgent, PhiAgentConfig, base_agent_builder};
pub use cli::{ApprovalMode, AutoApprovalHandler};
pub use config::{LlmConfig, resolve_llm_config};
pub use event_log::{event_to_jsonl, event_to_value, save_turn_log};
pub use prompt::{build_system_prompt, build_system_prompt_cn};
pub use render::{
    EventRenderer, JsonStreamRenderer, NullRenderer, OutputFormat, create_renderer, create_stdout_renderer,
};
pub use session::SessionContext;
