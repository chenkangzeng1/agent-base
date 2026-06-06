use tokio::sync::broadcast;

use crate::engine::middleware::{PostLlmCtx, PreLlmCtx, UserMessageCtx};
use crate::engine::recovery::ToolErrorAction;
use crate::engine::runtime::event_bus::EventBus;
use crate::engine::runtime::llm_engine::LlmTurnResult;
use crate::types::{AgentError, AgentEvent, AgentResult, CheckpointData, CheckpointStep, MessageRole, RunOutcome, RuntimeEvent, SessionId};

use super::AgentRuntime;

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

impl AgentRuntime {
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
        let total_tool_calls = self.session_or_err(session_id).await?.total_tool_calls;
        let mut ctx = PostLlmCtx {
            session_id: session_id.clone(),
            full_text,
            is_tool_call,
            tool_calls,
            available_tools: available_tools.to_vec(),
            turn_count,
            total_tool_calls,
            skip_push: false,
            follow_up_message: None,
        };
        for mw in &self.middlewares {
            mw.on_post_llm(&mut ctx).await?;
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
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<Option<RunOutcome>>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        if e.is_cancelled() {
            self.emit_event(AgentEvent::RunFinished { session_id: session_id.clone() });
            EventBus::drain_async_events(event_rx, on_event)?;
            let session = self.session_or_err(session_id).await?;
            if let Err(e) = self.session_manager.session_store().save(&session).await {
                tracing::warn!(session_id = session_id.id, error = %e, "Failed to persist session");
                if self.config.execution.fail_on_persist_error {
                    return Err(AgentError::internal(format!("Session persistence failed: {e}")));
                }
            }
            return Err(e);
        }

        let names: Vec<String> = tool_calls.iter().map(|(_, n, _)| n.clone()).collect();
        let error_text = e.to_string();
        let retry_prompt_template: Option<String> = self.config.tool.tool_error_retry_prompt.clone();
        let action = self.tool_engine.error_recovery().on_error(session_id, &names, &e)?;
        match action {
            ToolErrorAction::Stop => {
                self.emit_event(AgentEvent::RunFinished { session_id: session_id.clone() });
                EventBus::drain_async_events(event_rx, on_event)?;
                let session = self.session_or_err(session_id).await?;
                if let Err(e) = self.session_manager.session_store().save(&session).await {
                    tracing::warn!(session_id = session_id.id, error = %e, "Failed to persist session");
                    if self.config.execution.fail_on_persist_error {
                        return Err(AgentError::internal(format!("Session persistence failed: {e}")));
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
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let max_turns = self.config.execution.max_turns.unwrap_or(super::DEFAULT_MAX_TURNS);

        tracing::debug!(session_id = session_id.id, max_turns, "run turn loop start");

        loop {
            turn_count += 1;

            if turn_count > max_turns {
                tracing::warn!(session_id = session_id.id, turn_count, max_turns, "max turns exceeded");
                self.emit_event(AgentEvent::RunFinished {
                    session_id: session_id.clone(),
                });
                EventBus::drain_async_events(event_rx, on_event)?;
                return Ok((RunOutcome::MaxTurnsExceeded { turns: turn_count }, turn_count));
            }

            EventBus::drain_async_events(event_rx, on_event)?;

            let turn_span = tracing::info_span!("turn", session_id = session_id.id, turn = turn_count);
            let _turn_guard = turn_span.enter();

            let session = self.session_or_err(session_id).await?;
            let mut messages: Vec<_> = session.chat_messages().to_vec();
            let tools_for_turn = tool_definitions.to_vec();

            if let Some(ref ctx_mgr) = self.context_manager {
                let before = messages.len();
                ctx_mgr.trim(&mut messages);
                tracing::debug!(session_id = session_id.id, turn = turn_count, before, after = messages.len(), "context trimmed");
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

            let stream = match self.config.llm.llm_retry.as_ref() {
                Some(retry) => {
                    self.llm_engine.run_llm_turn_with_retry(
                        session_id,
                        &messages,
                        &tools_for_turn,
                        self.config.reasoning.as_ref(),
                        self.config.llm.response_format.as_ref(),
                        retry.clone(),
                    ).await?
                }
                None => {
                    self.llm_engine.chat_stream(
                        &messages,
                        &tools_for_turn,
                        self.config.reasoning.as_ref(),
                        self.config.llm.response_format.as_ref(),
                    ).await?
                }
            };

            let span = tracing::info_span!("llm_turn", session_id = session_id.id, turn = turn_count);
            let result = self.llm_engine.process_stream(session_id, stream, span, event_rx, on_event).await;

            match result {
                Ok(LlmTurnResult { full_text, is_tool_call, tool_calls, usage: _ }) => {
                    let tool_calls_parsed: Vec<(String, String, String)> = tool_calls.iter().map(|tc| {
                        let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let name = tc.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()).unwrap_or("").to_string();
                        let args = tc.get("function").and_then(|f| f.get("arguments")).and_then(|a| a.as_str()).unwrap_or("").to_string();
                        (id, name, args)
                    }).collect();

                    let available_tools: Vec<String> = tool_definitions.iter()
                        .filter_map(|d| d.get("function")?.get("name")?.as_str().map(|s| s.to_string()))
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
                        self.with_session_mut(session_id, |session| {
                            session.push_message(MessageRole::Assistant, &result.full_text);
                        }).await?;
                    }

                    if let Some(follow_up) = result.follow_up_message {
                        self.with_session_mut(session_id, |session| {
                            session.push_message(MessageRole::User, &follow_up);
                        }).await?;
                        continue;
                    }

                    if result.full_text.is_empty() && !result.is_tool_call {
                        tracing::debug!(session_id = session_id.id, turn = turn_count, "empty LLM response, continuing");
                        continue;
                    }

                    if result.is_tool_call && !result.tool_calls.is_empty() {
                        self.emit_event(AgentEvent::Checkpoint {
                            session_id: session_id.clone(),
                            checkpoint: CheckpointData {
                                session_id: session_id.clone(),
                                user_input: user_input_owned.to_string(),
                                step: CheckpointStep::BeforeToolCalls {
                                    tool_calls: result.tool_calls.clone(),
                                },
                                turn_count,
                            },
                        });

                        match self.handle_tool_calls(session_id, &result.tool_calls, event_rx, on_event).await {
                            Ok(ToolCallResult::Continue) => {
                                let n = result.tool_calls.len();
                                self.with_session_mut(session_id, |session| {
                                    session.total_tool_calls += n;
                                }).await?;
                                self.emit_event(AgentEvent::Checkpoint {
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
                                });
                                continue;
                            }
                            Ok(ToolCallResult::Break) => {
                                let n = result.tool_calls.len();
                                self.with_session_mut(session_id, |session| {
                                    session.total_tool_calls += n;
                                }).await?;
                                self.emit_event(AgentEvent::RunFinished { session_id: session_id.clone() });
                                EventBus::drain_async_events(event_rx, on_event)?;
                                return Ok((RunOutcome::Completed, turn_count));
                            }
                            Err(e) => {
                                if let Some(outcome) = self
                                    .handle_tool_error(session_id, &result.tool_calls, e, event_rx, on_event)
                                    .await?
                                {
                                    return Ok((outcome, turn_count));
                                }
                                continue;
                            }
                        }
                    }

                    self.emit_event(AgentEvent::RunFinished { session_id: session_id.clone() });
                    EventBus::drain_async_events(event_rx, on_event)?;
                    return Ok((RunOutcome::Completed, turn_count));
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }
    }

    pub(super) async fn handle_tool_calls<F>(
        &self,
        session_id: &SessionId,
        tool_calls: &[(String, String, String)],
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<ToolCallResult>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let tool_names: Vec<&str> = tool_calls.iter().map(|(_, name, _)| name.as_str()).collect();
        tracing::debug!(session_id = session_id.id, ?tool_names, "handle tool calls start");

        let mut parsed_calls: Vec<(String, String, String, serde_json::Value)> = Vec::new();
        for (id, name, args_str) in tool_calls {
            let args: serde_json::Value = serde_json::from_str(args_str).map_err(|_| AgentError::ToolArgsInvalid {
                name: name.clone(),
                raw: args_str.clone(),
            })?;

            self.tool_engine.process_approval(
                session_id, name, &args, args_str, event_rx, on_event, &self.session_manager,
            ).await?;

            parsed_calls.push((id.clone(), name.clone(), args_str.clone(), args));
        }



        {
            let tc: Vec<(String, String, String)> = parsed_calls
                .iter()
                .map(|(id, name, args_json, _)| (id.clone(), name.clone(), args_json.clone()))
                .collect();
            self.with_session_mut(session_id, |session| {
                session.push_assistant_tool_calls(&tc);
            }).await?;
        }

        for (id, name, args_str, args) in parsed_calls {
            let tool_result = self.tool_engine.execute_tool(
                session_id,
                &id,
                &name,
                &args,
                &args_str,
                event_rx,
                on_event,
                &self.session_manager,
                Some(self.llm_engine.client.clone()),
                self.config.language.clone(),
                self.config.tool.tool_timeout_ms,
                self.config.tool.max_tool_output_chars,
            ).await;

            match tool_result {
                Ok(result) => {
                    self.with_session_mut(session_id, |session| {
                        session.push_tool_result(&result.id, result.output.summary.clone());
                    }).await?;

                    if matches!(result.output.control_flow, crate::tool::ToolControlFlow::Break) {
                        return Ok(ToolCallResult::Break);
                    }
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        Ok(ToolCallResult::Continue)
    }
}
