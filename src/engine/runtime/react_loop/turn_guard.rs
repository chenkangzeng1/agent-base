use std::time::Instant;

use crate::engine::react_loop_guard::{GuardAction, GuardCtx};
use crate::engine::runtime::plan_runner::RuntimeCore;
use crate::types::{AgentResult, FinishReason, MessageRole, RunOutcome, SessionId};

use super::turn::TurnFlow;
use super::turn_end::TurnEndCtx;

/// LLM turn metrics needed to fire turn-end callbacks in guard dispatch.
pub(super) struct TurnMetrics<'a> {
    pub ttft_ms: u64,
    pub llm_duration_ms: u64,
    pub usage: &'a Option<crate::llm::UsageInfo>,
    pub text_len: u64,
    pub has_thinking: bool,
}

/// Bundle of per-turn context shared across all guard dispatch paths.
pub(super) struct TurnCtx<'a> {
    pub session_id: &'a SessionId,
    pub turn_count: u32,
    pub user_input: &'a str,
    pub finish_reason: &'a FinishReason,
    pub available_tools: &'a [String],
    pub turn_start: Instant,
    pub model: &'a str,
}

impl RuntimeCore {
    // ── Guard context construction ──────────────────────────────────────

    pub(super) async fn build_guard_ctx(
        &self,
        turn_ctx: &TurnCtx<'_>,
        model_response: &str,
    ) -> GuardCtx {
        let session_data = self
            .session_manager
            .session_or_err(turn_ctx.session_id)
            .await
            .map(|s| {
                (
                    s.run_state.reasoning_only_strikes,
                    s.run_state.empty_response_strikes,
                    s.run_state.run_has_tool_calls,
                )
            })
            .unwrap_or((0, 0, false));

        GuardCtx {
            session_id: turn_ctx.session_id.clone(),
            turn_count: turn_ctx.turn_count,
            user_input: turn_ctx.user_input.to_string(),
            model_response: model_response.to_string(),
            finish_reason: turn_ctx.finish_reason.clone(),
            available_tools: turn_ctx.available_tools.to_vec(),
            reasoning_only_strikes: session_data.0,
            empty_response_strikes: session_data.1,
            run_has_tool_calls: session_data.2,
        }
    }

    // ── Guard dispatch for reasoning-only / empty-response ──────────────

    /// Shared dispatch for degenerate states (reasoning-only, empty-response).
    ///
    /// `Done` is treated as failure — accepting no-output as success defeats
    /// the strike-counter defence.
    pub(super) async fn dispatch_nudge_guard(
        &self,
        action: GuardAction,
        turn_ctx: &TurnCtx<'_>,
        metrics: &TurnMetrics<'_>,
        label: &str,
    ) -> AgentResult<TurnFlow> {
        match action {
            GuardAction::Fail(error) => {
                self.fire_guard_fail(turn_ctx, metrics, &error).await;
                Ok(TurnFlow::Done(RunOutcome::Failed { error }))
            }
            GuardAction::Continue(nudge) => {
                self.with_session_mut(turn_ctx.session_id, |session| {
                    session.push_message(MessageRole::User, &nudge);
                })
                .await?;
                Ok(TurnFlow::Continue)
            }
            GuardAction::Done => {
                let error = format!("guard returned Done for {label}");
                self.fire_guard_fail(turn_ctx, metrics, &error).await;
                Ok(TurnFlow::Done(RunOutcome::Failed { error }))
            }
        }
    }

    // ── Guard dispatch for text-only ────────────────────────────────────

    /// Dispatch for text-only responses where the guard decides if the task
    /// is complete.
    ///
    /// - `Done` → task complete (success)
    /// - `Continue` → flush UI via turn-end, then push nudge
    /// - `Fail` → guard failure
    pub(super) async fn dispatch_text_only_guard(
        &self,
        action: GuardAction,
        turn_ctx: &TurnCtx<'_>,
        metrics: &TurnMetrics<'_>,
    ) -> AgentResult<TurnFlow> {
        match action {
            GuardAction::Done => {
                tracing::info!(
                    session_id = turn_ctx.session_id.id,
                    turn = turn_ctx.turn_count,
                    "guard confirmed task complete, ending loop"
                );
                self.fire_turn_end(TurnEndCtx {
                    ttft_ms: metrics.ttft_ms,
                    llm_duration_ms: metrics.llm_duration_ms,
                    usage: metrics.usage,
                    text_length: metrics.text_len,
                    has_thinking: metrics.has_thinking,
                    llm_calls: 1,
                    ..TurnEndCtx::new(
                        turn_ctx.session_id,
                        turn_ctx.turn_count,
                        turn_ctx.turn_start,
                        turn_ctx.model,
                        turn_ctx.user_input,
                        RunOutcome::Completed,
                    )
                })
                .await;
                Ok(TurnFlow::Done(RunOutcome::Completed))
            }
            GuardAction::Continue(nudge) => {
                tracing::info!(
                    session_id = turn_ctx.session_id.id,
                    turn = turn_ctx.turn_count,
                    "guard says task incomplete, nudging to continue"
                );
                // Flush the current turn's output to the UI before
                // injecting the nudge.  Without this, the "half-done"
                // text is swallowed and the user sees nothing between
                // the previous tool calls and the next turn.
                self.fire_turn_end(TurnEndCtx {
                    ttft_ms: metrics.ttft_ms,
                    llm_duration_ms: metrics.llm_duration_ms,
                    usage: metrics.usage,
                    text_length: metrics.text_len,
                    has_thinking: metrics.has_thinking,
                    llm_calls: 1,
                    ..TurnEndCtx::new(
                        turn_ctx.session_id,
                        turn_ctx.turn_count,
                        turn_ctx.turn_start,
                        turn_ctx.model,
                        turn_ctx.user_input,
                        RunOutcome::Continuing,
                    )
                })
                .await;
                self.with_session_mut(turn_ctx.session_id, |session| {
                    // NOTE: Do NOT reset run_has_tool_calls here.
                    // Once tools have been called in this run, the flag
                    // should stay true until the next run starts.
                    // Otherwise the judge can be bypassed: after one
                    // "incomplete" verdict the flag resets, and if the
                    // model returns text-only without tools on the next
                    // turn, run_has_tool_calls=false skips the judge
                    // entirely — defeating the guard.
                    session.push_message(MessageRole::User, &nudge);
                })
                .await?;
                Ok(TurnFlow::Continue)
            }
            GuardAction::Fail(error) => {
                tracing::warn!(
                    session_id = turn_ctx.session_id.id,
                    turn = turn_ctx.turn_count,
                    error = %error,
                    "guard failed, ending loop"
                );
                self.fire_guard_fail(turn_ctx, metrics, &error).await;
                Ok(TurnFlow::Done(RunOutcome::Failed { error }))
            }
        }
    }

    // ── Shared turn-end fire helper ─────────────────────────────────────

    /// Fire turn-end with a `Failed` outcome — factored out from the three
    /// guard dispatch sites to eliminate duplication.
    async fn fire_guard_fail(
        &self,
        turn_ctx: &TurnCtx<'_>,
        metrics: &TurnMetrics<'_>,
        error: &str,
    ) {
        self.fire_turn_end(TurnEndCtx {
            ttft_ms: metrics.ttft_ms,
            llm_duration_ms: metrics.llm_duration_ms,
            usage: metrics.usage,
            text_length: metrics.text_len,
            has_thinking: metrics.has_thinking,
            llm_calls: 1,
            error_message: Some(error),
            ..TurnEndCtx::new(
                turn_ctx.session_id,
                turn_ctx.turn_count,
                turn_ctx.turn_start,
                turn_ctx.model,
                turn_ctx.user_input,
                RunOutcome::Failed {
                    error: error.to_string(),
                },
            )
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::react_loop_guard::{GuardAction, GuardCtx, ReactLoopGuard};
    use crate::types::{FinishReason, SessionId};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A mock guard that counts calls and returns configurable actions.
    struct MockGuard {
        reasoning_only_action: GuardAction,
        empty_response_action: GuardAction,
        text_only_action: GuardAction,
        reasoning_only_calls: AtomicUsize,
        empty_response_calls: AtomicUsize,
        text_only_calls: AtomicUsize,
    }

    impl MockGuard {
        fn new(
            reasoning_only_action: GuardAction,
            empty_response_action: GuardAction,
            text_only_action: GuardAction,
        ) -> Self {
            Self {
                reasoning_only_action,
                empty_response_action,
                text_only_action,
                reasoning_only_calls: AtomicUsize::new(0),
                empty_response_calls: AtomicUsize::new(0),
                text_only_calls: AtomicUsize::new(0),
            }
        }

        fn reasoning_only_calls(&self) -> usize {
            self.reasoning_only_calls.load(Ordering::Relaxed)
        }

        fn empty_response_calls(&self) -> usize {
            self.empty_response_calls.load(Ordering::Relaxed)
        }

        fn text_only_calls(&self) -> usize {
            self.text_only_calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait::async_trait]
    impl ReactLoopGuard for MockGuard {
        async fn on_reasoning_only(&self, _ctx: &GuardCtx) -> GuardAction {
            self.reasoning_only_calls.fetch_add(1, Ordering::Relaxed);
            self.reasoning_only_action.clone()
        }

        async fn on_empty_response(&self, _ctx: &GuardCtx) -> GuardAction {
            self.empty_response_calls.fetch_add(1, Ordering::Relaxed);
            self.empty_response_action.clone()
        }

        async fn on_text_only(&self, _ctx: &GuardCtx) -> GuardAction {
            self.text_only_calls.fetch_add(1, Ordering::Relaxed);
            self.text_only_action.clone()
        }
    }

    #[test]
    fn test_guard_action_clone() {
        let action = GuardAction::Continue("test".to_string());
        let cloned = action.clone();
        match cloned {
            GuardAction::Continue(msg) => assert_eq!(msg, "test"),
            _ => panic!("Expected Continue"),
        }

        let action = GuardAction::Done;
        let cloned = action.clone();
        assert!(matches!(cloned, GuardAction::Done));

        let action = GuardAction::Fail("error".to_string());
        let cloned = action.clone();
        match cloned {
            GuardAction::Fail(msg) => assert_eq!(msg, "error"),
            _ => panic!("Expected Fail"),
        }
    }

    #[test]
    fn test_guard_ctx_debug() {
        let ctx = GuardCtx {
            session_id: SessionId::new(1),
            turn_count: 1,
            user_input: "test".to_string(),
            model_response: "response".to_string(),
            finish_reason: FinishReason::Stop,
            available_tools: vec!["tool1".to_string()],
            reasoning_only_strikes: 0,
            empty_response_strikes: 0,
            run_has_tool_calls: false,
        };
        let debug_str = format!("{:?}", ctx);
        assert!(debug_str.contains("session_id"));
        assert!(debug_str.contains("turn_count"));
        assert!(debug_str.contains("user_input"));
    }

    #[test]
    fn test_mock_guard_counts() {
        let guard = MockGuard::new(
            GuardAction::Continue("nudge".to_string()),
            GuardAction::Continue("nudge".to_string()),
            GuardAction::Done,
        );

        assert_eq!(guard.reasoning_only_calls(), 0);
        assert_eq!(guard.empty_response_calls(), 0);
        assert_eq!(guard.text_only_calls(), 0);

        // Simulate calls
        let ctx = GuardCtx {
            session_id: SessionId::new(1),
            turn_count: 1,
            user_input: "test".to_string(),
            model_response: "response".to_string(),
            finish_reason: FinishReason::Stop,
            available_tools: vec![],
            reasoning_only_strikes: 0,
            empty_response_strikes: 0,
            run_has_tool_calls: false,
        };

        // Use tokio runtime to test async calls
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            guard.on_reasoning_only(&ctx).await;
            guard.on_empty_response(&ctx).await;
            guard.on_text_only(&ctx).await;
        });

        assert_eq!(guard.reasoning_only_calls(), 1);
        assert_eq!(guard.empty_response_calls(), 1);
        assert_eq!(guard.text_only_calls(), 1);
    }
}
