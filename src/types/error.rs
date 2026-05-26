use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("LLM call failed: {0}")]
    Llm(String),

    #[error("LLM API error: {message}")]
    LlmApi { message: String },

    #[error("LLM rate limit exceeded")]
    RateLimitExceeded,

    #[error("LLM service unavailable: {0}")]
    ServiceUnavailable(String),

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

    #[error("Tool timeout exceeded")]
    ToolTimeout,

    #[error("Tool call rejected by approval: {tool_name}")]
    ApprovalDenied { tool_name: String },

    #[error("Session {0} not found")]
    SessionNotFound(u64),

    #[error("Max turns ({limit}) reached, stopping forcibly")]
    MaxTurnsExceeded { limit: u32 },

    #[error("Operation cancelled")]
    Cancelled,

    #[error("Plan error: {0}")]
    Plan(String),

    #[error("Plan step '{step_id}' failed: {message}")]
    PlanStepFailed { step_id: String, message: String },

    #[error("Plan generation failed: {0}")]
    PlanGeneration(String),

    #[error("Plan storage error: {0}")]
    PlanStorage(String),

    #[error("Resource unavailable: {0}")]
    ResourceUnavailable(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

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

    pub fn plan(message: impl Into<String>) -> Self {
        Self::Plan(message.into())
    }

    pub fn plan_step_failed(step_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self::PlanStepFailed {
            step_id: step_id.into(),
            message: message.into(),
        }
    }

    pub fn plan_generation(message: impl Into<String>) -> Self {
        Self::PlanGeneration(message.into())
    }

    pub fn plan_storage(message: impl Into<String>) -> Self {
        Self::PlanStorage(message.into())
    }

    pub fn tool_timeout() -> Self {
        Self::ToolTimeout
    }

    pub fn rate_limit_exceeded() -> Self {
        Self::RateLimitExceeded
    }

    pub fn service_unavailable(message: impl Into<String>) -> Self {
        Self::ServiceUnavailable(message.into())
    }

    pub fn resource_unavailable(message: impl Into<String>) -> Self {
        Self::ResourceUnavailable(message.into())
    }

    pub fn config_error(message: impl Into<String>) -> Self {
        Self::ConfigError(message.into())
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Llm(_) | Self::LlmApi { .. } | Self::LlmStream(_) | Self::ServiceUnavailable(_) | Self::RateLimitExceeded)
    }

    pub fn is_rate_limited(&self) -> bool {
        matches!(self, Self::RateLimitExceeded)
    }

    pub fn is_resource_unavailable(&self) -> bool {
        matches!(self, Self::ResourceUnavailable(_))
    }
}

pub type AgentResult<T> = Result<T, AgentError>;
