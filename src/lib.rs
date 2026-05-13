//! agent-core: 通用 ReAct Agent 运行时框架
//!
//! 本库提供纯粹的、与业务无关的 AI Agent 运行时能力：
//! - 大模型对话 (LLM)
//! - 工具抽象与注册 (Tool / ToolRegistry)
//! - 工具调用的自动分发与审批 (ToolPolicy / ApprovalHandler)
//! - 基于事件流的进度外抛 (AgentEvent)
//! - 多轮对话上下文管理 (AgentSession)
//!
//! agent-core 零业务假设，不含任何 SSH / Linux / 文件操作等业务逻辑。
//! 可被 ops-agent、db-agent、browser-agent 等具体场景应用作为底层依赖使用。

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
    SessionId,
};

// ---------------------------------------------------------------------------
// LLM 大模型接入
// ---------------------------------------------------------------------------
pub use llm::{
    LlmClient,
    OpenAiClient,
    StreamChunk,
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
};

// ---------------------------------------------------------------------------
// 事件
// ---------------------------------------------------------------------------
pub use types::AgentEvent;

// ---------------------------------------------------------------------------
// 数据类型
// ---------------------------------------------------------------------------
pub use types::{
    AgentConfig,
    AgentResult,
    Message,
    MessageRole,
};
