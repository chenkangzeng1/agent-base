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
    pub use crate::llm::{
        AnthropicAdapter, LlmClient, LlmClientBuilder, OpenAiAdapter, OpenAiClient, StreamClient,
    };
    pub use crate::tool::{
        AutoContinueTool, Content, DenyAllToolPolicy, Tool, ToolContext, ToolMetadata, ToolPolicy,
        ToolRegistry, TypedTool,
    };
    pub use crate::types::{
        AgentConfig, AgentError, AgentResult, ChatMessage, FinishReason, Language, Message,
        MessageRole, RunOutcome, RuntimeEvent, SessionId, TurnContext, UserEvent,
    };
}

// ---------------------------------------------------------------------------
// Agent Runtime
// ---------------------------------------------------------------------------
pub use engine::{
    AgentBuilder, AgentRuntime, AgentSession, DefaultPipeline, InMemorySessionStore, QueueMode,
    SessionId, SessionStore, ToolExecutionPipeline,
};

// ---------------------------------------------------------------------------
// LLM Provider
// ---------------------------------------------------------------------------
pub use llm::{
    AnthropicAdapter, LlmCapabilities, LlmClient, LlmClientBuilder, LlmProvider, OpenAiAdapter,
    OpenAiClient, ReasoningConfig, ReasoningEffort, StreamChunk, StreamClient, UsageInfo,
};

// ---------------------------------------------------------------------------
// Approval
// ---------------------------------------------------------------------------
pub use engine::{
    AllowAllApprovalHandler, ApprovalDecision, ApprovalHandler, ApprovalRequest,
    ConsecutiveFailureRecovery, ContextWindowManager, DenyAllApprovalHandler, GuardAction,
    GuardCtx, Middleware, NoopGuard, PostLlmCtx, PreLlmCtx, ReactLoopGuard, RetryOnError,
    RiskLevel, StopOnError, ToolEnforcementConfig, ToolEnforcementMiddleware, ToolErrorAction,
    ToolErrorRecovery, TurnFactMiddleware, TurnToolLimitMiddleware, UserMessageCtx,
};

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------
pub use tool::{
    AutoContinueTool, Content, DenyAllToolPolicy, Tool, ToolContext, ToolMetadata, ToolPolicy,
    ToolRegistry, TypedTool, UpdatePlanTool,
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
    AgentConfig, ChatMessage, CheckpointData, CheckpointStep, FinishReason, ImageAttachment,
    ImageDetail, Language, Message, MessageRole, PlanItem, PlanStepStatus, ResponseFormat,
    RetryConfig, RunOutcome, SafetyConfig, SessionConfig, ToolCallMessage, ToolResultData,
    TurnContext, UpdatePlanArgs,
};
