use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::llm::LlmClient;
use crate::tool::{ToolPolicy, ToolRegistry};
use crate::types::{AgentConfig, MessageRole, AgentError, CheckpointData, CheckpointStep};
use tokio::sync::broadcast;
use tracing::Span;

use crate::types::{AgentResult, AgentEvent, SessionId};
use super::approval::ApprovalHandler;
use super::context::ContextWindowManager;
use super::middleware::{MiddlewareRef, UserMessageCtx, PreLlmCtx, PostLlmCtx};
use super::session_store::SessionStore;
use super::AgentSession;

mod approval_flow;
mod llm;
mod tool_exec;

use tool_exec::ToolCallResult;

const DEFAULT_MAX_TURNS: u32 = 50;

pub struct AgentRuntime {
    pub(crate) client: Arc<dyn LlmClient>,
    pub(crate) config: AgentConfig,
    pub(crate) tools: ToolRegistry,
    pub(crate) approval_handler: Option<Arc<dyn ApprovalHandler>>,
    pub(crate) tool_policy: Option<Arc<dyn ToolPolicy>>,
    pub(crate) middlewares: Vec<MiddlewareRef>,
    pub(crate) event_bus: broadcast::Sender<AgentEvent>,
    pub(crate) next_session_id: AtomicU64,
    pub(crate) sessions: HashMap<SessionId, AgentSession>,
    pub(crate) context_manager: Option<ContextWindowManager>,
    pub(crate) session_store: Arc<dyn SessionStore>,
}

impl AgentRuntime {
    pub fn create_session(&mut self) -> SessionId {
        let id = SessionId {
            id: self.next_session_id.fetch_add(1, Ordering::Relaxed),
            external_id: None,
        };
        let mut session = AgentSession::new(id.clone());
        if let Some(system_prompt) = self.config.system_prompt.as_deref() {
            session.push_message(MessageRole::System, system_prompt);
        }
        self.sessions.insert(id.clone(), session);
        id
    }

    pub fn session(&self, session_id: &SessionId) -> Option<&AgentSession> {
        self.sessions.get(session_id)
    }

    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    pub fn client(&self) -> &Arc<dyn LlmClient> {
        &self.client
    }

    pub fn approval_handler(&self) -> Option<&Arc<dyn ApprovalHandler>> {
        self.approval_handler.as_ref()
    }

    pub fn tool_policy(&self) -> Option<&Arc<dyn ToolPolicy>> {
        self.tool_policy.as_ref()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<AgentEvent> {
        self.event_bus.subscribe()
    }

    pub fn session_store(&self) -> &Arc<dyn SessionStore> {
        &self.session_store
    }

    fn cached_approval(&self, session_id: &SessionId, action_key: &str) -> bool {
        self.sessions
            .get(session_id)
            .is_some_and(|session| session.is_action_allowed(action_key))
    }

    fn cache_approval(&mut self, session_id: &SessionId, action_key: String) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.allow_action(action_key);
        }
    }

    fn emit_event(&self, event: AgentEvent) {
        let _ = self.event_bus.send(event);
    }

    fn session_or_err(&self, session_id: &SessionId) -> AgentResult<&AgentSession> {
        self.sessions
            .get(session_id)
            .ok_or_else(|| AgentError::session_not_found(session_id.id))
    }

    fn session_mut_or_err(&mut self, session_id: &SessionId) -> AgentResult<&mut AgentSession> {
        self.sessions
            .get_mut(session_id)
            .ok_or_else(|| AgentError::session_not_found(session_id.id))
    }

    fn drain_async_events<F>(
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<()>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        loop {
            match event_rx.try_recv() {
                Ok(event) => on_event(event)?,
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }
        Ok(())
    }

    pub async fn run_turn_with_handler<F>(
        &mut self,
        session_id: SessionId,
        user_input: &str,
        mut on_event: F,
    ) -> AgentResult<()>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let span = Span::current();
        let _guard = span.enter();
        tracing::info!(session_id = session_id.id, user_input = %user_input, "agent turn start");
        drop(_guard);

        let mut event_rx = self.subscribe_events();
        let tool_definitions = self.tools.definitions();

        let mut user_input_owned = user_input.to_string();

        {
            let mut ctx = UserMessageCtx {
                session_id: session_id.clone(),
                user_input: user_input_owned.clone(),
                event_bus: self.event_bus.clone(),
            };
            for mw in &self.middlewares {
                mw.on_user_message(&mut ctx).await?;
            }
            user_input_owned = ctx.user_input;
        }

        {
            let session = self.session_mut_or_err(&session_id)?;
            session.push_message(MessageRole::User, &user_input_owned);
        }

        self.emit_event(AgentEvent::Checkpoint {
            session_id: session_id.clone(),
            checkpoint: CheckpointData {
                session_id: session_id.clone(),
                user_input: user_input_owned.clone(),
                step: CheckpointStep::AfterUserInput,
                turn_count: 0,
            },
        });

        let max_turns = self.config.max_turns.unwrap_or(DEFAULT_MAX_TURNS);
        let mut turn_count: u32 = 0;

        loop {
            turn_count += 1;

            if turn_count > max_turns {
                self.emit_event(AgentEvent::RunFailed {
                    session_id: session_id.clone(),
                    error: format!("达到最大轮次限制（{max_turns}次），强制停止"),
                });
                Self::drain_async_events(&mut event_rx, &mut on_event)?;
                break;
            }

            Self::drain_async_events(&mut event_rx, &mut on_event)?;

            let turn_span = tracing::info_span!("turn", session_id = session_id.id, turn = turn_count);
            let _turn_guard = turn_span.enter();

            let mut messages: Vec<_> = self.session_or_err(&session_id)?.chat_messages().to_vec();
            let mut tools_for_turn = tool_definitions.clone();

            if let Some(ref ctx_mgr) = self.context_manager {
                ctx_mgr.trim(&mut messages);
            }

            {
                let mut ctx = PreLlmCtx {
                    session_id: session_id.clone(),
                    messages: messages.clone(),
                    tools: tools_for_turn.clone(),
                    event_bus: self.event_bus.clone(),
                };
                for mw in &self.middlewares {
                    mw.on_pre_llm(&mut ctx).await?;
                }
                messages = ctx.messages;
                tools_for_turn = ctx.tools;
            }

            self.emit_event(AgentEvent::Checkpoint {
                session_id: session_id.clone(),
                checkpoint: CheckpointData {
                    session_id: session_id.clone(),
                    user_input: user_input_owned.clone(),
                    step: CheckpointStep::BeforeLlm {
                        messages: messages.clone(),
                        tools: tools_for_turn.clone(),
                    },
                    turn_count,
                },
            });

            let aggregator = self
                .execute_llm_turn(&session_id, &messages, &tools_for_turn, &mut event_rx, &mut on_event)
                .await?;

            let (mut full_text, mut is_tool_call, mut tool_calls) = aggregator.into_parts();

            {
                let mut ctx = PostLlmCtx {
                    session_id: session_id.clone(),
                    full_text: full_text.clone(),
                    is_tool_call,
                    tool_calls: tool_calls.clone(),
                    event_bus: self.event_bus.clone(),
                };
                for mw in &self.middlewares {
                    mw.on_post_llm(&mut ctx).await?;
                }
                full_text = ctx.full_text;
                is_tool_call = ctx.is_tool_call;
                tool_calls = ctx.tool_calls;
            }

            if full_text.is_empty() && !is_tool_call {
                continue;
            }

            if !full_text.is_empty() {
                let session = self.session_mut_or_err(&session_id)?;
                session.push_message(MessageRole::Assistant, full_text);
            }

            if is_tool_call && !tool_calls.is_empty() {
                self.emit_event(AgentEvent::Checkpoint {
                    session_id: session_id.clone(),
                    checkpoint: CheckpointData {
                        session_id: session_id.clone(),
                        user_input: user_input_owned.clone(),
                        step: CheckpointStep::BeforeToolCalls {
                            tool_calls: tool_calls.clone(),
                        },
                        turn_count,
                    },
                });

                match self
                    .handle_tool_calls(
                        &session_id,
                        &tool_calls,
                        &mut event_rx,
                        &mut on_event,
                    )
                    .await
                {
                    Ok(ToolCallResult::Continue) => {
                        self.emit_event(AgentEvent::Checkpoint {
                            session_id: session_id.clone(),
                            checkpoint: CheckpointData {
                                session_id: session_id.clone(),
                                user_input: user_input_owned.clone(),
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
                        self.emit_event(AgentEvent::RunCompleted { session_id: session_id.clone() });
                        Self::drain_async_events(&mut event_rx, &mut on_event)?;
                        break;
                    }
                    Err(e) => {
                        if e.is_cancelled() {
                            return Err(e);
                        }
                        let names: Vec<&str> = tool_calls.iter().map(|(_, n, _)| n.as_str()).collect();
                        let session = self.session_mut_or_err(&session_id)?;
                        session.push_message(
                            MessageRole::Assistant,
                            format!("(尝试调用工具 {} 失败)", names.join(", ")),
                        );
                        session.push_message(
                            MessageRole::User,
                            "你刚才尝试调用工具时出现了错误。请简化你的计划，然后重新调用工具。",
                        );
                        continue;
                    }
                }
            }

            self.emit_event(AgentEvent::RunCompleted { session_id: session_id.clone() });
            Self::drain_async_events(&mut event_rx, &mut on_event)?;
            break;
        }

        let session = self.session_or_err(&session_id)?;
        let _ = self.session_store.save(session).await;

        tracing::info!(session_id = session_id.id, turn_count, "agent turn completed");
        Ok(())
    }

    pub async fn run_turn_stream(
        &mut self,
        session_id: SessionId,
        user_input: &str,
    ) -> AgentResult<Vec<AgentEvent>> {
        let mut events = Vec::new();
        self.run_turn_with_handler(session_id, user_input, |event| {
            events.push(event);
            Ok(())
        })
        .await?;
        Ok(events)
    }

    pub async fn resume_from_checkpoint<F>(
        &mut self,
        checkpoint: CheckpointData,
        mut on_event: F,
    ) -> AgentResult<()>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let session_id = checkpoint.session_id;
        let user_input = checkpoint.user_input;
        let turn_count = checkpoint.turn_count;

        tracing::info!(session_id = session_id.id, turn_count, step = ?checkpoint.step, "resuming from checkpoint");

        let mut event_rx = self.subscribe_events();
        let tool_definitions = self.tools.definitions();
        let max_turns = self.config.max_turns.unwrap_or(DEFAULT_MAX_TURNS);
        let mut turn_count = turn_count;

        match checkpoint.step {
            CheckpointStep::BeforeToolCalls { tool_calls } => {
                match self
                    .handle_tool_calls(
                        &session_id,
                        &tool_calls,
                        &mut event_rx,
                        &mut on_event,
                    )
                    .await
                {
                    Ok(ToolCallResult::Continue) => {}
                    Ok(ToolCallResult::Break) => {
                        self.emit_event(AgentEvent::RunCompleted { session_id: session_id.clone() });
                        Self::drain_async_events(&mut event_rx, &mut on_event)?;
                        return Ok(());
                    }
                    Err(e) => {
                        if e.is_cancelled() {
                            return Err(e);
                        }
                        let names: Vec<&str> = tool_calls.iter().map(|(_, n, _)| n.as_str()).collect();
                        let session = self.session_mut_or_err(&session_id)?;
                        session.push_message(
                            MessageRole::Assistant,
                            format!("(尝试调用工具 {} 失败)", names.join(", ")),
                        );
                        session.push_message(
                            MessageRole::User,
                            "你刚才尝试调用工具时出现了错误。请简化你的计划，然后重新调用工具。",
                        );
                    }
                }
            }
            _ => {}
        }

        loop {
            turn_count += 1;

            if turn_count > max_turns {
                self.emit_event(AgentEvent::RunFailed {
                    session_id: session_id.clone(),
                    error: format!("达到最大轮次限制（{max_turns}次），强制停止"),
                });
                Self::drain_async_events(&mut event_rx, &mut on_event)?;
                break;
            }

            Self::drain_async_events(&mut event_rx, &mut on_event)?;

            let mut messages: Vec<_> = self.session_or_err(&session_id)?.chat_messages().to_vec();
            let mut tools_for_turn = tool_definitions.clone();

            if let Some(ref ctx_mgr) = self.context_manager {
                ctx_mgr.trim(&mut messages);
            }

            {
                let mut ctx = PreLlmCtx {
                    session_id: session_id.clone(),
                    messages: messages.clone(),
                    tools: tools_for_turn.clone(),
                    event_bus: self.event_bus.clone(),
                };
                for mw in &self.middlewares {
                    mw.on_pre_llm(&mut ctx).await?;
                }
                messages = ctx.messages;
                tools_for_turn = ctx.tools;
            }

            self.emit_event(AgentEvent::Checkpoint {
                session_id: session_id.clone(),
                checkpoint: CheckpointData {
                    session_id: session_id.clone(),
                    user_input: user_input.clone(),
                    step: CheckpointStep::BeforeLlm {
                        messages: messages.clone(),
                        tools: tools_for_turn.clone(),
                    },
                    turn_count,
                },
            });

            let aggregator = self
                .execute_llm_turn(&session_id, &messages, &tools_for_turn, &mut event_rx, &mut on_event)
                .await?;

            let (mut full_text, mut is_tool_call, mut tool_calls) = aggregator.into_parts();

            {
                let mut ctx = PostLlmCtx {
                    session_id: session_id.clone(),
                    full_text: full_text.clone(),
                    is_tool_call,
                    tool_calls: tool_calls.clone(),
                    event_bus: self.event_bus.clone(),
                };
                for mw in &self.middlewares {
                    mw.on_post_llm(&mut ctx).await?;
                }
                full_text = ctx.full_text;
                is_tool_call = ctx.is_tool_call;
                tool_calls = ctx.tool_calls;
            }

            if full_text.is_empty() && !is_tool_call {
                continue;
            }

            if !full_text.is_empty() {
                let session = self.session_mut_or_err(&session_id)?;
                session.push_message(MessageRole::Assistant, full_text);
            }

            if is_tool_call && !tool_calls.is_empty() {
                self.emit_event(AgentEvent::Checkpoint {
                    session_id: session_id.clone(),
                    checkpoint: CheckpointData {
                        session_id: session_id.clone(),
                        user_input: user_input.clone(),
                        step: CheckpointStep::BeforeToolCalls {
                            tool_calls: tool_calls.clone(),
                        },
                        turn_count,
                    },
                });

                match self
                    .handle_tool_calls(
                        &session_id,
                        &tool_calls,
                        &mut event_rx,
                        &mut on_event,
                    )
                    .await
                {
                    Ok(ToolCallResult::Continue) => {
                        self.emit_event(AgentEvent::Checkpoint {
                            session_id: session_id.clone(),
                            checkpoint: CheckpointData {
                                session_id: session_id.clone(),
                                user_input: user_input.clone(),
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
                        self.emit_event(AgentEvent::RunCompleted { session_id: session_id.clone() });
                        Self::drain_async_events(&mut event_rx, &mut on_event)?;
                        break;
                    }
                    Err(e) => {
                        if e.is_cancelled() {
                            return Err(e);
                        }
                        let names: Vec<&str> = tool_calls.iter().map(|(_, n, _)| n.as_str()).collect();
                        let session = self.session_mut_or_err(&session_id)?;
                        session.push_message(
                            MessageRole::Assistant,
                            format!("(尝试调用工具 {} 失败)", names.join(", ")),
                        );
                        session.push_message(
                            MessageRole::User,
                            "你刚才尝试调用工具时出现了错误。请简化你的计划，然后重新调用工具。",
                        );
                        continue;
                    }
                }
            }

            self.emit_event(AgentEvent::RunCompleted { session_id: session_id.clone() });
            Self::drain_async_events(&mut event_rx, &mut on_event)?;
            break;
        }

        let session = self.session_or_err(&session_id)?;
        let _ = self.session_store.save(session).await;

        tracing::info!(session_id = session_id.id, turn_count, "agent resume completed");
        Ok(())
    }
}
