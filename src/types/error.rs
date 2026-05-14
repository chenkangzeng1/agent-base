use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("LLM 调用失败: {0}")]
    Llm(String),

    #[error("LLM API 错误: {message}")]
    LlmApi { message: String },

    #[error("SSE 流错误: {0}")]
    LlmStream(String),

    #[error("JSON 解析错误: {0}")]
    Json(String),

    #[error("工具 '{name}' 未注册")]
    ToolNotFound { name: String },

    #[error("工具 '{name}' 参数解析失败: {raw}")]
    ToolArgsInvalid { name: String, raw: String },

    #[error("工具 '{name}' 执行失败: {source}")]
    ToolExecution {
        name: String,
        #[source]
        source: Box<AgentError>,
    },

    #[error("工具调用被审批拒绝: {tool_name}")]
    ApprovalDenied { tool_name: String },

    #[error("会话 {0} 不存在")]
    SessionNotFound(u64),

    #[error("达到最大轮次限制 ({limit})，强制停止")]
    MaxTurnsExceeded { limit: u32 },

    #[error("操作已取消")]
    Cancelled,

    #[error("内部错误: {0}")]
    Internal(String),
}

impl AgentError {
    pub fn llm(message: impl Into<String>) -> Self {
        Self::Llm(message.into())
    }

    pub fn json(message: impl Into<String>) -> Self {
        Self::Json(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    pub fn tool_not_found(name: impl Into<String>) -> Self {
        Self::ToolNotFound { name: name.into() }
    }

    pub fn session_not_found(id: u64) -> Self {
        Self::SessionNotFound(id)
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Llm(_) | Self::LlmApi { .. } | Self::LlmStream(_))
    }
}

pub type AgentResult<T> = Result<T, AgentError>;
