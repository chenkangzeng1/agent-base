use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use crate::engine::middleware::{PostLlmCtx, PreLlmCtx};
use crate::engine::runtime::llm_engine::LlmTurnResult;
use crate::engine::runtime::plan_runner::RuntimeCore;
use crate::types::{
    AgentResult, CheckpointData, CheckpointStep, MessageRole, RunOutcome, RuntimeEvent, SessionId,
    UserEvent, default_convert_to_llm,
};

use super::entry::drain_locked;
use super::turn_end::TurnEndCtx;

struct PostLlmMwResult {
    pub full_text: String,
    pub is_tool_call: bool,
    pub tool_calls: Vec<(String, String, String)>,
    pub skip_push: bool,
    pub follow_up_message: Option<String>,
}

/// Control-flow result of one `handle_llm_turn` call: keep looping, or
/// terminate the turn with a final outcome.
enum TurnFlow {
    Continue,
    Done(RunOutcome),
}

/// Max consecutive reasoning-only turns (no text, no tool call) before the
/// react loop gives up and fails instead of looping on a reasoning-model runaway.
const REASONING_ONLY_MAX_STRIKES: usize = 3;

/// Max consecutive completely-empty responses (no text, no reasoning, no tool
/// call) before the react loop fails instead of looping forever. Mirrors the
/// bounded-retry budget of the reference harnesses (2 retries after the first).
const EMPTY_RESPONSE_MAX_STRIKES: usize = 3;

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
        // Create drain_fn with its own broadcast subscriber.
        // Middleware (e.g. compression) can call this during long-running
        // operations to flush accumulated events in real-time.
        let drain_fn: Arc<dyn Fn() + Send + Sync> = {
            let rx = Mutex::new(self.event_bus.subscribe());
            let cb = on_event.clone();
            Arc::new(move || {
                let mut rx_guard = rx.lock().unwrap_or_else(|e| e.into_inner());
                if let Err(e) = drain_locked(&mut rx_guard, &cb) {
                    tracing::warn!("drain_fn failed: {}", e);
                }
            })
        };

        let mut ctx = PreLlmCtx {
            session_id: session_id.clone(),
            messages,
            tools,
            user_event_fn: {
                let eb = self.event_bus.clone();
                let sid = session_id.clone();
                Some(std::sync::Arc::new(move |ev: UserEvent| {
                    eb.emit(RuntimeEvent::UserEvent {
                        session_id: sid.clone(),
                        event: ev,
                        agent_id: None,
                        trace_id: None,
                    });
                }))
            },
            drain_fn: Some(drain_fn.clone()),
        };

        for mw in &self.middlewares {
            mw.on_pre_llm(&mut ctx).await?;
        }

        // Drain events emitted by middleware (e.g. compression Started)
        // so they render immediately rather than after the LLM call.
        drain_fn();
        Ok((ctx.messages, ctx.tools))
    }

    async fn apply_post_llm_mw(
        &self,
        session_id: &SessionId,
        full_text: String,
        is_tool_call: bool,
        tool_calls: Vec<(String, String, String)>,
        available_tools: &[String],
        turn_count: u32,
    ) -> AgentResult<PostLlmMwResult> {
        let session = self.session_manager.session_or_err(session_id).await?;
        let total_tool_calls = session.total_tool_calls;
        let nudge_count = session.nudge_count;
        let turn_tool_calls = session.turn_tool_calls;
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
        };
        for mw in &self.middlewares {
            mw.on_post_llm(&mut ctx).await?;
        }
        // Write back nudge_count if middleware modified it
        if ctx.nudge_count != nudge_count {
            self.with_session_mut(session_id, |session| {
                session.nudge_count = ctx.nudge_count;
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

    pub(super) async fn run_turn_loop<F>(
        &self,
        session_id: &SessionId,
        user_input_owned: &str,
        tool_definitions: &[serde_json::Value],
        mut turn_count: u32,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: Arc<Mutex<F>>,
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
            let model = self.llm_engine.get_client().model_name().to_string();

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

                // Degenerate state: the model emitted reasoning_content but no
                // `content` and no tool call. We never promote reasoning into the
                // answer, so this is a no-output turn: the model "thought" but
                // neither committed to a tool call nor wrote an answer. Nudge it to
                // decide, and fail after a few consecutive strikes.
                if reasoning_only {
                    let strikes = self
                        .with_session_mut(session_id, |session| {
                            session.reasoning_only_strikes += 1;
                            session.reasoning_only_strikes
                        })
                        .await?;
                    tracing::warn!(
                        session_id = session_id.id,
                        turn = turn_count,
                        strikes,
                        "reasoning-only response with tools available — nudging the model to commit"
                    );
                    if strikes >= REASONING_ONLY_MAX_STRIKES {
                        let error = "model produced only reasoning (no tool call or answer) \
                                     across multiple turns despite tools being available";
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
                    self.with_session_mut(session_id, |session| {
                        session.push_message(
                            MessageRole::User,
                            "You produced internal reasoning but no tool call and no final answer. \
                             Make a decision now: call a tool to make progress, or write your \
                             final answer as plain text.",
                        );
                    })
                    .await?;
                    return Ok(TurnFlow::Continue);
                }

                let result = self
                    .apply_post_llm_mw(
                        session_id,
                        full_text,
                        is_tool_call,
                        tool_calls_parsed,
                        &available_tools,
                        turn_count,
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
                            session.empty_response_strikes += 1;
                            session.empty_response_strikes
                        })
                        .await?;
                    tracing::warn!(
                        session_id = session_id.id,
                        turn = turn_count,
                        strikes,
                        "empty LLM response (no text, no reasoning, no tool call)"
                    );
                    if strikes >= EMPTY_RESPONSE_MAX_STRIKES {
                        let error = "model returned empty responses repeatedly \
                                     (no text, no reasoning, no tool call)";
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
                    self.with_session_mut(session_id, |session| {
                        session.push_message(
                            MessageRole::User,
                            "You returned an empty response with no tool call and no \
                             answer. Produce output now: call a tool to make progress, \
                             or write your final answer as plain text.",
                        );
                    })
                    .await?;
                    return Ok(TurnFlow::Continue);
                }

                if result.is_tool_call && !result.tool_calls.is_empty() {
                    // P5: Truncation guard — when the LLM response hit the token limit,
                    // tool call arguments may be incomplete. Fail all tool calls
                    // without executing them, so the LLM can retry with complete args.
                    if finish_reason.as_deref() == Some("length") {
                        tracing::warn!(
                            session_id = session_id.id,
                            turn = turn_count,
                            tool_count = tool_calls.len(),
                            "LLM response truncated (finish_reason=length) — tool calls may have incomplete arguments, marking as errors"
                        );
                        for tc in &tool_calls {
                            let tc_id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            let tc_name = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|n| n.as_str())
                                .unwrap_or("unknown");
                            self.with_session_mut(session_id, |session| {
                                session.push_message(
                                    MessageRole::Assistant,
                                    format!("[would call tool: {}]", tc_name),
                                );
                                session.push_tool_result(
                                    tc_id,
                                    "Tool call was not executed: the response hit the output token limit, \
                                     so its arguments may be truncated. Re-issue the tool call with complete arguments.",
                                );
                            })
                            .await?;
                        }
                        // Skip tool execution entirely — the error results above will
                        // cause the LLM to regenerate the tool calls in the next turn.
                        return Ok(TurnFlow::Continue);
                    }

                    tracing::info!(
                        session_id = session_id.id,
                        turn = turn_count,
                        tool_count = result.tool_calls.len(),
                        "handling tool calls"
                    );
                    self.event_bus.emit(RuntimeEvent::Checkpoint {
                        session_id: session_id.clone(),
                        checkpoint: CheckpointData {
                            session_id: session_id.clone(),
                            user_input: user_input_owned.to_string(),
                            step: CheckpointStep::BeforeToolCalls {
                                tool_calls: result.tool_calls.clone(),
                            },
                            turn_count,
                        },
                        agent_id: None,
                        trace_id: None,
                    });

                    let tool_start = std::time::Instant::now();
                    let tool_call_count = result.tool_calls.len() as u32;
                    let tool_names: Vec<String> = result
                        .tool_calls
                        .iter()
                        .map(|(_, name, _)| name.clone())
                        .collect();

                    match self
                        .handle_tool_calls(
                            session_id,
                            &result.tool_calls,
                            event_rx,
                            on_event.clone(),
                            reasoning_text,
                        )
                        .await
                    {
                        Ok(()) => {
                            let tool_duration_ms = tool_start.elapsed().as_millis() as u64;
                            self.fire_turn_end(TurnEndCtx {
                                ttft_ms,
                                llm_duration_ms,
                                tool_duration_ms,
                                usage: &usage,
                                text_length: text_len,
                                has_thinking,
                                tool_call_count,
                                tools_used: &tool_names,
                                tool_success: tool_call_count,
                                llm_calls: 1,
                                ..TurnEndCtx::new(
                                    session_id,
                                    turn_count,
                                    turn_start,
                                    model,
                                    user_input_owned,
                                    RunOutcome::Completed,
                                )
                            })
                            .await;
                            tracing::info!(
                                session_id = session_id.id,
                                turn = turn_count,
                                "tool calls done, continuing loop"
                            );
                            let n = result.tool_calls.len();
                            self.with_session_mut(session_id, |session| {
                                session.total_tool_calls += n;
                                session.turn_tool_calls += n;
                            })
                            .await?;
                            self.event_bus.emit(RuntimeEvent::Checkpoint {
                                session_id: session_id.clone(),
                                checkpoint: CheckpointData {
                                    session_id: session_id.clone(),
                                    user_input: user_input_owned.to_string(),
                                    step: CheckpointStep::AfterToolCalls {
                                        tool_calls: result.tool_calls.clone(),
                                        results: Vec::new(),
                                    },
                                    turn_count,
                                },
                                agent_id: None,
                                trace_id: None,
                            });
                            return Ok(TurnFlow::Continue);
                        }
                        Err(e) => {
                            let tool_duration_ms = tool_start.elapsed().as_millis() as u64;
                            let error_msg = e.to_string();
                            if let Some(outcome) = self
                                .handle_tool_error(
                                    session_id,
                                    &result.tool_calls,
                                    e,
                                    event_rx,
                                    on_event.clone(),
                                )
                                .await?
                            {
                                self.fire_turn_end(TurnEndCtx {
                                    ttft_ms,
                                    llm_duration_ms,
                                    tool_duration_ms,
                                    usage: &usage,
                                    text_length: text_len,
                                    has_thinking,
                                    tool_call_count,
                                    tools_used: &tool_names,
                                    tool_failed: tool_call_count,
                                    error_message: Some(&error_msg),
                                    llm_calls: 1,
                                    ..TurnEndCtx::new(
                                        session_id,
                                        turn_count,
                                        turn_start,
                                        model,
                                        user_input_owned,
                                        RunOutcome::Failed {
                                            error: error_msg.clone(),
                                        },
                                    )
                                })
                                .await;
                                return Ok(TurnFlow::Done(outcome));
                            }
                            // Retry: record metrics for the failed attempt
                            self.fire_turn_end(TurnEndCtx {
                                ttft_ms,
                                llm_duration_ms,
                                tool_duration_ms,
                                usage: &usage,
                                text_length: text_len,
                                has_thinking,
                                tool_call_count,
                                tools_used: &tool_names,
                                tool_failed: tool_call_count,
                                error_message: Some(&error_msg),
                                llm_calls: 1,
                                ..TurnEndCtx::new(
                                    session_id,
                                    turn_count,
                                    turn_start,
                                    model,
                                    user_input_owned,
                                    RunOutcome::Failed {
                                        error: error_msg.clone(),
                                    },
                                )
                            })
                            .await;
                            return Ok(TurnFlow::Continue);
                        }
                    }
                }

                tracing::info!(
                    session_id = session_id.id,
                    turn = turn_count,
                    "text-only response, run completed"
                );
                self.fire_turn_end(TurnEndCtx {
                    ttft_ms,
                    llm_duration_ms,
                    usage: &usage,
                    text_length: text_len,
                    has_thinking,
                    llm_calls: 1,
                    ..TurnEndCtx::new(
                        session_id,
                        turn_count,
                        turn_start,
                        model,
                        user_input_owned,
                        RunOutcome::Completed,
                    )
                })
                .await;
                Ok(TurnFlow::Done(RunOutcome::Completed))
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
