pub mod engine;
pub mod llm;
pub mod tool;
pub mod types;

// ---------------------------------------------------------------------------
// Agent Runtime
// ---------------------------------------------------------------------------
pub use engine::{
    AbortOnFailure,
    AgentBuilder,
    AgentRuntime,
    AgentSession,
    AlwaysContinue,
    AlternativeAction,
    InMemoryPlanStore,
    InMemorySessionStore,
    PlanExecTool,
    PlanGenerator,
    PlanOrchestrator,
    PlanStore,
    RecoveryStrategy,
    ReflectionResult,
    ReflexionContext,
    ReflexionHandler,
    SessionId,
    SessionStore,
    StepContinuePolicy,
    StepExecutor,
    StepHistoryEntry,
    StreamingJsonParser,
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
// Events
// ---------------------------------------------------------------------------
pub use types::AgentEvent;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------
pub use types::{AgentError, AgentResult};

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
    PlanStatus,
    PlanStep,
    PlanStoreData,
    RecoveryAction,
    ResponseFormat,
    RetryConfig,
    RunOutcome,
    StepResult,
    StepStatus,
    ToolCallMessage,
    ToolResultData,
};
