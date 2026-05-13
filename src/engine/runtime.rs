use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::llm::LlmClient;
use crate::tool::{ToolContext, ToolOutput, ToolControlFlow, ToolPolicy, ToolRegistry};
use crate::types::{AgentResult, AgentConfig, MessageRole};
use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::types::{AgentEvent, ApprovalDecision, SessionId};
use super::approval::ApprovalHandler;
use super::AgentSession;

const DEFAULT_MAX_TURNS: u32 = 50;

pub struct AgentRuntime {
    pub(crate) client: Arc<dyn LlmClient>,
    pub(crate) config: AgentConfig,
    pub(crate) tools: ToolRegistry,
    pub(crate) approval_handler: Option<Arc<dyn ApprovalHandler>>,
    pub(crate) tool_policy: Option<Arc<dyn ToolPolicy>>,
    pub(crate) event_bus: broadcast::Sender<AgentEvent>,
    pub(crate) next_session_id: AtomicU64,
    pub(crate) sessions: HashMap<SessionId, AgentSession>,
}

impl AgentRuntime {
    pub fn create_session(&mut self) -> SessionId {
        let id = SessionId(self.next_session_id.fetch_add(1, Ordering::Relaxed));
        let mut session = AgentSession::new(id);
        if let Some(system_prompt) = self.config.system_prompt.as_deref() {
            session.push_message(MessageRole::System, system_prompt);
        }
        self.sessions.insert(id, session);
        id
    }

    pub fn session(&self, session_id: SessionId) -> Option<&AgentSession> {
        self.sessions.get(&session_id)
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

    fn cached_approval(&self, session_id: SessionId, action_key: &str) -> bool {
        self.sessions
            .get(&session_id)
            .map(|session| session.is_action_allowed(action_key))
            .unwrap_or(false)
    }

    fn cache_approval(&mut self, session_id: SessionId, action_key: String) {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.allow_action(action_key);
        }
    }

    fn emit_event(&self, event: AgentEvent) {
        let _ = self.event_bus.send(event);
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
        let mut event_rx = self.subscribe_events();
        let tool_definitions = self.tools.definitions();
        let Some(session) = self.sessions.get_mut(&session_id) else {
            anyhow::bail!("session {} not found", session_id.0);
        };

        session.push_message(MessageRole::User, user_input);

        let mut turn_count: u32 = 0;

        loop {
            turn_count += 1;
            if turn_count > DEFAULT_MAX_TURNS {
                self.emit_event(AgentEvent::RunFailed {
                    session_id,
                    error: format!("达到最大轮次限制（{}次），强制停止", DEFAULT_MAX_TURNS),
                });
                Self::drain_async_events(&mut event_rx, &mut on_event)?;
                break;
            }

            Self::drain_async_events(&mut event_rx, &mut on_event)?;
            let raw_messages = self
                .sessions
                .get(&session_id)
                .map(|session| session.raw_messages().to_vec())
                .ok_or_else(|| anyhow::anyhow!("session {} not found", session_id.0))?;
            let mut stream = self
                .client
                .chat_stream(&raw_messages, &tool_definitions)
                .await?;

            let mut full_text_reply = String::new();
            let mut tool_call_id = String::new();
            let mut tool_name = String::new();
            let mut tool_args_json = String::new();
            let mut is_tool_call = false;

            loop {
                tokio::select! {
                    recv_result = event_rx.recv() => {
                        match recv_result {
                            Ok(event) => on_event(event)?,
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    maybe_chunk = stream.next() => {
                        let Some(chunk_result) = maybe_chunk else {
                            break;
                        };
                        match chunk_result? {
                            crate::llm::StreamChunk::Text(text) => {
                                if !text.is_empty() && !is_tool_call {
                                    full_text_reply.push_str(&text);
                                    self.emit_event(AgentEvent::TextDelta {
                                        session_id,
                                        text,
                                    });
                                    Self::drain_async_events(&mut event_rx, &mut on_event)?;
                                }
                            }
                            crate::llm::StreamChunk::ToolCall(choice) => {
                                is_tool_call = true;
                                if let Some(tool_calls) = choice
                                    .get("delta")
                                    .and_then(|d| d.get("tool_calls"))
                                    .and_then(Value::as_array)
                                {
                                    for tool_call in tool_calls {
                                        if let Some(id) = tool_call.get("id").and_then(Value::as_str) {
                                            tool_call_id = id.to_string();
                                        }
                                        if let Some(func) = tool_call.get("function") {
                                            if let Some(name) = func.get("name").and_then(Value::as_str) {
                                                tool_name = name.to_string();
                                            }
                                            if let Some(args) =
                                                func.get("arguments").and_then(Value::as_str)
                                            {
                                                tool_args_json.push_str(args);
                                            }
                                        }
                                    }
                                }
                            }
                            crate::llm::StreamChunk::Stop => break,
                        }
                    }
                }
            }

            if !full_text_reply.is_empty() {
                let Some(session) = self.sessions.get_mut(&session_id) else {
                    anyhow::bail!("session {} not found", session_id.0);
                };
                session.push_message(MessageRole::Assistant, full_text_reply);
            }

            if is_tool_call && !tool_name.is_empty() {
                let args: Value = match serde_json::from_str(&tool_args_json) {
                    Ok(v) => v,
                    Err(_) => {
                        let Some(session) = self.sessions.get_mut(&session_id) else {
                            anyhow::bail!("session {} not found", session_id.0);
                        };
                        session.push_message(
                            MessageRole::Assistant,
                            format!(
                                "(尝试调用工具 {} 失败，生成的参数不是合法的 JSON)",
                                tool_name
                            ),
                        );
                        session.push_message(
                            MessageRole::User,
                            "你刚才尝试调用工具时生成的参数格式不正确或被截断了。请简化你的计划，确保输出完整的 JSON，然后重新调用工具。",
                        );
                        continue;
                    }
                };

                let approval_request = self.tool_policy.as_ref().and_then(|policy| {
                    policy.evaluate_approval(&tool_name, &args, &tool_args_json)
                });

                if let Some(request) = approval_request {
                    let approved = if let Some(action_key) = request.action_key.as_deref() {
                        self.cached_approval(session_id, action_key)
                    } else {
                        false
                    };

                    if !approved {
                        self.emit_event(AgentEvent::AwaitingApproval {
                            session_id,
                            request: request.clone(),
                        });
                        Self::drain_async_events(&mut event_rx, &mut on_event)?;

                        let decision = match self.approval_handler() {
                            Some(handler) => handler.approve(request.clone()).await?,
                            None => ApprovalDecision::Deny,
                        };

                        match decision {
                            ApprovalDecision::AllowOnce => {}
                            ApprovalDecision::AllowAlways => {
                                if let Some(action_key) = request.action_key.clone() {
                                    self.cache_approval(session_id, action_key);
                                }
                            }
                            ApprovalDecision::Deny => {
                                let denial_summary = format!(
                                    "[Action Denied]: 审批拒绝执行工具 {}。",
                                    tool_name
                                );
                                let Some(session) = self.sessions.get_mut(&session_id) else {
                                    anyhow::bail!("session {} not found", session_id.0);
                                };
                                session.push_assistant_tool_call(
                                    &tool_call_id,
                                    &tool_name,
                                    &tool_args_json,
                                );
                                session.push_tool_result(&tool_call_id, denial_summary.clone());
                                self.emit_event(AgentEvent::ToolCallFinished {
                                    session_id,
                                    tool_name: tool_name.clone(),
                                    summary: denial_summary,
                                });
                                Self::drain_async_events(&mut event_rx, &mut on_event)?;
                                continue;
                            }
                        }
                    }
                }

                let tool_ctx = ToolContext {
                    session_id,
                    event_bus: self.event_bus.clone(),
                };
                if let Some(policy) = self.tool_policy.as_ref() {
                    policy.on_pre_call(&tool_name, &args, &tool_ctx);
                }

                self.emit_event(AgentEvent::ToolCallStarted {
                    session_id,
                    tool_name: tool_name.clone(),
                    args_json: tool_args_json.clone(),
                });
                Self::drain_async_events(&mut event_rx, &mut on_event)?;

                {
                    let Some(session) = self.sessions.get_mut(&session_id) else {
                        anyhow::bail!("session {} not found", session_id.0);
                    };
                    session.push_assistant_tool_call(&tool_call_id, &tool_name, &tool_args_json);
                }

                let tool_result = if let Some(tool) = self.tools.get(&tool_name) {
                    tool.call(&args, &tool_ctx).await?
                } else {
                    ToolOutput {
                        summary: format!("Tool {} not found", tool_name),
                        raw: None,
                        control_flow: ToolControlFlow::Break,
                    }
                };

                if let Some(policy) = self.tool_policy.as_ref() {
                    policy.on_post_call(&tool_name, &args, &tool_result, &tool_ctx);
                }

                self.emit_event(AgentEvent::ToolCallFinished {
                    session_id,
                    tool_name: tool_name.clone(),
                    summary: tool_result.summary.clone(),
                });
                Self::drain_async_events(&mut event_rx, &mut on_event)?;

                {
                    let Some(session) = self.sessions.get_mut(&session_id) else {
                        anyhow::bail!("session {} not found", session_id.0);
                    };
                    session.push_tool_result(&tool_call_id, tool_result.summary);
                }

                match tool_result.control_flow {
                    ToolControlFlow::Continue => continue,
                    ToolControlFlow::Break => {
                        self.emit_event(AgentEvent::RunCompleted { session_id });
                        Self::drain_async_events(&mut event_rx, &mut on_event)?;
                        break;
                    }
                }
            }

            self.emit_event(AgentEvent::RunCompleted { session_id });
            Self::drain_async_events(&mut event_rx, &mut on_event)?;
            break;
        }

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
}
