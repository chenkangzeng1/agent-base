use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::types::{FinishReason, SessionId};

/// Guard decision result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GuardAction {
    /// Continue loop, inject nudge message
    Continue(String),
    /// End loop, return done
    Done,
    /// End loop, return failure
    Fail(String),
}

/// Guard context information
#[derive(Debug, Clone)]
pub struct GuardCtx {
    pub session_id: SessionId,
    pub turn_count: u32,
    pub user_input: String,
    pub model_response: String,
    pub finish_reason: FinishReason,
    pub available_tools: Vec<String>,
    // RunState information
    pub reasoning_only_strikes: usize,
    pub empty_response_strikes: usize,
    pub run_has_tool_calls: bool,
}

/// React Loop Guard trait
///
/// Each method corresponds to an abnormal branch, returns GuardAction to decide next step.
/// Implementors can:
/// - Use Focus for intelligent judgment
/// - Use counters + thresholds
/// - Pass through or intercept directly
#[async_trait]
pub trait ReactLoopGuard: Send + Sync {
    /// Model returns only reasoning, no text/tool call
    async fn on_reasoning_only(&self, ctx: &GuardCtx) -> GuardAction;

    /// Model returns empty (no text, no reasoning, no tool call)
    async fn on_empty_response(&self, ctx: &GuardCtx) -> GuardAction;

    /// Model returns text-only (no tool call)
    ///
    /// Note: This method is called after middleware.
    /// If middleware has set follow_up_message, this branch won't be entered.
    async fn on_text_only(&self, ctx: &GuardCtx) -> GuardAction;
}

/// No-op guard for backward compatibility
pub struct NoopGuard;

#[async_trait]
impl ReactLoopGuard for NoopGuard {
    async fn on_reasoning_only(&self, _ctx: &GuardCtx) -> GuardAction {
        GuardAction::Done
    }

    async fn on_empty_response(&self, _ctx: &GuardCtx) -> GuardAction {
        GuardAction::Done
    }

    async fn on_text_only(&self, _ctx: &GuardCtx) -> GuardAction {
        GuardAction::Done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_guard_always_done() {
        let guard = NoopGuard;
        let ctx = GuardCtx {
            session_id: SessionId {
                id: 1,
                external_id: None,
            },
            turn_count: 1,
            user_input: "test".to_string(),
            model_response: "response".to_string(),
            finish_reason: FinishReason::Stop,
            available_tools: vec![],
            reasoning_only_strikes: 0,
            empty_response_strikes: 0,
            run_has_tool_calls: false,
        };

        assert!(matches!(
            guard.on_reasoning_only(&ctx).await,
            GuardAction::Done
        ));
        assert!(matches!(
            guard.on_empty_response(&ctx).await,
            GuardAction::Done
        ));
        assert!(matches!(guard.on_text_only(&ctx).await, GuardAction::Done));
    }
}
