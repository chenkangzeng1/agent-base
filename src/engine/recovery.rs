use async_trait::async_trait;

use crate::types::{AgentError, AgentResult, SessionId};

/// 工具执行失败后运行时采取的动作
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolErrorAction {
    /// 停止当前 run，以失败态结束
    Stop,
    /// 将错误信息回灌给模型，继续推理
    Retry,
}

/// 工具执行失败后的恢复策略
///
/// 默认使用 StopOnError，符合轻内核“默认保守、策略外置”的设计原则。
/// 上层 agent 可按需注入 RetryOnError 等自定义策略。
#[async_trait]
pub trait ToolErrorRecovery: Send + Sync {
    async fn on_error(
        &self,
        _session_id: &SessionId,
        _tool_names: &[String],
        _error: &AgentError,
    ) -> AgentResult<ToolErrorAction>;
}

/// 默认策略：工具失败后停止运行
///
/// 这是最保守的策略，内核只表达事实，不替上层做业务恢复决策。
pub struct StopOnError;

#[async_trait]
impl ToolErrorRecovery for StopOnError {
    async fn on_error(
        &self,
        _session_id: &SessionId,
        _tool_names: &[String],
        _error: &AgentError,
    ) -> AgentResult<ToolErrorAction> {
        Ok(ToolErrorAction::Stop)
    }
}

/// 工具失败后继续运行，将错误回灌给模型
///
/// 适用于需要模型自恢复的场景（如 code-agent、browser-agent）。
pub struct RetryOnError;

#[async_trait]
impl ToolErrorRecovery for RetryOnError {
    async fn on_error(
        &self,
        _session_id: &SessionId,
        _tool_names: &[String],
        _error: &AgentError,
    ) -> AgentResult<ToolErrorAction> {
        Ok(ToolErrorAction::Retry)
    }
}
