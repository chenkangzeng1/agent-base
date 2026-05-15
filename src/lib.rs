pub mod engine;
pub mod llm;
pub mod skill;
pub mod tool;
pub mod types;

// ---------------------------------------------------------------------------
// Agent Runtime
// ---------------------------------------------------------------------------
pub use engine::{
    AgentBuilder,
    AgentRuntime,
    AgentSession,
    InMemorySessionStore,
    SessionId,
    SessionStore,
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
    ToolErrorAction,
    ToolErrorRecovery,
};

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------
pub use tool::{
    McpClient,
    McpToolInfo,
    McpToolRegistry,
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
// Skill
// ---------------------------------------------------------------------------
pub use skill::{
    FullDetailPrompter,
    LazySkillPrompter,
    Skill,
    SkillPrompter,
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
    ImageAttachment,
    ImageDetail,
    Message,
    MessageRole,
    ResponseFormat,
    RetryConfig,
    RunOutcome,
    ToolCallMessage,
    ToolResultData,
};
