use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use crate::engine::runtime::llm_engine::LlmTurnResult;
use crate::engine::runtime::plan_runner::RuntimeCore;
use crate::types::{
    AgentResult, FinishReason, MessageRole, RunOutcome, RuntimeEvent, SessionId,
};

use super::turn_loop::TurnFlow;
use super::turn_end::TurnEndCtx;
use super::turn_guard::{TurnCtx, TurnMetrics};
use super::turn_loop::PostLlmMwResult;

impl RuntimeCore {
    // ── Dispatch ────────────────────────────────────────────────────────

    /// Dispatch one LLM turn result: push messages, run tool calls, fire the
    /// turn-end callback, and decide whether the loop continues or the turn ends.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn handle_llm_turn<F>(
        &self,
        session_id: &SessionId,
        user_input_owned: &str,
        tool_definitions: &[serde_json::Value],
        turn_count: u32,
        turn_start: std::time::Instant,
        model: &str,
        result: AgentResult<LlmTurnResult>,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: Arc<Mutex<F>>,
        all_user_inputs: &[String],
        max_turns: u32,
    ) -> AgentResult<TurnFlow>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send + 'static,
    {
        let LlmTurnResult {
            full_text,
            reasoning_text,
            is_tool_call,
            tool_calls,
            usage,
            finish_reason,
            ttft_ms,
            llm_duration_ms,
            reasoning_only,
            thinking_signature: _,
        } = match result {
            Ok(r) => r,
            Err(e) => return self.handle_llm_error(session_id, turn_count, turn_start, model, user_input_owned, e).await,
        };

        let finish_reason = FinishReason::from_raw(finish_reason.as_deref());
        tracing::info!(
            session_id = session_id.id,
            turn = turn_count,
            text_len = full_text.len(),
            is_tool_call = is_tool_call,
            tool_call_count = tool_calls.len(),
            "LLM turn result"
        );

        let text_len = full_text.len() as u64;
        let has_thinking = !reasoning_text.is_empty();
        let tool_calls_parsed: Vec<(String, String, String)> = tool_calls
            .iter()
            .map(|tc| {
                let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let name = tc.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()).unwrap_or("").to_string();
                let args = tc.get("function").and_then(|f| f.get("arguments")).and_then(|a| a.as_str()).unwrap_or("").to_string();
                (id, name, args)
            })
            .collect();
        let available_tools: Vec<String> = tool_definitions
            .iter()
            .filter_map(|d| d.get("function")?.get("name")?.as_str().map(|s| s.to_string()))
            .collect();

        let turn_ctx = TurnCtx {
            session_id,
            turn_count,
            user_input: user_input_owned,
            finish_reason: &finish_reason.clone(),
            available_tools: &available_tools,
            turn_start,
            model,
            all_user_inputs,
            max_turns,
        };
        let metrics = TurnMetrics { ttft_ms, llm_duration_ms, usage: &usage, text_len, has_thinking };

        if reasoning_only {
            return self.handle_reasoning_only(&turn_ctx, &metrics).await;
        }

        self.handle_post_llm_result(
            &turn_ctx,
            &metrics,
            full_text,
            reasoning_text,
            is_tool_call,
            tool_calls_parsed,
            &available_tools,
            turn_count,
            finish_reason,
            event_rx,
            on_event,
        )
        .await
    }

    // ── Branch handlers ─────────────────────────────────────────────────

    /// Reasoning-only: the model emitted thinking content but no answer and no
    /// tool call. Nudge it to commit; fail after consecutive strikes.
    async fn handle_reasoning_only(
        &self,
        turn_ctx: &TurnCtx<'_>,
        metrics: &TurnMetrics<'_>,
    ) -> AgentResult<TurnFlow> {
        let strikes = self
            .with_session_mut(turn_ctx.session_id, |session| {
                session.run_state.record_reasoning_only()
            })
            .await?;
        let thinking_disabled = self
            .with_session_mut(turn_ctx.session_id, |session| {
                session.run_state.thinking_disabled_for_rest_of_run
            })
            .await?;
        tracing::warn!(
            session_id = turn_ctx.session_id.id,
            turn = turn_ctx.turn_count,
            strikes,
            thinking_disabled,
            "reasoning-only response with tools available — nudging the model to commit"
        );

        let guard_ctx = self
            .build_guard_ctx(turn_ctx, "", true, false, false, thinking_disabled)
            .await;
        let decision = self.guard.on_turn(&guard_ctx).await;
        self.execute_guard_decision(decision, turn_ctx, metrics).await
    }

    /// Normal LLM response: apply post-LLM middleware, push messages, then
    /// dispatch to the appropriate outcome branch.
    #[allow(clippy::too_many_arguments)]
    async fn handle_post_llm_result<F>(
        &self,
        turn_ctx: &TurnCtx<'_>,
        metrics: &TurnMetrics<'_>,
        full_text: String,
        reasoning_text: String,
        is_tool_call: bool,
        tool_calls_parsed: Vec<(String, String, String)>,
        available_tools: &[String],
        turn_count: u32,
        finish_reason: FinishReason,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: Arc<Mutex<F>>,
    ) -> AgentResult<TurnFlow>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send + 'static,
    {
        let result = self
            .apply_post_llm_mw(
                turn_ctx.session_id,
                full_text,
                is_tool_call,
                tool_calls_parsed,
                available_tools,
                turn_count,
                finish_reason.clone(),
            )
            .await?;

        // Push text-only assistant message (when there are NO tool calls).
        if !result.skip_push && !result.full_text.is_empty() && !result.is_tool_call {
            let reasoning = reasoning_text.clone();
            self.with_session_mut(turn_ctx.session_id, |session| {
                if !reasoning.is_empty() {
                    session.push_assistant_with_reasoning(&result.full_text, &reasoning);
                } else {
                    session.push_message(MessageRole::Assistant, &result.full_text);
                }
            })
            .await?;
        }

        if let Some(follow_up) = result.follow_up_message {
            self.with_session_mut(turn_ctx.session_id, |session| {
                session.push_message(MessageRole::User, &follow_up);
            })
            .await?;
            return Ok(TurnFlow::Continue);
        }

        if result.full_text.is_empty() && !result.is_tool_call {
            return self.handle_empty_response(turn_ctx, metrics).await;
        }

        if result.is_tool_call && !result.tool_calls.is_empty() {
            return self
                .handle_tool_call_branch(
                    turn_ctx,
                    metrics,
                    result,
                    reasoning_text,
                    event_rx,
                    on_event,
                )
                .await;
        }

        if result.is_tool_call && result.tool_calls.is_empty() {
            return self.handle_incomplete_tool_call(turn_ctx, metrics).await;
        }

        self.handle_finish_anomaly(turn_ctx, metrics, &finish_reason, &result.full_text).await
    }

    /// Empty response: no text, no reasoning, no tool call. Nudge the model;
    /// fail after consecutive strikes.
    async fn handle_empty_response(
        &self,
        turn_ctx: &TurnCtx<'_>,
        metrics: &TurnMetrics<'_>,
    ) -> AgentResult<TurnFlow> {
        let strikes = self
            .with_session_mut(turn_ctx.session_id, |session| {
                session.run_state.record_empty_response()
            })
            .await?;
        tracing::warn!(
            session_id = turn_ctx.session_id.id,
            turn = turn_ctx.turn_count,
            strikes,
            "empty LLM response (no text, no reasoning, no tool call)"
        );

        let guard_ctx = self
            .build_guard_ctx(turn_ctx, "", false, true, false, false)
            .await;
        let decision = self.guard.on_turn(&guard_ctx).await;
        self.execute_guard_decision(decision, turn_ctx, metrics).await
    }

    /// Tool call branch: notify guard, handle RestoreThinking, then execute.
    async fn handle_tool_call_branch<F>(
        &self,
        turn_ctx: &TurnCtx<'_>,
        metrics: &TurnMetrics<'_>,
        result: PostLlmMwResult,
        reasoning_text: String,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: Arc<Mutex<F>>,
    ) -> AgentResult<TurnFlow>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send + 'static,
    {
        // 1. Notify guard about tool call
        let thinking_disabled = self
            .session_manager
            .session_or_err(turn_ctx.session_id)
            .await
            .map(|s| s.run_state.thinking_disabled_for_rest_of_run)
            .unwrap_or(false);
        let guard_ctx = self
            .build_guard_ctx(turn_ctx, "", false, false, false, thinking_disabled)
            .await;
        let decision = self.guard.on_tool_call(&guard_ctx).await;

        // 2. Handle RestoreThinking (update RunState)
        if matches!(
            decision,
            crate::engine::react_loop_guard::GuardDecision::RestoreThinking
        ) {
            self.execute_guard_decision(decision, turn_ctx, metrics)
                .await?;
        }

        // 3. Execute tools regardless of guard decision
        self.run_tool_turn(
            turn_ctx.session_id,
            turn_ctx.user_input,
            turn_ctx.turn_count,
            turn_ctx.turn_start,
            turn_ctx.model,
            result.tool_calls,
            turn_ctx.finish_reason,
            reasoning_text,
            result.full_text,
            metrics,
            event_rx,
            on_event,
        )
        .await
    }

    /// Incomplete tool call: model signalled tool_use but no complete tool calls
    /// were parsed (stream cut off mid-delta, JSON incomplete, etc.).
    async fn handle_incomplete_tool_call(
        &self,
        turn_ctx: &TurnCtx<'_>,
        metrics: &TurnMetrics<'_>,
    ) -> AgentResult<TurnFlow> {
        let strikes = self
            .with_session_mut(turn_ctx.session_id, |session| {
                session.run_state.record_empty_response()
            })
            .await?;
        tracing::warn!(
            session_id = turn_ctx.session_id.id,
            turn = turn_ctx.turn_count,
            strikes,
            "model signalled tool call but no complete tool calls were parsed"
        );
        let guard_ctx = self
            .build_guard_ctx(turn_ctx, "", false, true, false, false)
            .await;
        let decision = self.guard.on_turn(&guard_ctx).await;
        self.execute_guard_decision(decision, turn_ctx, metrics).await
    }

    /// Finish-reason anomaly: tool_use with no parsed tool calls, or truncated.
    async fn handle_finish_anomaly(
        &self,
        turn_ctx: &TurnCtx<'_>,
        metrics: &TurnMetrics<'_>,
        finish_reason: &FinishReason,
        full_text: &str,
    ) -> AgentResult<TurnFlow> {
        match finish_reason {
            FinishReason::ToolUse => {
                let error = "finish_reason=tool_use but no tool calls were parsed";
                tracing::warn!(
                    session_id = turn_ctx.session_id.id,
                    turn = turn_ctx.turn_count,
                    error,
                    "model signalled tool_use but no tool calls were parsed"
                );
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
                        RunOutcome::Failed { error: error.to_string() },
                    )
                })
                .await;
                Ok(TurnFlow::Done(RunOutcome::Failed { error: error.to_string() }))
            }
            FinishReason::Truncated { reason } => {
                let error = format!("response truncated ({:?})", reason);
                tracing::warn!(
                    session_id = turn_ctx.session_id.id,
                    turn = turn_ctx.turn_count,
                    ?reason,
                    "text-only response truncated by token limit"
                );
                self.fire_turn_end(TurnEndCtx {
                    ttft_ms: metrics.ttft_ms,
                    llm_duration_ms: metrics.llm_duration_ms,
                    usage: metrics.usage,
                    text_length: metrics.text_len,
                    has_thinking: metrics.has_thinking,
                    llm_calls: 1,
                    error_message: Some(&error),
                    ..TurnEndCtx::new(
                        turn_ctx.session_id,
                        turn_ctx.turn_count,
                        turn_ctx.turn_start,
                        turn_ctx.model,
                        turn_ctx.user_input,
                        RunOutcome::Failed { error: error.clone() },
                    )
                })
                .await;
                Ok(TurnFlow::Done(RunOutcome::Failed { error }))
            }
            _ => {
                // Text-only branch — use the guard to determine completion.
                let guard_ctx = self
                    .build_guard_ctx(turn_ctx, full_text, false, false, true, false)
                    .await;
                let decision = self.guard.on_turn(&guard_ctx).await;
                self.execute_guard_decision(decision, turn_ctx, metrics).await
            }
        }
    }

    /// LLM stream error: fire turn-end and persist session on cancellation.
    async fn handle_llm_error(
        &self,
        session_id: &SessionId,
        turn_count: u32,
        turn_start: std::time::Instant,
        model: &str,
        user_input_owned: &str,
        e: crate::types::AgentError,
    ) -> AgentResult<TurnFlow> {
        let stream_outcome = if e.is_cancelled() {
            RunOutcome::Cancelled
        } else {
            RunOutcome::Failed { error: e.to_string() }
        };
        self.fire_turn_end(TurnEndCtx {
            error_message: Some(&e.to_string()),
            ..TurnEndCtx::new(session_id, turn_count, turn_start, model, user_input_owned, stream_outcome)
        })
        .await;
        // Persist session on cancellation (LLM-stream path bypasses handle_tool_error)
        if e.is_cancelled()
            && let Ok(session) = self.session_manager.session_or_err(session_id).await
        {
            let _ = self.session_manager.session_store().save(&session).await;
        }
        Err(e)
    }

    // ── Guard decision execution ────────────────────────────────────────

    /// Execute a guard decision — fire events, inject nudge, or terminate.
    pub(super) async fn execute_guard_decision(
        &self,
        decision: crate::engine::react_loop_guard::GuardDecision,
        turn_ctx: &TurnCtx<'_>,
        metrics: &TurnMetrics<'_>,
    ) -> AgentResult<TurnFlow> {
        use crate::engine::react_loop_guard::GuardDecision;
        match decision {
            GuardDecision::Continue { nudge } => {
                tracing::info!(
                    session_id = turn_ctx.session_id.id,
                    turn = turn_ctx.turn_count,
                    "guard decided to continue"
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
                        RunOutcome::Continuing,
                    )
                })
                .await;
                if let Some(msg) = nudge {
                    self.with_session_mut(turn_ctx.session_id, |session| {
                        session.push_message(MessageRole::User, &msg);
                    })
                    .await?;
                }
                Ok(TurnFlow::Continue)
            }
            GuardDecision::Complete => {
                tracing::info!(
                    session_id = turn_ctx.session_id.id,
                    turn = turn_ctx.turn_count,
                    "guard decided task is complete"
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
            GuardDecision::Fail { error } => {
                tracing::warn!(
                    session_id = turn_ctx.session_id.id,
                    turn = turn_ctx.turn_count,
                    error = %error,
                    "guard decided to fail"
                );
                self.fire_guard_fail(turn_ctx, metrics, &error).await;
                Ok(TurnFlow::Done(RunOutcome::Failed { error }))
            }
            GuardDecision::DisableThinking { nudge } => {
                self.with_session_mut(turn_ctx.session_id, |session| {
                    session.run_state.thinking_disabled_for_rest_of_run = true;
                    session.push_message(MessageRole::User, &nudge);
                })
                .await?;

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

                tracing::info!(
                    session_id = turn_ctx.session_id.id,
                    turn = turn_ctx.turn_count,
                    "thinking disabled by guard"
                );
                Ok(TurnFlow::Continue)
            }
            GuardDecision::RestoreThinking => {
                let original = self
                    .session_manager
                    .session_or_err(turn_ctx.session_id)
                    .await
                    .map(|s| s.run_state.original_thinking_enabled)
                    .unwrap_or(false);

                self.with_session_mut(turn_ctx.session_id, |session| {
                    session.run_state.thinking_disabled_for_rest_of_run = !original;
                })
                .await?;

                tracing::info!(
                    session_id = turn_ctx.session_id.id,
                    turn = turn_ctx.turn_count,
                    thinking_enabled = original,
                    "thinking restored by guard"
                );
                Ok(TurnFlow::Continue)
            }
        }
    }
}
