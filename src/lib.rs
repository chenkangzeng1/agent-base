pub mod engine;
pub mod llm;
pub mod tool;
pub mod types;
pub mod config_manager;

// ---------------------------------------------------------------------------
// Prelude — most commonly used types for `use agent_base::prelude::*`
// ---------------------------------------------------------------------------
pub mod prelude {
    pub use crate::engine::{
        AgentBuilder, AgentRuntime, AgentSession, AllowAllApprovalHandler,
        DenyAllApprovalHandler,
    };
    pub use crate::llm::{
        AnthropicClient, LlmClient, LlmClientBuilder, OpenAiClient,
    };
    pub use crate::tool::{
        AutoContinueTool, SubAgentTool, Tool, ToolContext, ToolOutput,
        ToolPolicy, ToolRegistry, TypedTool,
    };
    pub use crate::types::{
        AgentConfig, AgentError, AgentResult, ChatMessage, Language,
        Message, MessageRole, RunOutcome, RuntimeEvent, SessionId, UserEvent,
    };
    pub use crate::config_manager::AppConfig;
}

// ---------------------------------------------------------------------------
// Agent Runtime
// ---------------------------------------------------------------------------
pub use engine::{
    AbortOnFailure,
    AdaptiveRecoveryStrategy,
    AgentBuilder,
    AgentRuntime,
    AgentSession,
    AlwaysContinue,
    AlternativeAction,
    CircuitBreaker,
    CircuitState,
    CustomRecovery,
    DefaultPipeline,
    InMemoryPlanStore,
    InMemorySessionStore,
    LlmAdaptiveRecovery,
    LlmPlanGenerator,
    PlanConfig,
    PlanExecTool,
    PlanGenerator,
    PlanOptions,
    PlanOrchestrator,
    PlanStore,
    Recovery,
    RecoveryPolicy,
    RecoveryStrategy,
    ReflectionResult,
    ReflexionContext,
    ReflexionHandler,
    RetryOnFailure,
    SessionId,
    SessionStore,
    SkipOnFailure,
    StepContinuePolicy,
    StepExecutor,
    StepHistoryEntry,
    StreamingJsonParser,
    ToolCallingStepExecutor,
    ToolExecutionPipeline,
};

// ---------------------------------------------------------------------------
// LLM Provider
// ---------------------------------------------------------------------------
pub use llm::{
    AnthropicClient,
    LlmCapabilities,
    LlmClient,
    LlmClientBuilder,
    LlmProvider,
    OpenAiClient,
    ReasoningConfig,
    ReasoningEffort,
    StreamChunk,
    UsageInfo,
};

// ---------------------------------------------------------------------------
// Approval
// ---------------------------------------------------------------------------
pub use engine::{
    AllowAllApprovalHandler,
    ApprovalDecision,
    ApprovalHandler,
    ApprovalRequest,
    DenyAllApprovalHandler,
    Middleware,
    PostLlmCtx,
    PreLlmCtx,
    UserMessageCtx,
    ContextWindowManager,
    RiskLevel,
    RetryOnError,
    StopOnError,
    ToolEnforcementConfig,
    ToolEnforcementMiddleware,
    ToolErrorAction,
    ToolErrorRecovery,
};

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------
pub use tool::{
    AutoContinueTool,
    SubAgentSessionPolicy,
    SubAgentTool,
    Tool,
    ToolContext,
    ToolControlFlow,
    ToolOutput,
    ToolPolicy,
    ToolRegistry,
    TypedTool,
};

// ---------------------------------------------------------------------------
// Config Manager
// ---------------------------------------------------------------------------
pub use config_manager::{AppConfig, ConfigManager, ConfigSource, ConfigError};

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
    AgentConfig,
    ChatMessage,
    CheckpointData,
    CheckpointStep,
    ExecutionPlan,
    ImageAttachment,
    ImageDetail,
    Language,
    Message,
    MessageRole,
    PhaseStatus,
    PlanPhase,
    PlanStatus,
    PlanStep,
    PlanStoreData,
    RecoveryAction,
    RecoveryContext,
    ResponseFormat,
    RetryConfig,
    RunOutcome,
    SessionConfig,
    StepResult,
    StepStatus,
    ToolCallMessage,
    ToolResultData,
};
