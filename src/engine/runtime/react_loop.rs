use tokio::sync::broadcast;

use crate::engine::middleware::{PostLlmCtx, PreLlmCtx, UserMessageCtx};
use crate::engine::recovery::ToolErrorAction;
use crate::engine::runtime::event_bus::EventBus;
use crate::engine::runtime::llm_engine::LlmTurnResult;
use crate::types::{
    AgentError, AgentResult, CheckpointData, CheckpointStep, MessageRole, RunOutcome, RuntimeEvent,
    SessionId,
};

use super::plan_runner::RuntimeCore;

pub(super) enum ToolCallResult {
    Continue,
    Break,
}

pub(super) struct PostLlmMwResult {
    pub full_text: String,
    pub is_tool_call: bool,
    pub tool_calls: Vec<(String, String, String)>,
    pub skip_push: bool,
    pub follow_up_message: Option<String>,
}

impl RuntimeCore {
    pub async fn run<F>(&self, session_id: SessionId, mut on_event: F) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        // Reset cancel token for this run
        self.reset_cancel();

        let span = tracing::info_span!("agent_run", session_id = session_id.id);
        let _enter = span.enter();

        let mut event_rx = self.event_bus.subscribe();

        if let Err(e) = self.validate_session(&session_id).await {
            tracing::warn!(session_id = session_id.id, error = %e, "session validation failed");
            self.event_bus.emit(RuntimeEvent::RunFinished {
                session_id: session_id.clone(),
                agent_id: None,
                trace_id: None,
            });
            EventBus::drain_async_events(&mut event_rx, &mut on_event)?;
            return Err(e);
        }

        let tool_definitions = self.tool_engine.definitions().await;
        tracing::debug!(
            session_id = session_id.id,
            tool_count = tool_definitions.len(),
            "agent run start"
        );
        let user_input_owned = self
            .with_session_mut(&session_id, |session| {
                session
                    .chat_messages()
                    .last()
                    .and_then(|m| match m {
                        crate::types::ChatMessage::User { content, .. } => Some(content.clone()),
                        _ => None,
                    })
                    .unwrap_or_default()
            })
            .await?;

        // Apply user message middleware (same as run_turn)
        let user_input_owned = self
            .apply_user_message_mw(&session_id, user_input_owned)
            .await?;

        // Reset nudge_count and turn_tool_calls for the new turn
        self.with_session_mut(&session_id, |session| {
            session.nudge_count = 0;
            session.turn_tool_calls = 0;
        })
        .await?;

        let result = self
            .run_turn_loop(
                &session_id,
                &user_input_owned,
                &tool_definitions,
                0,
                &mut event_rx,
                &mut on_event,
            )
            .await;

        // Emit RunCancelled if cancelled, RunFinished otherwise (same as run_turn)
        match &result {
            Ok((RunOutcome::Cancelled, _)) => {
                on_event(RuntimeEvent::RunCancelled {
                    session_id: session_id.clone(),
                    agent_id: None,
                    trace_id: None,
                })?;
            }
            Err(e) if e.is_cancelled() => {
                on_event(RuntimeEvent::RunCancelled {
                    session_id: session_id.clone(),
                    agent_id: None,
                    trace_id: None,
                })?;
            }
            _ => {
                self.event_bus.emit(RuntimeEvent::RunFinished {
                    session_id: session_id.clone(),
                    agent_id: None,
                    trace_id: None,
                });
                EventBus::drain_async_events(&mut event_rx, &mut on_event)?;
            }
        }

        // Turn 结束：清理临时消息（包括错误路径，避免 ephemeral 残留到持久化）
        if let Err(e) = self
            .with_session_mut(&session_id, |session| {
                session.remove_ephemeral_messages();
            })
            .await
        {
            tracing::warn!(error = %e, "failed to clean up ephemeral messages");
        }

        let (outcome, _turn_count) = result?;
        Ok(outcome)
    }

    pub async fn run_turn<F>(
        &self,
        session_id: SessionId,
        user_input: &str,
        mut on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        // Helper: emit RunFinished and return the error, so event
        // listeners (e.g. serve.rs) don't hang on session/middleware
        // failures that happen before the react loop starts.
        let fail = |e: AgentError, f: &mut F| -> AgentResult<RunOutcome> {
            let _ = f(RuntimeEvent::RunFinished {
                session_id: session_id.clone(),
                agent_id: None,
                trace_id: None,
            });
            Err(e)
        };

        // Reset cancel token for this turn
        self.reset_cancel();

        let span = tracing::Span::current();
        let _guard = span.enter();
        tracing::info!(session_id = session_id.id, user_input = %user_input, "agent turn start");
        drop(_guard);

        tracing::debug!(
            session_id = session_id.id,
            "run_turn: subscribing to event bus"
        );
        let mut event_rx = self.event_bus.subscribe();
        let tool_definitions = self.tool_engine.definitions().await;

        let user_input_owned = match self
            .apply_user_message_mw(&session_id, user_input.to_string())
            .await
        {
            Ok(u) => u,
            Err(e) => return fail(e, &mut on_event),
        };

        // Reset nudge_count and turn_tool_calls for the new turn
        if let Err(e) = self
            .with_session_mut(&session_id, |session| {
                session.nudge_count = 0;
                session.turn_tool_calls = 0;
            })
            .await
        {
            return fail(e, &mut on_event);
        }

        if let Err(e) = self
            .with_session_mut(&session_id, |session| {
                session.push_message(MessageRole::User, &user_input_owned);
            })
            .await
        {
            return fail(e, &mut on_event);
        }

        self.event_bus.emit(RuntimeEvent::Checkpoint {
            session_id: session_id.clone(),
            checkpoint: CheckpointData {
                session_id: session_id.clone(),
                user_input: user_input_owned.clone(),
                step: CheckpointStep::AfterUserInput,
                turn_count: 0,
            },
            agent_id: None,
            trace_id: None,
        });

        tracing::info!(
            session_id = session_id.id,
            "run_turn: entering run_turn_loop"
        );
        let result = self
            .run_turn_loop(
                &session_id,
                &user_input_owned,
                &tool_definitions,
                0,
                &mut event_rx,
                &mut on_event,
            )
            .await;

        // Emit RunCancelled event if cancelled
        match &result {
            Ok((RunOutcome::Cancelled, _)) => {
                on_event(RuntimeEvent::RunCancelled {
                    session_id: session_id.clone(),
                    agent_id: None,
                    trace_id: None,
                })?;
            }
            Err(e) if e.is_cancelled() => {
                on_event(RuntimeEvent::RunCancelled {
                    session_id: session_id.clone(),
                    agent_id: None,
                    trace_id: None,
                })?;
            }
            _ => {}
        }

        // Clean up ephemeral messages
        if let Err(e) = self
            .with_session_mut(&session_id, |session| {
                session.remove_ephemeral_messages();
            })
            .await
        {
            tracing::warn!(error = %e, "failed to clean up ephemeral messages");
        }

        // Emit RunFinished on any non-cancelled error so event listeners
        // know the turn ended.
        let (outcome, turn_count) = match result {
            Ok(tuple) => tuple,
            Err(e) if e.is_cancelled() => {
                return Err(e);
            }
            Err(e) => {
                let _ = on_event(RuntimeEvent::RunFinished {
                    session_id: session_id.clone(),
                    agent_id: None,
                    trace_id: None,
                });
                return Err(e);
            }
        };

        tracing::info!(
            session_id = session_id.id,
            turn_count,
            "agent turn completed"
        );
        Ok(outcome)
    }

    pub async fn run_turn_collect(
        &self,
        session_id: SessionId,
        user_input: &str,
    ) -> AgentResult<(Vec<RuntimeEvent>, RunOutcome)> {
        let mut events = Vec::new();
        let outcome = self
            .run_turn(session_id, user_input, |event| {
                events.push(event);
                Ok(())
            })
            .await?;
        Ok((events, outcome))
    }

    #[allow(dead_code)]
    pub async fn resume_from_checkpoint<F>(
        &self,
        checkpoint: CheckpointData,
        mut on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        // Reset cancel token for this resume
        self.reset_cancel();

        let session_id = checkpoint.session_id.clone();
        let user_input = checkpoint.user_input.clone();
        let turn_count = checkpoint.turn_count;

        tracing::info!(session_id = session_id.id, turn_count, step = ?checkpoint.step, "resuming from checkpoint");

        let mut event_rx = self.event_bus.subscribe();
        let tool_definitions = self.tool_engine.definitions().await;

        if let CheckpointStep::BeforeToolCalls { tool_calls } = checkpoint.step {
            match self
                .handle_tool_calls(
                    &session_id,
                    &tool_calls,
                    &mut event_rx,
                    &mut on_event,
                    String::new(),
                )
                .await
            {
                Ok(ToolCallResult::Continue) => {}
                Ok(ToolCallResult::Break) => {
                    self.event_bus.emit(RuntimeEvent::RunFinished {
                        session_id: session_id.clone(),
                        agent_id: None,
                        trace_id: None,
                    });
                    EventBus::drain_async_events(&mut event_rx, &mut on_event)?;
                    return Ok(RunOutcome::Completed);
                }
                Err(e) => {
                    if let Some(outcome) = self
                        .handle_tool_error(
                            &session_id,
                            &tool_calls,
                            e,
                            &mut event_rx,
                            &mut on_event,
                        )
                        .await?
                    {
                        return Ok(outcome);
                    }
                }
            }
        }

        let result = self
            .run_turn_loop(
                &session_id,
                &user_input,
                &tool_definitions,
                turn_count,
                &mut event_rx,
                &mut on_event,
            )
            .await;

        // Emit RunCancelled if cancelled, same as run_turn
        match &result {
            Ok((RunOutcome::Cancelled, _)) => {
                on_event(RuntimeEvent::RunCancelled {
                    session_id: session_id.clone(),
                    agent_id: None,
                    trace_id: None,
                })?;
            }
            Err(e) if e.is_cancelled() => {
                on_event(RuntimeEvent::RunCancelled {
                    session_id: session_id.clone(),
                    agent_id: None,
                    trace_id: None,
                })?;
            }
            _ => {}
        }

        // Turn 结束：清理临时消息（包括错误路径）
        if let Err(e) = self
            .with_session_mut(&session_id, |session| {
                session.remove_ephemeral_messages();
            })
            .await
        {
            tracing::warn!(error = %e, "failed to clean up ephemeral messages");
        }

        let (outcome, _final_turn_count) = result?;
        Ok(outcome)
    }

    pub(super) async fn apply_user_message_mw(
        &self,
        session_id: &SessionId,
        user_input: String,
    ) -> AgentResult<String> {
        let mut ctx = UserMessageCtx {
            session_id: session_id.clone(),
            user_input,
        };
        for mw in &self.middlewares {
            mw.on_user_message(&mut ctx).await?;
        }
        Ok(ctx.user_input)
    }

    pub(super) async fn apply_pre_llm_mw(
        &self,
        session_id: &SessionId,
        messages: Vec<crate::types::ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> AgentResult<(Vec<crate::types::ChatMessage>, Vec<serde_json::Value>)> {
        let mut ctx = PreLlmCtx {
            session_id: session_id.clone(),
            messages,
            tools,
        };
        for mw in &self.middlewares {
            mw.on_pre_llm(&mut ctx).await?;
        }
        Ok((ctx.messages, ctx.tools))
    }

    pub(super) async fn apply_post_llm_mw(
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

    pub(super) async fn handle_tool_error<F>(
        &self,
        session_id: &SessionId,
        tool_calls: &[(String, String, String)],
        e: AgentError,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: &mut F,
    ) -> AgentResult<Option<RunOutcome>>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let config = self.config_snapshot_async().await;

        if e.is_cancelled() {
            // Don't emit RunFinished when cancelled — RunCancelled will be emitted by run_turn
            // But still drain pending events and persist session
            EventBus::drain_async_events(event_rx, on_event)?;
            let session = self.session_manager.session_or_err(session_id).await?;
            if let Err(e) = self.session_manager.session_store().save(&session).await {
                tracing::warn!(session_id = session_id.id, error = %e, "Failed to persist session");
                if config.execution.fail_on_persist_error {
                    return Err(AgentError::internal(format!(
                        "Session persistence failed: {e}"
                    )));
                }
            }
            return Err(e);
        }

        let names: Vec<String> = tool_calls.iter().map(|(_, n, _)| n.clone()).collect();
        let error_text = e.to_string();
        let retry_prompt_template: Option<String> = config.tool.tool_error_retry_prompt.clone();

        // 用户主动拒绝（输入 n）→ 立即停止，不走 recovery 重试逻辑
        if matches!(e, AgentError::ApprovalDenied { .. }) {
            tracing::info!(
                session_id = session_id.id,
                "user rejected tool call, stopping immediately"
            );
            let error_summary = if config.language == crate::types::Language::Zh {
                format!("❌ 用户拒绝执行: {}", e)
            } else {
                format!("❌ User rejected: {}", e)
            };
            self.with_session_mut(session_id, |session| {
                session.close_dangling_tool_calls(&error_summary);
                session.remove_ephemeral_messages();
            })
            .await?;
            self.event_bus.emit(RuntimeEvent::RunFinished {
                session_id: session_id.clone(),
                agent_id: None,
                trace_id: None,
            });
            EventBus::drain_async_events(event_rx, on_event)?;
            let session = self.session_manager.session_or_err(session_id).await?;
            if let Err(e) = self.session_manager.session_store().save(&session).await {
                tracing::warn!(session_id = session_id.id, error = %e, "Failed to persist session");
            }
            return Ok(Some(RunOutcome::Completed));
        }

        let action = self
            .tool_engine
            .error_recovery()
            .on_error(session_id, &names, &e)?;
        match action {
            ToolErrorAction::Stop => {
                // Close any dangling tool calls in the session before stopping
                let error_summary = if config.language == crate::types::Language::Zh {
                    format!("❌ 执行失败: {}", e)
                } else {
                    format!("❌ Tool execution failed: {}", e)
                };
                self.with_session_mut(session_id, |session| {
                    session.close_dangling_tool_calls(&error_summary);
                    session.remove_ephemeral_messages();
                })
                .await?;
                self.event_bus.emit(RuntimeEvent::RunFinished {
                    session_id: session_id.clone(),
                    agent_id: None,
                    trace_id: None,
                });
                EventBus::drain_async_events(event_rx, on_event)?;
                let session = self.session_manager.session_or_err(session_id).await?;
                if let Err(e) = self.session_manager.session_store().save(&session).await {
                    tracing::warn!(session_id = session_id.id, error = %e, "Failed to persist session");
                    if config.execution.fail_on_persist_error {
                        return Err(AgentError::internal(format!(
                            "Session persistence failed: {e}"
                        )));
                    }
                }
                Ok(Some(RunOutcome::Failed {
                    error: format!("Tool execution failed: {}", e),
                }))
            }
            ToolErrorAction::Retry => {
                let retry_prompt = match &retry_prompt_template {
                    Some(template) => template
                        .replace("{tool_names}", &names.join(", "))
                        .replace("{error}", &error_text),
                    None => format!(
                        "Tool calls failed: {}\nError: {}\nPlease analyze the error and adjust your approach.",
                        names.join(", "),
                        error_text,
                    ),
                };

                let error_summary = if config.language == crate::types::Language::Zh {
                    format!("❌ 执行失败: {}", error_text)
                } else {
                    format!("❌ Tool execution failed: {}", error_text)
                };
                self.with_session_mut(session_id, |session| {
                    session.close_dangling_tool_calls(&error_summary);
                    session.push_message(MessageRole::User, retry_prompt);
                })
                .await?;
                Ok(None)
            }
        }
    }

    pub(super) async fn run_turn_loop<F>(
        &self,
        session_id: &SessionId,
        user_input_owned: &str,
        tool_definitions: &[serde_json::Value],
        mut turn_count: u32,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: &mut F,
    ) -> AgentResult<(RunOutcome, u32)>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
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
                self.fire_turn_end(
                    session_id,
                    turn_count,
                    turn_start,
                    &model,
                    user_input_owned,
                    0,
                    0,
                    0,
                    &None,
                    0,
                    false,
                    0,
                    &[],
                    0,
                    0,
                    RunOutcome::Cancelled,
                    None,
                    0,
                )
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
                self.fire_turn_end(
                    session_id,
                    turn_count,
                    turn_start,
                    &model,
                    user_input_owned,
                    0,
                    0,
                    0,
                    &None,
                    0,
                    false,
                    0,
                    &[],
                    0,
                    0,
                    RunOutcome::MaxTurnsExceeded { turns: turn_count },
                    Some("max turns exceeded"),
                    0,
                )
                .await;
                self.event_bus.emit(RuntimeEvent::RunFinished {
                    session_id: session_id.clone(),
                    agent_id: None,
                    trace_id: None,
                });
                EventBus::drain_async_events(event_rx, on_event)?;
                return Ok((
                    RunOutcome::MaxTurnsExceeded { turns: turn_count },
                    turn_count,
                ));
            }

            EventBus::drain_async_events(event_rx, on_event)?;

            let turn_span =
                tracing::info_span!("turn", session_id = session_id.id, turn = turn_count);
            let _turn_guard = turn_span.enter();

            let session = self.session_manager.session_or_err(session_id).await?;
            let mut messages: Vec<_> = session.chat_messages().to_vec();
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
                .apply_pre_llm_mw(session_id, messages, tools_for_turn)
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
                .process_stream(session_id, stream, span, event_rx, on_event, &cancel_token)
                .await;
            tracing::info!(
                session_id = session_id.id,
                turn = turn_count,
                is_err = result.is_err(),
                "LLM stream processed"
            );

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
                                session
                                    .push_assistant_with_reasoning(&result.full_text, &reasoning);
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
                        continue;
                    }

                    if result.full_text.is_empty() && !result.is_tool_call {
                        tracing::debug!(
                            session_id = session_id.id,
                            turn = turn_count,
                            "empty LLM response, continuing"
                        );
                        continue;
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
                            continue;
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
                                on_event,
                                reasoning_text,
                            )
                            .await
                        {
                            Ok(ToolCallResult::Continue) => {
                                let tool_duration_ms = tool_start.elapsed().as_millis() as u64;
                                self.fire_turn_end(
                                    session_id,
                                    turn_count,
                                    turn_start,
                                    &model,
                                    user_input_owned,
                                    ttft_ms,
                                    llm_duration_ms,
                                    tool_duration_ms,
                                    &usage,
                                    text_len,
                                    has_thinking,
                                    tool_call_count,
                                    &tool_names,
                                    tool_call_count, // all success
                                    0,               // no failures
                                    RunOutcome::Completed,
                                    None,
                                    1,
                                )
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
                                continue;
                            }
                            Ok(ToolCallResult::Break) => {
                                let tool_duration_ms = tool_start.elapsed().as_millis() as u64;
                                self.fire_turn_end(
                                    session_id,
                                    turn_count,
                                    turn_start,
                                    &model,
                                    user_input_owned,
                                    ttft_ms,
                                    llm_duration_ms,
                                    tool_duration_ms,
                                    &usage,
                                    text_len,
                                    has_thinking,
                                    tool_call_count,
                                    &tool_names,
                                    tool_call_count,
                                    0,
                                    RunOutcome::Completed,
                                    None,
                                    1,
                                )
                                .await;
                                tracing::info!(
                                    session_id = session_id.id,
                                    turn = turn_count,
                                    "tool calls requested break"
                                );
                                let n = result.tool_calls.len();
                                self.with_session_mut(session_id, |session| {
                                    session.total_tool_calls += n;
                                    session.turn_tool_calls += n;
                                })
                                .await?;
                                self.event_bus.emit(RuntimeEvent::RunFinished {
                                    session_id: session_id.clone(),
                                    agent_id: None,
                                    trace_id: None,
                                });
                                EventBus::drain_async_events(event_rx, on_event)?;
                                return Ok((RunOutcome::Completed, turn_count));
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
                                        on_event,
                                    )
                                    .await?
                                {
                                    self.fire_turn_end(
                                        session_id,
                                        turn_count,
                                        turn_start,
                                        &model,
                                        user_input_owned,
                                        ttft_ms,
                                        llm_duration_ms,
                                        tool_duration_ms,
                                        &usage,
                                        text_len,
                                        has_thinking,
                                        tool_call_count,
                                        &tool_names,
                                        0,
                                        tool_call_count,
                                        RunOutcome::Failed {
                                            error: error_msg.clone(),
                                        },
                                        Some(&error_msg),
                                        1,
                                    )
                                    .await;
                                    return Ok((outcome, turn_count));
                                }
                                // Retry: record metrics for the failed attempt
                                self.fire_turn_end(
                                    session_id,
                                    turn_count,
                                    turn_start,
                                    &model,
                                    user_input_owned,
                                    ttft_ms,
                                    llm_duration_ms,
                                    tool_duration_ms,
                                    &usage,
                                    text_len,
                                    has_thinking,
                                    tool_call_count,
                                    &tool_names,
                                    0,
                                    tool_call_count,
                                    RunOutcome::Failed {
                                        error: error_msg.clone(),
                                    },
                                    Some(&error_msg),
                                    1,
                                )
                                .await;
                                continue;
                            }
                        }
                    }

                    tracing::info!(
                        session_id = session_id.id,
                        turn = turn_count,
                        "text-only response, run completed"
                    );
                    self.fire_turn_end(
                        session_id,
                        turn_count,
                        turn_start,
                        &model,
                        user_input_owned,
                        ttft_ms,
                        llm_duration_ms,
                        0, // no tools
                        &usage,
                        text_len,
                        has_thinking,
                        0,
                        &[],
                        0,
                        0,
                        RunOutcome::Completed,
                        None,
                        1,
                    )
                    .await;
                    self.event_bus.emit(RuntimeEvent::RunFinished {
                        session_id: session_id.clone(),
                        agent_id: None,
                        trace_id: None,
                    });
                    EventBus::drain_async_events(event_rx, on_event)?;
                    return Ok((RunOutcome::Completed, turn_count));
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
                    self.fire_turn_end(
                        session_id,
                        turn_count,
                        turn_start,
                        &model,
                        user_input_owned,
                        0, // no TTFT if stream errored
                        0, // no LLM duration
                        0,
                        &None,
                        0,
                        false,
                        0,
                        &[],
                        0,
                        0,
                        stream_outcome,
                        Some(&e.to_string()),
                        0,
                    )
                    .await;
                    // Persist session on cancellation (LLM-stream path bypasses handle_tool_error)
                    if e.is_cancelled()
                        && let Ok(session) = self.session_manager.session_or_err(session_id).await
                    {
                        let _ = self.session_manager.session_store().save(&session).await;
                    }
                    return Err(e);
                }
            }
        }
    }

    pub(super) async fn handle_tool_calls<F>(
        &self,
        session_id: &SessionId,
        tool_calls: &[(String, String, String)],
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: &mut F,
        reasoning: String,
    ) -> AgentResult<ToolCallResult>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let tool_names: Vec<&str> = tool_calls
            .iter()
            .map(|(_, name, _)| name.as_str())
            .collect();
        tracing::debug!(
            session_id = session_id.id,
            ?tool_names,
            "handle tool calls start"
        );

        let config = self.config_snapshot_async().await;

        let ctx = super::tool_engine::ExecutionContext {
            session_manager: self.session_manager.clone(),
            llm_client: Some(self.llm_engine.get_client()),
            language: config.language.clone(),
            tool_timeout_ms: config.tool.tool_timeout_ms,
            max_output_chars: config.tool.max_tool_output_chars,
            cancel_token: self.cancel_token(),
        };

        // Orchestrate: approval check + execution for all tool calls.
        // Approval denial is handled inside process_approval (pushes fake tool state
        // and returns Err). We only push real tool calls on success.
        let results = self
            .tool_engine
            .orchestrate(session_id, tool_calls, &ctx, event_rx, on_event)
            .await?;

        // Push assistant tool calls to session after all approvals pass
        {
            let tc: Vec<(String, String, String)> = tool_calls.to_vec();
            self.with_session_mut(session_id, |session| {
                let r = if reasoning.is_empty() {
                    None
                } else {
                    Some(reasoning.clone())
                };
                session.push_assistant_tool_calls(&tc, r);
            })
            .await?;
        }

        // Process results: push to session, check for Break
        for result in results {
            self.with_session_mut(session_id, |session| {
                session.push_tool_result(&result.id, result.output.summary.clone());
            })
            .await?;

            if matches!(
                result.output.control_flow,
                crate::tool::ToolControlFlow::Break
            ) {
                return Ok(ToolCallResult::Break);
            }
        }

        Ok(ToolCallResult::Continue)
    }

    pub async fn validate_session(&self, session_id: &SessionId) -> AgentResult<()> {
        if self.session_manager.session(session_id).await.is_none() {
            return Err(AgentError::session_not_found(session_id.id));
        }
        Ok(())
    }

    /// Managed run with follow-up queue support (P2).
    ///
    /// After the inner turn loop completes naturally (text response, no tool calls),
    /// follow-up messages are drained and a new inner loop is started. This repeats
    /// until no more follow-up messages are queued.
    pub(super) async fn run_managed<F>(
        &self,
        session_id: SessionId,
        user_input: &str,
        mut on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        // Reset cancel token for this managed run
        self.reset_cancel();

        let span = tracing::info_span!("agent_managed_run", session_id = session_id.id);
        let _enter = span.enter();

        let mut event_rx = self.event_bus.subscribe();

        if let Err(e) = self.validate_session(&session_id).await {
            tracing::warn!(session_id = session_id.id, error = %e, "session validation failed");
            self.event_bus.emit(RuntimeEvent::RunFinished {
                session_id: session_id.clone(),
                agent_id: None,
                trace_id: None,
            });
            EventBus::drain_async_events(&mut event_rx, &mut on_event)?;
            return Err(e);
        }

        let tool_definitions = self.tool_engine.definitions().await;
        tracing::debug!(
            session_id = session_id.id,
            tool_count = tool_definitions.len(),
            "managed run start"
        );

        // Push initial user message
        self.with_session_mut(&session_id, |session| {
            session.push_message(MessageRole::User, user_input);
        })
        .await?;

        // Apply user message middleware
        let user_input_owned = self
            .apply_user_message_mw(&session_id, user_input.to_string())
            .await?;

        // Reset nudge_count and turn_tool_calls
        self.with_session_mut(&session_id, |session| {
            session.nudge_count = 0;
            session.turn_tool_calls = 0;
        })
        .await?;

        // Outer follow-up loop
        let mut current_input = user_input_owned;
        let mut final_outcome;
        let mut total_turns = 0u32;

        let config = self.config_snapshot_async().await;
        let max_turns = config
            .execution
            .max_turns
            .unwrap_or(crate::engine::runtime::DEFAULT_MAX_TURNS);

        loop {
            // Guard against unbounded execution: if follow-up messages
            // keep arriving, break out when the cumulative turn budget is
            // exhausted.
            if total_turns >= max_turns {
                tracing::warn!(
                    session_id = session_id.id,
                    total_turns,
                    max_turns,
                    "managed run: global turn cap reached"
                );
                final_outcome = RunOutcome::MaxTurnsExceeded { turns: total_turns };
                break;
            }

            // Run inner turn loop (steering drained at each iteration inside)
            let result = self
                .run_turn_loop(
                    &session_id,
                    &current_input,
                    &tool_definitions,
                    total_turns,
                    &mut event_rx,
                    &mut on_event,
                )
                .await;

            // Emit RunCancelled if cancelled
            let is_cancelled = matches!(&result, Err(e) if e.is_cancelled());
            if is_cancelled {
                on_event(RuntimeEvent::RunCancelled {
                    session_id: session_id.clone(),
                    agent_id: None,
                    trace_id: None,
                })?;
                // Safe: we just checked it's Err and cancelled
                return Err(result.unwrap_err());
            }

            // Emit RunCancelled for Ok(Cancelled) outcome
            if let Ok((RunOutcome::Cancelled, _)) = &result {
                on_event(RuntimeEvent::RunCancelled {
                    session_id: session_id.clone(),
                    agent_id: None,
                    trace_id: None,
                })?;
                return Ok(RunOutcome::Cancelled);
            }

            let (outcome, turns) = result?;
            total_turns += turns;
            final_outcome = outcome;

            // Check for follow-up messages
            let follow_up_msgs = self.message_queue.drain_follow_up();
            if follow_up_msgs.is_empty() {
                break;
            }

            tracing::info!(
                session_id = session_id.id,
                count = follow_up_msgs.len(),
                "drained follow-up messages, starting new inner loop"
            );

            for msg in follow_up_msgs {
                self.with_session_mut(&session_id, |session| {
                    session.push_message(MessageRole::User, &msg);
                })
                .await?;
            }

            // Use an empty input for follow-up continuation (messages are already in session)
            current_input = String::new();
        }

        // Emit RunFinished
        self.event_bus.emit(RuntimeEvent::RunFinished {
            session_id: session_id.clone(),
            agent_id: None,
            trace_id: None,
        });
        EventBus::drain_async_events(&mut event_rx, &mut on_event)?;

        // Clean up ephemeral messages
        if let Err(e) = self
            .with_session_mut(&session_id, |session| {
                session.remove_ephemeral_messages();
            })
            .await
        {
            tracing::warn!(error = %e, "failed to clean up ephemeral messages");
        }

        Ok(final_outcome)
    }

    pub async fn with_session_mut<F, R>(&self, session_id: &SessionId, f: F) -> AgentResult<R>
    where
        F: FnOnce(&mut crate::engine::AgentSession) -> R,
    {
        self.session_manager.with_session_mut(session_id, f).await
    }

    /// Build a TurnContext and fire all registered turn-end callbacks.
    /// agent-base does NOT store, aggregate, or persist metrics — consumers
    /// (e.g. phi-telemetry) do that via their registered callback.
    #[allow(clippy::too_many_arguments)]
    async fn fire_turn_end(
        &self,
        session_id: &SessionId,
        turn_number: u32,
        turn_start: std::time::Instant,
        model: &str,
        user_input: &str,
        ttft_ms: u64,
        llm_duration_ms: u64,
        tool_duration_ms: u64,
        usage: &Option<crate::llm::UsageInfo>,
        text_length: u64,
        has_thinking: bool,
        tool_call_count: u32,
        tools_used: &[String],
        tool_success: u32,
        tool_failed: u32,
        outcome: RunOutcome,
        error_message: Option<&str>,
        llm_calls: u32,
    ) {
        let duration_ms = turn_start.elapsed().as_millis() as u64;

        let ctx = crate::types::TurnContext {
            session_id: session_id.id,
            turn_number,
            ttft_ms,
            llm_duration_ms,
            duration_ms,
            tool_duration_ms,
            usage: usage.clone(),
            full_text_len: text_length,
            has_thinking,
            tools_used: tools_used.to_vec(),
            tool_call_count,
            tool_success,
            tool_failed,
            outcome,
            error_message: error_message.map(|s| s.to_string()),
            user_input: truncate_for_context(user_input),
            model: model.to_string(),
            plan_updates: self.event_bus.take_plan_updates(),
            approval_count: self.event_bus.take_approval_count(),
            llm_calls,
        };

        let callbacks = self.turn_end_callbacks.read().unwrap();
        for cb in callbacks.iter() {
            cb(&ctx);
        }
        drop(callbacks);
    }
}

/// Truncate a string to 80 characters (respecting UTF-8 boundaries),
/// appending "..." if truncated.
fn truncate_for_context(s: &str) -> String {
    if s.chars().count() > 80 {
        let truncated: String = s.chars().take(80).collect();
        format!("{}...", truncated)
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::AgentBuilder;
    use crate::engine::middleware::{Middleware, UserMessageCtx};
    use crate::llm::{LlmCapabilities, LlmClient, StreamChunk};
    use crate::types::{AgentError, AgentResult, ChatMessage, ResponseFormat, SessionId};
    use async_trait::async_trait;
    use futures_core::Stream;
    use serde_json::Value;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    /// Minimal LLM client for tests that don't need LLM calls.
    struct DummyClient;

    #[async_trait]
    impl LlmClient for DummyClient {
        async fn chat(
            &self,
            _messages: &[ChatMessage],
            _tools: &[Value],
            _reasoning: Option<&crate::ReasoningConfig>,
            _response_format: Option<&ResponseFormat>,
        ) -> AgentResult<Value> {
            Ok(Value::Null)
        }

        async fn chat_stream(
            &self,
            _messages: &[ChatMessage],
            _tools: &[Value],
            _reasoning: Option<&crate::ReasoningConfig>,
            _response_format: Option<&ResponseFormat>,
        ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
            unimplemented!("not used")
        }

        fn capabilities(&self) -> LlmCapabilities {
            LlmCapabilities::default()
        }
    }

    #[tokio::test]
    async fn run_turn_emits_run_finished_on_session_not_found() {
        let client: Arc<dyn LlmClient> = Arc::new(DummyClient);
        let runtime = AgentBuilder::new(client)
            .system_prompt("test")
            .build()
            .expect("build runtime");

        // Use a SessionId that was never created — session lookup will fail.
        let nonexistent = SessionId::new(99999);

        let event_fired = Arc::new(AtomicBool::new(false));
        let event_fired_clone = event_fired.clone();

        let result = runtime
            .run_turn(nonexistent.clone(), "test input", move |event| {
                if let RuntimeEvent::RunFinished { session_id: _, .. } = &event {
                    event_fired_clone.store(true, Ordering::SeqCst);
                }
                Ok(())
            })
            .await;

        // Must return an error
        assert!(
            result.is_err(),
            "run_turn should return Err for nonexistent session"
        );
        // Must have emitted RunFinished before returning
        assert!(
            event_fired.load(Ordering::SeqCst),
            "run_turn must emit RunFinished before returning Err on session not found"
        );
    }

    /// Middleware that always fails — used to test the middleware error path.
    struct FailingMiddleware;

    #[async_trait]
    impl Middleware for FailingMiddleware {
        async fn on_user_message(&self, _ctx: &mut UserMessageCtx) -> AgentResult<()> {
            Err(AgentError::internal("middleware intentionally fails"))
        }
    }

    #[tokio::test]
    async fn run_turn_emits_run_finished_on_middleware_failure() {
        let client: Arc<dyn LlmClient> = Arc::new(DummyClient);
        let runtime = AgentBuilder::new(client)
            .system_prompt("test")
            .middleware(FailingMiddleware)
            .build()
            .expect("build runtime");

        // Create a valid session — middleware failure happens AFTER session lookup.
        let sid = runtime.create_session().await;

        let event_fired = Arc::new(AtomicBool::new(false));
        let event_fired_clone = event_fired.clone();

        let result = runtime
            .run_turn(sid, "test input", move |event| {
                if let RuntimeEvent::RunFinished { session_id: _, .. } = &event {
                    event_fired_clone.store(true, Ordering::SeqCst);
                }
                Ok(())
            })
            .await;

        assert!(
            result.is_err(),
            "run_turn should return Err when middleware fails"
        );
        assert!(
            event_fired.load(Ordering::SeqCst),
            "run_turn must emit RunFinished before returning Err on middleware failure"
        );
    }

    /// LLM client whose stream immediately yields an error.
    struct ErrorStreamClient;

    #[async_trait]
    impl LlmClient for ErrorStreamClient {
        async fn chat(
            &self,
            _messages: &[ChatMessage],
            _tools: &[Value],
            _reasoning: Option<&crate::ReasoningConfig>,
            _response_format: Option<&ResponseFormat>,
        ) -> AgentResult<Value> {
            Ok(Value::Null)
        }

        async fn chat_stream(
            &self,
            _messages: &[ChatMessage],
            _tools: &[Value],
            _reasoning: Option<&crate::ReasoningConfig>,
            _response_format: Option<&ResponseFormat>,
        ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
            // Return a stream that immediately yields an error then ends.
            struct ErrorStream;
            impl Stream for ErrorStream {
                type Item = AgentResult<StreamChunk>;
                fn poll_next(
                    self: Pin<&mut Self>,
                    _cx: &mut Context<'_>,
                ) -> Poll<Option<Self::Item>> {
                    Poll::Ready(Some(Err(AgentError::internal("simulated LLM error"))))
                }
            }
            Ok(Box::pin(ErrorStream))
        }

        fn capabilities(&self) -> LlmCapabilities {
            LlmCapabilities::default()
        }
    }

    #[tokio::test]
    async fn run_turn_emits_run_finished_on_llm_error() {
        let client: Arc<dyn LlmClient> = Arc::new(ErrorStreamClient);
        let runtime = AgentBuilder::new(client)
            .system_prompt("test")
            .build()
            .expect("build runtime");

        let sid = runtime.create_session().await;

        let event_fired = Arc::new(AtomicBool::new(false));
        let event_fired_clone = event_fired.clone();

        let result = runtime
            .run_turn(sid, "test input", move |event| {
                if let RuntimeEvent::RunFinished { session_id: _, .. } = &event {
                    event_fired_clone.store(true, Ordering::SeqCst);
                }
                Ok(())
            })
            .await;

        // LLM errors should still emit RunFinished so event listeners don't hang.
        assert!(
            event_fired.load(Ordering::SeqCst),
            "run_turn must emit RunFinished when LLM returns an error"
        );
        // Note: the react loop may retry LLM errors, so the result might be Ok (retry succeeded
        // via retry logic) or Err.  Either is fine — the key assertion is that RunFinished fires.
        let _ = result;
    }

    /// Mock LLM that returns scripted responses — one Vec<StreamChunk> per call.
    struct ScriptedClient {
        script: Mutex<std::vec::IntoIter<Vec<StreamChunk>>>,
    }

    impl ScriptedClient {
        fn new(script: Vec<Vec<StreamChunk>>) -> Self {
            Self {
                script: Mutex::new(script.into_iter()),
            }
        }
    }

    #[async_trait]
    impl LlmClient for ScriptedClient {
        async fn chat(
            &self,
            _messages: &[ChatMessage],
            _tools: &[Value],
            _reasoning: Option<&crate::ReasoningConfig>,
            _response_format: Option<&ResponseFormat>,
        ) -> AgentResult<Value> {
            Ok(Value::Null)
        }

        async fn chat_stream(
            &self,
            _messages: &[ChatMessage],
            _tools: &[Value],
            _reasoning: Option<&crate::ReasoningConfig>,
            _response_format: Option<&ResponseFormat>,
        ) -> AgentResult<Pin<Box<dyn Stream<Item = AgentResult<StreamChunk>> + Send>>> {
            let chunks: Vec<AgentResult<StreamChunk>> = self
                .script
                .lock()
                .unwrap()
                .next()
                .unwrap_or_default()
                .into_iter()
                .map(Ok)
                .collect();
            Ok(Box::pin(futures_util::stream::iter(chunks)))
        }

        fn capabilities(&self) -> LlmCapabilities {
            LlmCapabilities {
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                supports_thinking: false,
                max_context_tokens: None,
                max_output_tokens: None,
            }
        }
    }

    #[tokio::test]
    async fn truncation_guard_blocks_tool_calls_on_length_finish_reason() {
        // First call: tool call with finish_reason="length" — should be blocked by guard.
        // Second call: model retries with corrected approach (text response).
        let client = Arc::new(ScriptedClient::new(vec![
            // Turn 1: truncated tool call
            vec![
                StreamChunk::ToolCall(serde_json::json!({
                    "delta": {
                        "tool_calls": [{
                            "id": "call_trunc",
                            "function": {
                                "name": "shell",
                                "arguments": "{\"cmd\": \"rm -rf /inco"
                            }
                        }]
                    }
                })),
                StreamChunk::Stop {
                    finish_reason: Some("length".to_string()),
                },
            ],
            // Turn 2: model sees the error and retries
            vec![
                StreamChunk::Text(
                    "I see the previous call was truncated. Let me re-issue it.".to_string(),
                ),
                StreamChunk::Stop {
                    finish_reason: Some("stop".to_string()),
                },
            ],
        ]));

        let runtime = AgentBuilder::new(client)
            .system_prompt("You are a careful assistant.")
            .build()
            .expect("build runtime");

        let sid = runtime.create_session().await;

        let mut events = Vec::new();
        let result = runtime
            .run_turn(sid.clone(), "run a command", |event| {
                events.push(event);
                Ok(())
            })
            .await;

        assert!(
            result.is_ok(),
            "run_turn should complete: {:?}",
            result.err()
        );

        // Verify the session messages contain the truncation error, not the
        // partial argument that would have been executed.
        let session = runtime.session(&sid).await.expect("session exists");
        let messages = session.chat_messages().to_vec();

        let has_truncation_error = messages.iter().any(|m| {
            if let ChatMessage::Tool { content, .. } = m {
                content.contains("Tool call was not executed")
                    && content.contains("output token limit")
            } else {
                false
            }
        });
        assert!(
            has_truncation_error,
            "session should contain truncation error tool result. Messages: {:#?}",
            messages
                .iter()
                .map(|m| format!("{:?}", m))
                .collect::<Vec<_>>()
        );
    }
}
