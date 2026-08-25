use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use crate::engine::middleware::{PostLlmCtx, PreLlmCtx};
use crate::engine::runtime::llm_engine::LlmTurnResult;
use crate::engine::runtime::plan_runner::RuntimeCore;
use crate::types::{
    AgentResult, CheckpointData, CheckpointStep, FinishReason, MessageRole, RunOutcome,
    RuntimeEvent, SessionId, UserEvent, default_convert_to_llm,
};

use super::entry::drain_locked;
use super::turn_end::TurnEndCtx;
use super::turn_guard::{TurnCtx, TurnMetrics};

/// Control-flow result of one `handle_llm_turn` call: keep looping, or
/// terminate the turn with a final outcome.
pub(super) enum TurnFlow {
    Continue,
    Done(RunOutcome),
}

struct PostLlmMwResult {
    pub full_text: String,
    pub is_tool_call: bool,
    pub tool_calls: Vec<(String, String, String)>,
    pub skip_push: bool,
    pub follow_up_message: Option<String>,
}

impl RuntimeCore {
    async fn apply_pre_llm_mw<F>(
        &self,
        session_id: &SessionId,
        messages: Vec<crate::types::ChatMessage>,
        tools: Vec<serde_json::Value>,
        on_event: Arc<Mutex<F>>,
    ) -> AgentResult<(Vec<crate::types::ChatMessage>, Vec<serde_json::Value>)>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send + 'static,
    {
        // Build a unified emit function that sends UserEvents to both:
        // 1. on_event (real-time renderer callback)
        // 2. event_bus (persistence, event_log, checkpoint)
        let emit_fn: Box<dyn Fn(UserEvent) + Send + Sync> = {
            let eb = self.event_bus.clone();
            let sid = session_id.clone();
            let cb = on_event.clone();
            Box::new(move |ev: UserEvent| {
                // Real-time render
                if let Ok(mut cb) = cb.lock() {
                    let _ = cb(RuntimeEvent::UserEvent {
                        session_id: sid.clone(),
                        event: ev.clone(),
                        agent_id: None,
                        trace_id: None,
                    });
                }
                // Persistence
                eb.emit(RuntimeEvent::UserEvent {
                    session_id: sid.clone(),
                    event: ev,
                    agent_id: None,
                    trace_id: None,
                });
            })
        };

        let mut ctx = PreLlmCtx {
            session_id: session_id.clone(),
            messages,
            tools,
            emit_fn: Some(emit_fn),
        };

        for mw in &self.middlewares {
            mw.on_pre_llm(&mut ctx).await?;
        }

        // No drain needed — ctx.emit() already delivered events directly.
        Ok((ctx.messages, ctx.tools))
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_post_llm_mw(
        &self,
        session_id: &SessionId,
        full_text: String,
        is_tool_call: bool,
        tool_calls: Vec<(String, String, String)>,
        available_tools: &[String],
        turn_count: u32,
        finish_reason: FinishReason,
    ) -> AgentResult<PostLlmMwResult> {
        let session = self.session_manager.session_or_err(session_id).await?;
        let total_tool_calls = session.total_tool_calls;
        let nudge_count = session.run_state.nudge_count;
        let turn_tool_calls = session.run_state.turn_tool_calls;
        drop(session);
        let mut ctx = PostLlmCtx {
            session_id: session_id.clone(),
            full_text,
            is_tool_call,
            tool_calls,
            available_tools: available_tools.to_vec(),
            turn_count,
            total_tool_calls,
            nudge_count,
            turn_tool_calls,
            skip_push: false,
            follow_up_message: None,
            finish_reason,
        };
        for mw in &self.middlewares {
            mw.on_post_llm(&mut ctx).await?;
        }
        // Write back nudge_count if middleware modified it
        if ctx.nudge_count != nudge_count {
            self.with_session_mut(session_id, |session| {
                session.run_state.nudge_count = ctx.nudge_count;
            })
            .await?;
        }
        Ok(PostLlmMwResult {
            full_text: ctx.full_text,
            is_tool_call: ctx.is_tool_call,
            tool_calls: ctx.tool_calls,
            skip_push: ctx.skip_push,
            follow_up_message: ctx.follow_up_message,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_turn_loop<F>(
        &self,
        session_id: &SessionId,
        user_input_owned: &str,
        tool_definitions: &[serde_json::Value],
        mut turn_count: u32,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: Arc<Mutex<F>>,
        all_user_inputs: &[String],
    ) -> AgentResult<(RunOutcome, u32)>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send + 'static,
    {
        let config = self.config_snapshot_async().await;
        let max_turns = config
            .execution
            .max_turns
            .unwrap_or(crate::engine::runtime::DEFAULT_MAX_TURNS);

        tracing::debug!(session_id = session_id.id, max_turns, "run turn loop start");

        loop {
            turn_count += 1;
            let turn_start = std::time::Instant::now();
            let model = self.llm_engine.get_provider().info().model.clone();

            // Check for cancellation at the top of each iteration
            if self.is_cancelled() {
                tracing::info!(session_id = session_id.id, "run_turn_loop cancelled");
                self.fire_turn_end(TurnEndCtx::new(
                    session_id,
                    turn_count,
                    turn_start,
                    &model,
                    user_input_owned,
                    RunOutcome::Cancelled,
                ))
                .await;
                return Ok((RunOutcome::Cancelled, turn_count));
            }

            // Drain steering messages (P2) — injected mid-run by steer().
            // These are pushed as user messages and processed in this iteration.
            {
                let steering_msgs = self.message_queue.drain_steering();
                if !steering_msgs.is_empty() {
                    tracing::info!(
                        session_id = session_id.id,
                        count = steering_msgs.len(),
                        "drained steering messages"
                    );
                    for msg in steering_msgs {
                        self.with_session_mut(session_id, |session| {
                            session.push_message(MessageRole::User, &msg);
                        })
                        .await?;
                    }
                }
            }

            if turn_count > max_turns {
                tracing::warn!(
                    session_id = session_id.id,
                    turn_count,
                    max_turns,
                    "max turns exceeded"
                );
                self.fire_turn_end(TurnEndCtx {
                    error_message: Some("max turns exceeded"),
                    ..TurnEndCtx::new(
                        session_id,
                        turn_count,
                        turn_start,
                        &model,
                        user_input_owned,
                        RunOutcome::MaxTurnsExceeded { turns: turn_count },
                    )
                })
                .await;
                return Ok((
                    RunOutcome::MaxTurnsExceeded { turns: turn_count },
                    turn_count,
                ));
            }

            drain_locked(event_rx, &on_event)?;

            let turn_span =
                tracing::info_span!("turn", session_id = session_id.id, turn = turn_count);
            let _turn_guard = turn_span.enter();

            let session = self.session_manager.session_or_err(session_id).await?;
            let messages: Vec<_> = session.chat_messages().to_vec();

            // Apply message conversion before sending to LLM.
            // Default: strip Custom messages that providers don't understand.
            let mut messages = match &self.convert_to_llm {
                Some(convert) => convert(&messages),
                None => default_convert_to_llm(&messages),
            };

            let tools_for_turn = tool_definitions.to_vec();

            if let Some(ref ctx_mgr) = self.context_manager {
                let before = messages.len();
                ctx_mgr.trim(&mut messages);
                tracing::debug!(
                    session_id = session_id.id,
                    turn = turn_count,
                    before,
                    after = messages.len(),
                    "context trimmed"
                );
            }

            let (messages, tools_for_turn) = self
                .apply_pre_llm_mw(session_id, messages, tools_for_turn, on_event.clone())
                .await?;

            // Drain UserEvent copies from event_rx that were written by
            // ctx.emit() during middleware.  These were already delivered to
            // on_event directly; leaving them in the bus would cause
            // double-rendering in process_stream or orphaning on error paths
            // (e.g. LLM call fails before process_stream runs).
            loop {
                match event_rx.try_recv() {
                    Ok(RuntimeEvent::UserEvent { .. }) => continue,
                    Ok(event) => {
                        // Non-UserEvent — process it and keep draining.
                        if let Ok(mut cb) = on_event.lock() {
                            cb(event)?;
                        }
                        continue;
                    }
                    Err(_) => break,
                }
            }

            self.event_bus.emit(RuntimeEvent::Checkpoint {
                session_id: session_id.clone(),
                checkpoint: CheckpointData {
                    session_id: session_id.clone(),
                    user_input: user_input_owned.to_string(),
                    step: CheckpointStep::BeforeLlm {
                        messages: messages.clone(),
                        tools: tools_for_turn.clone(),
                    },
                    turn_count,
                },
                agent_id: None,
                trace_id: None,
            });

            tracing::info!(
                session_id = session_id.id,
                turn = turn_count,
                msg_count = messages.len(),
                tool_count = tools_for_turn.len(),
                "calling LLM"
            );
            let stream = match config.llm.llm_retry.as_ref() {
                Some(retry) => {
                    tracing::debug!(
                        session_id = session_id.id,
                        turn = turn_count,
                        "LLM: using retry mode"
                    );
                    self.llm_engine
                        .run_llm_turn_with_retry(
                            session_id,
                            &messages,
                            &tools_for_turn,
                            config.reasoning.as_ref(),
                            config.llm.response_format.as_ref(),
                            retry.clone(),
                        )
                        .await?
                }
                None => {
                    tracing::debug!(
                        session_id = session_id.id,
                        turn = turn_count,
                        "LLM: calling chat_stream"
                    );
                    self.llm_engine
                        .chat_stream(
                            &messages,
                            &tools_for_turn,
                            config.reasoning.as_ref(),
                            config.llm.response_format.as_ref(),
                        )
                        .await?
                }
            };
            tracing::info!(
                session_id = session_id.id,
                turn = turn_count,
                "LLM stream obtained, processing"
            );

            let span =
                tracing::info_span!("llm_turn", session_id = session_id.id, turn = turn_count);
            let cancel_token = self.cancel_token();
            let result = self
                .llm_engine
                .process_stream(
                    session_id,
                    stream,
                    span,
                    event_rx,
                    on_event.clone(),
                    &cancel_token,
                )
                .await;
            tracing::info!(
                session_id = session_id.id,
                turn = turn_count,
                is_err = result.is_err(),
                "LLM stream processed"
            );

            match self
                .handle_llm_turn(
                    session_id,
                    user_input_owned,
                    tool_definitions,
                    turn_count,
                    turn_start,
                    &model,
                    result,
                    event_rx,
                    on_event.clone(),
                    all_user_inputs,
                )
                .await?
            {
                TurnFlow::Continue => continue,
                TurnFlow::Done(outcome) => return Ok((outcome, turn_count)),
            }
        }
    }

    /// Dispatch one LLM turn result: push messages, run tool calls, fire the
    /// turn-end callback, and decide whether the loop continues or the turn ends.
    #[allow(clippy::too_many_arguments)]
    async fn handle_llm_turn<F>(
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
    ) -> AgentResult<TurnFlow>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        match result {
            Ok(LlmTurnResult {
                full_text,
                reasoning_text,
                is_tool_call,
                tool_calls,
                usage,
                finish_reason,
                ttft_ms,
                llm_duration_ms,
                reasoning_only,
            }) => {
                // Normalise provider-specific finish_reason into a semantic enum.
                let finish_reason = FinishReason::from_raw(finish_reason.as_deref());
                tracing::info!(
                    session_id = session_id.id,
                    turn = turn_count,
                    text_len = full_text.len(),
                    is_tool_call = is_tool_call,
                    tool_call_count = tool_calls.len(),
                    "LLM turn result"
                );
                // Capture text info before moves
                let text_len = full_text.len() as u64;
                let has_thinking = !reasoning_text.is_empty();
                let tool_calls_parsed: Vec<(String, String, String)> = tool_calls
                    .iter()
                    .map(|tc| {
                        let id = tc
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = tc
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        let args = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(|a| a.as_str())
                            .unwrap_or("")
                            .to_string();
                        (id, name, args)
                    })
                    .collect();

                let available_tools: Vec<String> = tool_definitions
                    .iter()
                    .filter_map(|d| {
                        d.get("function")?
                            .get("name")?
                            .as_str()
                            .map(|s| s.to_string())
                    })
                    .collect();

                // Shared context for guard dispatch
                let turn_ctx = TurnCtx {
                    session_id,
                    turn_count,
                    user_input: user_input_owned,
                    finish_reason: &finish_reason,
                    available_tools: &available_tools,
                    turn_start,
                    model,
                    all_user_inputs,
                };
                let metrics = TurnMetrics {
                    ttft_ms,
                    llm_duration_ms,
                    usage: &usage,
                    text_len,
                    has_thinking,
                };

                // Degenerate state: the model emitted reasoning_content but no
                // `content` and no tool call. We never promote reasoning into the
                // answer, so this is a no-output turn: the model "thought" but
                // neither committed to a tool call nor wrote an answer. Nudge it to
                // decide, and fail after a few consecutive strikes.
                if reasoning_only {
                    let strikes = self
                        .with_session_mut(session_id, |session| {
                            session.run_state.record_reasoning_only()
                        })
                        .await?;
                    tracing::warn!(
                        session_id = session_id.id,
                        turn = turn_count,
                        strikes,
                        "reasoning-only response with tools available — nudging the model to commit"
                    );

                    let guard_ctx = self.build_guard_ctx(&turn_ctx, "").await;
                    let action = self.guard.on_reasoning_only(&guard_ctx).await;
                    return self
                        .dispatch_nudge_guard(
                            action,
                            &turn_ctx,
                            &metrics,
                            "reasoning-only response",
                        )
                        .await;
                }

                let result = self
                    .apply_post_llm_mw(
                        session_id,
                        full_text,
                        is_tool_call,
                        tool_calls_parsed,
                        &available_tools,
                        turn_count,
                        finish_reason.clone(),
                    )
                    .await?;

                if !result.skip_push && !result.full_text.is_empty() {
                    // Preserve reasoning so the LLM can see its own prior thinking
                    // in subsequent turns, avoiding "amnesia" re-derivation.
                    let reasoning = reasoning_text.clone();
                    self.with_session_mut(session_id, |session| {
                        if !reasoning.is_empty() {
                            session.push_assistant_with_reasoning(&result.full_text, &reasoning);
                        } else {
                            session.push_message(MessageRole::Assistant, &result.full_text);
                        }
                    })
                    .await?;
                }

                if let Some(follow_up) = result.follow_up_message {
                    self.with_session_mut(session_id, |session| {
                        session.push_message(MessageRole::User, &follow_up);
                    })
                    .await?;
                    return Ok(TurnFlow::Continue);
                }

                if result.full_text.is_empty() && !result.is_tool_call {
                    // Degenerate state: the model returned nothing — no text, no
                    // reasoning, no tool call. This is an EMPTY_RESPONSE. Retry a
                    // bounded number of times (nudging the model to produce output),
                    // then fail loudly instead of looping forever.
                    let strikes = self
                        .with_session_mut(session_id, |session| {
                            session.run_state.record_empty_response()
                        })
                        .await?;
                    tracing::warn!(
                        session_id = session_id.id,
                        turn = turn_count,
                        strikes,
                        "empty LLM response (no text, no reasoning, no tool call)"
                    );

                    let guard_ctx = self.build_guard_ctx(&turn_ctx, "").await;
                    let action = self.guard.on_empty_response(&guard_ctx).await;
                    return self
                        .dispatch_nudge_guard(action, &turn_ctx, &metrics, "empty response")
                        .await;
                }

                if result.is_tool_call && !result.tool_calls.is_empty() {
                    return self
                        .run_tool_turn(
                            session_id,
                            user_input_owned,
                            turn_count,
                            turn_start,
                            model,
                            result.tool_calls,
                            &finish_reason,
                            reasoning_text,
                            &metrics,
                            event_rx,
                            on_event.clone(),
                        )
                        .await;
                }

                // ── incomplete tool call: model signalled tool_use but no
                // complete tool calls were parsed (stream cut off mid-delta,
                // JSON incomplete, etc.) and finish_reason is normal.  Nudge
                // the model to retry instead of silently falling through to
                // the text-only branch. ──
                if result.is_tool_call && result.tool_calls.is_empty() {
                    let strikes = self
                        .with_session_mut(session_id, |session| {
                            session.run_state.record_empty_response()
                        })
                        .await?;
                    tracing::warn!(
                        session_id = session_id.id,
                        turn = turn_count,
                        strikes,
                        "model signalled tool call but no complete tool calls were parsed"
                    );
                    let guard_ctx = self.build_guard_ctx(&turn_ctx, "").await;
                    let action = self
                        .guard
                        .on_empty_response(&guard_ctx)
                        .await;
                    return self
                        .dispatch_nudge_guard(
                            action,
                            &turn_ctx,
                            &metrics,
                            "incomplete tool call",
                        )
                        .await;
                }

                // ── finish_reason anomaly detection (no tool call branch above matched) ──
                match &finish_reason {
                    FinishReason::ToolUse => {
                        let error = "finish_reason=tool_use but no tool calls were parsed";
                        tracing::warn!(
                            session_id = session_id.id,
                            turn = turn_count,
                            error,
                            "model signalled tool_use but no tool calls were parsed"
                        );
                        self.fire_turn_end(TurnEndCtx {
                            ttft_ms,
                            llm_duration_ms,
                            usage: &usage,
                            text_length: text_len,
                            has_thinking,
                            llm_calls: 1,
                            error_message: Some(error),
                            ..TurnEndCtx::new(
                                session_id,
                                turn_count,
                                turn_start,
                                model,
                                user_input_owned,
                                RunOutcome::Failed {
                                    error: error.to_string(),
                                },
                            )
                        })
                        .await;
                        return Ok(TurnFlow::Done(RunOutcome::Failed {
                            error: error.to_string(),
                        }));
                    }
                    FinishReason::Truncated { reason } => {
                        // Pure-text response hit the token limit — this was previously
                        // silently swallowed as a normal completion.
                        let error = format!("response truncated ({:?})", reason);
                        tracing::warn!(
                            session_id = session_id.id,
                            turn = turn_count,
                            ?reason,
                            "text-only response truncated by token limit"
                        );
                        self.fire_turn_end(TurnEndCtx {
                            ttft_ms,
                            llm_duration_ms,
                            usage: &usage,
                            text_length: text_len,
                            has_thinking,
                            llm_calls: 1,
                            error_message: Some(&error),
                            ..TurnEndCtx::new(
                                session_id,
                                turn_count,
                                turn_start,
                                model,
                                user_input_owned,
                                RunOutcome::Failed {
                                    error: error.clone(),
                                },
                            )
                        })
                        .await;
                        return Ok(TurnFlow::Done(RunOutcome::Failed { error }));
                    }
                    _ => {}
                }

                // ── text-only branch ──
                // Use the guard to determine if the task is actually complete.
                let guard_ctx = self.build_guard_ctx(&turn_ctx, &result.full_text).await;
                let action = self.guard.on_text_only(&guard_ctx).await;
                self.dispatch_text_only_guard(action, &turn_ctx, &metrics)
                    .await
            }
            Err(e) => {
                // Fire turn-end callback for LLM stream error
                let stream_outcome = if e.is_cancelled() {
                    RunOutcome::Cancelled
                } else {
                    RunOutcome::Failed {
                        error: e.to_string(),
                    }
                };
                self.fire_turn_end(TurnEndCtx {
                    error_message: Some(&e.to_string()),
                    ..TurnEndCtx::new(
                        session_id,
                        turn_count,
                        turn_start,
                        model,
                        user_input_owned,
                        stream_outcome,
                    )
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
        }
    }
}
