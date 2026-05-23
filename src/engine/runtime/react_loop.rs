use tokio::sync::broadcast;

use crate::types::{AgentError, AgentEvent, AgentResult, CheckpointData, CheckpointStep, MessageRole, RunOutcome, SessionId};
use crate::engine::middleware::{PostLlmCtx, PreLlmCtx, UserMessageCtx};
use crate::engine::recovery::ToolErrorAction;
use crate::engine::AgentRuntime;
use super::tool_exec::ToolCallResult;

impl AgentRuntime {
    pub(super) async fn apply_user_message_mw(
        &self,
        session_id: &SessionId,
        user_input: String,
    ) -> AgentResult<String> {
        let mut ctx = UserMessageCtx {
            session_id: session_id.clone(),
            user_input,
            event_bus: self.event_bus.clone(),
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
            event_bus: self.event_bus.clone(),
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
    ) -> AgentResult<(String, bool, Vec<(String, String, String)>)> {
        let mut ctx = PostLlmCtx {
            session_id: session_id.clone(),
            full_text,
            is_tool_call,
            tool_calls,
            event_bus: self.event_bus.clone(),
        };
        for mw in &self.middlewares {
            mw.on_post_llm(&mut ctx).await?;
        }
        Ok((ctx.full_text, ctx.is_tool_call, ctx.tool_calls))
    }

    pub(super) async fn handle_tool_error<F>(
        &self,
        session_id: &SessionId,
        tool_calls: &[(String, String, String)],
        e: AgentError,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<Option<RunOutcome>>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        if e.is_cancelled() {
            self.emit_event(AgentEvent::RunFinished { session_id: session_id.clone() });
            Self::drain_async_events(event_rx, on_event)?;
            let session = self.session_or_err(session_id).await?;
            let _ = self.session_store.save(&session).await;
            return Err(e);
        }

        let names: Vec<String> = tool_calls.iter().map(|(_, n, _)| n.clone()).collect();
        let error_text = e.to_string();
        let retry_prompt_template: Option<String> = self.config.tool_error_retry_prompt.clone();
        let action = self.error_recovery.on_error(session_id, &names, &e)?;
        match action {
            ToolErrorAction::Stop => {
                self.emit_event(AgentEvent::RunFinished { session_id: session_id.clone() });
                Self::drain_async_events(event_rx, on_event)?;
                let session = self.session_or_err(session_id).await?;
                let _ = self.session_store.save(&session).await;
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

                self.with_session_mut(session_id, |session| {
                    session.close_dangling_tool_calls(&format!("[Tool Execution Failed] {}", error_text));
                    session.push_message(MessageRole::User, retry_prompt);
                }).await?;
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
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<(RunOutcome, u32)>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let max_turns = self.config.max_turns.unwrap_or(super::DEFAULT_MAX_TURNS);

        loop {
            turn_count += 1;

            if turn_count > max_turns {
                self.emit_event(AgentEvent::RunFinished {
                    session_id: session_id.clone(),
                });
                Self::drain_async_events(event_rx, on_event)?;
                return Ok((RunOutcome::MaxTurnsExceeded { turns: turn_count }, turn_count));
            }

            Self::drain_async_events(event_rx, on_event)?;

            let turn_span = tracing::info_span!("turn", session_id = session_id.id, turn = turn_count);
            let _turn_guard = turn_span.enter();

            let session = self.session_or_err(session_id).await?;
            let mut messages: Vec<_> = session.chat_messages().to_vec();
            let tools_for_turn = tool_definitions.to_vec();

            if let Some(ref ctx_mgr) = self.context_manager {
                ctx_mgr.trim(&mut messages);
            }

            let (messages, tools_for_turn) = self.apply_pre_llm_mw(session_id, messages, tools_for_turn).await?;

            self.emit_event(AgentEvent::Checkpoint {
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
            });

            let aggregator = self
                .execute_llm_turn(session_id, &messages, &tools_for_turn, event_rx, on_event)
                .await?;

            let (full_text, is_tool_call, tool_calls) = aggregator.into_parts();

            let (full_text, is_tool_call, tool_calls) = self
                .apply_post_llm_mw(session_id, full_text, is_tool_call, tool_calls)
                .await?;

            if full_text.is_empty() && !is_tool_call {
                continue;
            }

            if !full_text.is_empty() {
                self.with_session_mut(session_id, |session| {
                    session.push_message(MessageRole::Assistant, full_text);
                }).await?;
            }

            if is_tool_call && !tool_calls.is_empty() {
                self.emit_event(AgentEvent::Checkpoint {
                    session_id: session_id.clone(),
                    checkpoint: CheckpointData {
                        session_id: session_id.clone(),
                        user_input: user_input_owned.to_string(),
                        step: CheckpointStep::BeforeToolCalls {
                            tool_calls: tool_calls.clone(),
                        },
                        turn_count,
                    },
                });

                match self.handle_tool_calls(session_id, &tool_calls, event_rx, on_event).await {
                    Ok(ToolCallResult::Continue) => {
                        self.emit_event(AgentEvent::Checkpoint {
                            session_id: session_id.clone(),
                            checkpoint: CheckpointData {
                                session_id: session_id.clone(),
                                user_input: user_input_owned.to_string(),
                                step: CheckpointStep::AfterToolCalls {
                                    tool_calls: tool_calls.clone(),
                                    results: Vec::new(),
                                },
                                turn_count,
                            },
                        });
                        continue;
                    }
                    Ok(ToolCallResult::Break) => {
                        self.emit_event(AgentEvent::RunFinished { session_id: session_id.clone() });
                        Self::drain_async_events(event_rx, on_event)?;
                        break;
                    }
                    Err(e) => {
                        if let Some(outcome) = self
                            .handle_tool_error(session_id, &tool_calls, e, event_rx, on_event)
                            .await?
                        {
                            return Ok((outcome, turn_count));
                        }
                        continue;
                    }
                }
            }

            self.emit_event(AgentEvent::RunFinished { session_id: session_id.clone() });
            Self::drain_async_events(event_rx, on_event)?;
            break;
        }

        let session = self.session_or_err(session_id).await?;
        let _ = self.session_store.save(&session).await;

        Ok((RunOutcome::Completed, turn_count))
    }
}
