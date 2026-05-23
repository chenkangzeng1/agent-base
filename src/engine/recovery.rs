use crate::types::{AgentError, AgentResult, SessionId};

/// Action taken by the runtime after a tool execution failure
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolErrorAction {
    /// Stop the current run with a failed outcome
    Stop,
    /// Feed the error back to the LLM and continue reasoning
    Retry,
}

/// Recovery strategy after a tool execution failure
///
/// Defaults to [`StopOnError`], following the lightweight kernel design of
/// conservative defaults and strategy injection.
/// Upper-layer agents can inject custom strategies such as [`RetryOnError`].
pub trait ToolErrorRecovery: Send + Sync {
    fn on_error(
        &self,
        _session_id: &SessionId,
        _tool_names: &[String],
        _error: &AgentError,
    ) -> AgentResult<ToolErrorAction>;
}

/// Default strategy: stop on tool failure.
///
/// This is the most conservative strategy. The kernel only reports the fact
/// without making business recovery decisions for the upper layer.
pub struct StopOnError;

impl ToolErrorRecovery for StopOnError {
    fn on_error(
        &self,
        _session_id: &SessionId,
        _tool_names: &[String],
        _error: &AgentError,
    ) -> AgentResult<ToolErrorAction> {
        Ok(ToolErrorAction::Stop)
    }
}

/// Continue on tool failure, feeding the error back to the model
///
/// Suitable for scenarios where model self-healing is desired (e.g. code-agent, browser-agent).
pub struct RetryOnError;

impl ToolErrorRecovery for RetryOnError {
    fn on_error(
        &self,
        _session_id: &SessionId,
        _tool_names: &[String],
        _error: &AgentError,
    ) -> AgentResult<ToolErrorAction> {
        Ok(ToolErrorAction::Retry)
    }
}
