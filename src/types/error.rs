use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("LLM call failed: {0}")]
    Llm(String),

    #[error("LLM API error: {message}")]
    LlmApi { message: String },

    #[error("SSE stream error: {0}")]
    LlmStream(String),

    #[error("JSON parse error: {0}")]
    Json(String),

    #[error("Tool '{name}' not registered")]
    ToolNotFound { name: String },

    #[error("Tool '{name}' argument parsing failed: {raw}")]
    ToolArgsInvalid { name: String, raw: String },

    #[error("Tool '{name}' execution failed: {source}")]
    ToolExecution {
        name: String,
        #[source]
        source: Box<AgentError>,
    },

    #[error("Tool call rejected by approval: {tool_name}")]
    ApprovalDenied { tool_name: String },

    #[error("Session {0} not found")]
    SessionNotFound(u64),

    #[error("Max turns ({limit}) reached, stopping forcibly")]
    MaxTurnsExceeded { limit: u32 },

    #[error("Operation cancelled")]
    Cancelled,

    #[error("Internal error: {0}")]
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
