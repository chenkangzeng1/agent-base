pub mod engine;
pub mod llm;
pub mod tool;
pub mod types;

// ---------------------------------------------------------------------------
// Prelude — most commonly used types for `use agent_base::prelude::*`
// ---------------------------------------------------------------------------
pub mod prelude {
    pub use crate::engine::{
        AgentBuilder, AgentRuntime, AgentSession, AllowAllApprovalHandler, DenyAllApprovalHandler,
    };
    pub use crate::llm::{AnthropicClient, LlmClient, LlmClientBuilder, OpenAiClient};
    pub use crate::tool::{
        AutoContinueTool, SubAgentTool, Tool, ToolContext, ToolMetadata, ToolOutput, ToolPolicy,
        ToolRegistry, TypedTool,
    };
    pub use crate::types::{
        AgentConfig, AgentError, AgentResult, ChatMessage, Language, Message, MessageRole,
        RunOutcome, RuntimeEvent, SessionId, TurnContext, UserEvent,
    };
}

// ---------------------------------------------------------------------------
// Agent Runtime
// ---------------------------------------------------------------------------
pub use engine::{
    AgentBuilder, AgentRuntime, AgentSession, CircuitBreaker, CircuitState, DefaultPipeline,
    InMemorySessionStore, SessionId, SessionStore, ToolExecutionPipeline,
};

// ---------------------------------------------------------------------------
// LLM Provider
// ---------------------------------------------------------------------------
pub use llm::{
    AnthropicClient, LlmCapabilities, LlmClient, LlmClientBuilder, LlmProvider, OpenAiClient,
    ReasoningConfig, ReasoningEffort, StreamChunk, UsageInfo,
};

// ---------------------------------------------------------------------------
// Approval
// ---------------------------------------------------------------------------
pub use engine::{
    AllowAllApprovalHandler, ApprovalDecision, ApprovalHandler, ApprovalRequest,
    ConsecutiveFailureRecovery, ContextWindowManager, DenyAllApprovalHandler, Middleware,
    PostLlmCtx, PreLlmCtx, RetryOnError, RiskLevel, StopOnError, ToolEnforcementConfig,
    ToolEnforcementMiddleware, ToolErrorAction, ToolErrorRecovery, TurnFactMiddleware,
    TurnToolLimitMiddleware, UserMessageCtx,
};

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------
pub use tool::{
    AutoContinueTool, SubAgentSessionPolicy, SubAgentTool, Tool, ToolContext, ToolControlFlow,
    ToolMetadata, ToolOutput, ToolPolicy, ToolRegistry, TypedTool, UpdatePlanTool,
};

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------
pub use types::{RuntimeEvent, UserEvent};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------
pub use types::{AgentError, AgentResult, ErrorKind};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------
pub use types::{
    AgentConfig, ChatMessage, CheckpointData, CheckpointStep, ImageAttachment, ImageDetail,
    Language, Message, MessageRole, PlanItem, PlanStepStatus, ResponseFormat, RetryConfig,
    RunOutcome, SafetyConfig, SessionConfig, ToolCallMessage, ToolResultData, TurnContext,
    UpdatePlanArgs,
};
