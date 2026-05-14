pub mod engine;
pub mod llm;
pub mod tool;
pub mod types;

// ---------------------------------------------------------------------------
// Agent 运行时
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
// LLM 大模型接入
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
// 审批
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
};

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------
pub use tool::{
    Tool,
    ToolContext,
    ToolControlFlow,
    ToolOutput,
    ToolPolicy,
    ToolRegistry,
    TypedTool,
};

// ---------------------------------------------------------------------------
// 事件
// ---------------------------------------------------------------------------
pub use types::AgentEvent;

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------
pub use types::{AgentError, AgentResult};

// ---------------------------------------------------------------------------
// 数据类型
// ---------------------------------------------------------------------------
pub use types::{
    AgentConfig,
    ChatMessage,
    Message,
    MessageRole,
    ResponseFormat,
    RetryConfig,
    ToolCallMessage,
};
