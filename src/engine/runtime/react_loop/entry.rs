use crate::engine::middleware::UserMessageCtx;
use crate::engine::runtime::event_bus::EventBus;
use crate::engine::runtime::plan_runner::RuntimeCore;
use crate::types::{
    AgentError, AgentResult, CheckpointData, CheckpointStep, MessageRole, RunOutcome, RuntimeEvent,
    SessionId,
};

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
            Ok((RunOutcome::Cancelled, _)) => return Ok(RunOutcome::Cancelled),
            Ok(tuple) => {
                // The loop no longer emits RunFinished on completion; emit it
                // here once so listeners see a single terminal event.
                self.event_bus.emit(RuntimeEvent::RunFinished {
                    session_id: session_id.clone(),
                    agent_id: None,
                    trace_id: None,
                });
                EventBus::drain_async_events(&mut event_rx, &mut on_event)?;
                tuple
            }
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
                Ok(()) => {}
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

    async fn apply_user_message_mw(
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

    /// Managed run with follow-up queue support (P2).
    ///
    /// After the inner turn loop completes naturally (text response, no tool calls),
    /// follow-up messages are drained and a new inner loop is started. This repeats
    /// until no more follow-up messages are queued.
    pub(crate) async fn run_managed<F>(
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
                self.cleanup_ephemeral(&session_id).await;
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
                self.cleanup_ephemeral(&session_id).await;
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
        self.cleanup_ephemeral(&session_id).await;

        Ok(final_outcome)
    }

    /// Remove ephemeral messages from the session, best-effort (warn on failure).
    async fn cleanup_ephemeral(&self, session_id: &SessionId) {
        if let Err(e) = self
            .with_session_mut(session_id, |session| {
                session.remove_ephemeral_messages();
            })
            .await
        {
            tracing::warn!(error = %e, "failed to clean up ephemeral messages");
        }
    }
}
