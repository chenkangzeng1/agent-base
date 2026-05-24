use std::sync::Arc;

use crate::engine::context::ContextWindowManager;
use crate::engine::middleware::MiddlewareRef;
use crate::engine::session_store::SessionStore;
use crate::engine::AgentSession;
use crate::types::{AgentConfig, AgentError, AgentEvent, AgentResult, CheckpointData, CheckpointStep, MessageRole, RunOutcome, SessionId};

use super::approval::ApprovalHandler;

mod event_bus;
mod llm_engine;
mod plan;
mod react_loop;
mod session_manager;
mod tool_engine;

pub(super) const DEFAULT_MAX_TURNS: u32 = 50;

pub use event_bus::EventBus;
pub use llm_engine::LlmEngine;
pub use session_manager::SessionManager;
pub use tool_engine::ToolEngine;

pub struct AgentRuntime {
    pub(crate) config: AgentConfig,
    pub(crate) llm_engine: LlmEngine,
    pub(crate) tool_engine: ToolEngine,
    pub(crate) session_manager: SessionManager,
    pub(crate) event_bus: EventBus,
    pub(crate) context_manager: Option<ContextWindowManager>,
    pub(crate) middlewares: Vec<MiddlewareRef>,
}

impl AgentRuntime {
    pub async fn create_session(&self) -> SessionId {
        self.session_manager.create_session(self.config.system_prompt.as_deref()).await
    }

    pub async fn restore_session(&self, session_id: &SessionId) -> Option<AgentSession> {
        self.session_manager.restore_session(session_id).await
    }

    pub async fn session(&self, session_id: &SessionId) -> Option<AgentSession> {
        self.session_manager.session(session_id).await
    }

    pub async fn session_or_err(&self, session_id: &SessionId) -> AgentResult<AgentSession> {
        self.session_manager.session_or_err(session_id).await
    }

    pub async fn with_session_mut<F, R>(&self, session_id: &SessionId, f: F) -> AgentResult<R>
    where
        F: FnOnce(&mut AgentSession) -> R,
    {
        self.session_manager.with_session_mut(session_id, f).await
    }

    pub fn emit_event(&self, event: AgentEvent) {
        self.event_bus.emit(event);
    }

    pub fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<AgentEvent> {
        self.event_bus.subscribe()
    }

    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    pub fn session_manager(&self) -> &SessionManager {
        &self.session_manager
    }

    pub fn llm_engine(&self) -> &LlmEngine {
        &self.llm_engine
    }

    pub fn tool_engine(&self) -> &ToolEngine {
        &self.tool_engine
    }

    pub fn tool_engine_mut(&mut self) -> &mut ToolEngine {
        &mut self.tool_engine
    }

    pub fn client(&self) -> Arc<dyn crate::llm::LlmClient> {
        self.llm_engine.client.clone()
    }

    pub fn tools_mut(&mut self) -> &mut crate::tool::ToolRegistry {
        self.tool_engine.tools_mut()
    }

    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    pub fn approval_handler(&self) -> Option<&Arc<dyn ApprovalHandler>> {
        self.tool_engine.approval_handler()
    }

    pub async fn cached_approval(&self, session_id: &SessionId, action_key: &str) -> bool {
        self.session_manager.cached_approval(session_id, action_key).await
    }

    pub async fn cache_approval(&self, session_id: &SessionId, action_key: String) {
        self.session_manager.cache_approval(session_id, action_key).await
    }

    pub async fn save_checkpoint(&self, session_id: &SessionId, checkpoint: CheckpointData) -> AgentResult<()> {
        self.emit_event(AgentEvent::Checkpoint {
            session_id: session_id.clone(),
            checkpoint,
        });
        Ok(())
    }

    pub async fn load_checkpoint(&self, _session_id: &SessionId, _checkpoint: &CheckpointData) -> AgentResult<Option<CheckpointData>> {
        Ok(None)
    }

    pub async fn resume_from_checkpoint<F>(
        &self,
        checkpoint: CheckpointData,
        mut on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(AgentEvent) -> AgentResult<()> + Send,
    {
        let session_id = checkpoint.session_id.clone();
        let user_input = checkpoint.user_input.clone();
        let turn_count = checkpoint.turn_count;

        tracing::info!(session_id = session_id.id, turn_count, step = ?checkpoint.step, "resuming from checkpoint");

        let mut event_rx = self.subscribe_events();
        let tool_definitions = self.tool_engine.definitions();

        if let CheckpointStep::BeforeToolCalls { tool_calls } = checkpoint.step {
            match self.handle_tool_calls(&session_id, &tool_calls, &mut event_rx, &mut on_event).await {
                Ok(react_loop::ToolCallResult::Continue) => {}
                Ok(react_loop::ToolCallResult::Break) => {
                    self.emit_event(AgentEvent::RunFinished { session_id: session_id.clone() });
                    EventBus::drain_async_events(&mut event_rx, &mut on_event)?;
                    return Ok(RunOutcome::Completed);
                }
                Err(e) => {
                    if let Some(outcome) = self
                        .handle_tool_error(&session_id, &tool_calls, e, &mut event_rx, &mut on_event)
                        .await?
                    {
                        return Ok(outcome);
                    }
                }
            }
        }

        let (outcome, _final_turn_count) = self
            .run_turn_loop(
                &session_id,
                &user_input,
                &tool_definitions,
                turn_count,
                &mut event_rx,
                &mut on_event,
            )
            .await?;

        Ok(outcome)
    }

    pub async fn run<F>(
        &self,
        session_id: SessionId,
        mut on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(AgentEvent) -> AgentResult<()> + Send,
    {
        let span = tracing::info_span!("agent_run", session_id = session_id.id);
        let _enter = span.enter();

        let mut event_rx = self.subscribe_events();

        if let Err(e) = self.validate_session(&session_id).await {
            self.emit_event(AgentEvent::RunFinished { session_id: session_id.clone() });
            EventBus::drain_async_events(&mut event_rx, &mut on_event)?;
            return Err(e);
        }

        let tool_definitions = self.tool_engine.definitions();
        let user_input_owned = self.with_session_mut(&session_id, |session| {
            session.chat_messages().last()
                .and_then(|m| match m {
                    crate::types::ChatMessage::User { content, .. } => Some(content.clone()),
                    _ => None,
                })
                .unwrap_or_default()
        }).await?;

        let (outcome, _turn_count) = self
            .run_turn_loop(
                &session_id,
                &user_input_owned,
                &tool_definitions,
                0,
                &mut event_rx,
                &mut on_event,
            )
            .await?;

        self.emit_event(AgentEvent::RunFinished { session_id: session_id.clone() });
        EventBus::drain_async_events(&mut event_rx, &mut on_event)?;

        Ok(outcome)
    }

    pub async fn run_turn_with_handler<F>(
        &self,
        session_id: SessionId,
        user_input: &str,
        mut on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(AgentEvent) -> AgentResult<()> + Send,
    {
        let span = tracing::Span::current();
        let _guard = span.enter();
        tracing::info!(session_id = session_id.id, user_input = %user_input, "agent turn start");
        drop(_guard);

        let mut event_rx = self.subscribe_events();
        let tool_definitions = self.tool_engine.definitions();

        let user_input_owned = self.apply_user_message_mw(&session_id, user_input.to_string()).await?;

        self.with_session_mut(&session_id, |session| {
            session.push_message(MessageRole::User, &user_input_owned);
        }).await?;

        self.emit_event(AgentEvent::Checkpoint {
            session_id: session_id.clone(),
            checkpoint: CheckpointData {
                session_id: session_id.clone(),
                user_input: user_input_owned.clone(),
                step: CheckpointStep::AfterUserInput,
                turn_count: 0,
            },
        });

        let (outcome, turn_count) = self
            .run_turn_loop(
                &session_id,
                &user_input_owned,
                &tool_definitions,
                0,
                &mut event_rx,
                &mut on_event,
            )
            .await?;

        tracing::info!(session_id = session_id.id, turn_count, "agent turn completed");
        Ok(outcome)
    }

    pub async fn run_turn_stream(
        &self,
        session_id: SessionId,
        user_input: &str,
    ) -> AgentResult<(Vec<AgentEvent>, RunOutcome)> {
        let mut events = Vec::new();
        let outcome = self.run_turn_with_handler(session_id, user_input, |event| {
            events.push(event);
            Ok(())
        })
        .await?;
        Ok((events, outcome))
    }

    pub async fn add_user_message(&self, session_id: &SessionId, text: impl Into<String>) -> AgentResult<()> {
        let text = text.into();
        self.with_session_mut(session_id, |session| {
            session.push_message(MessageRole::User, &text);
        }).await
    }

    pub async fn add_system_message(&self, session_id: &SessionId, text: impl Into<String>) -> AgentResult<()> {
        let text = text.into();
        self.with_session_mut(session_id, |session| {
            session.push_message(MessageRole::System, &text);
        }).await
    }

    pub async fn add_tool_result(&self, session_id: &SessionId, tool_call_id: &str, summary: impl Into<String>) -> AgentResult<()> {
        let summary = summary.into();
        self.with_session_mut(session_id, |session| {
            session.push_tool_result(tool_call_id, summary.clone());
        }).await
    }

    pub async fn get_messages(&self, session_id: &SessionId) -> AgentResult<Vec<crate::types::ChatMessage>> {
        let session = self.session_or_err(session_id).await?;
        Ok(session.chat_messages().to_vec())
    }

    pub async fn validate_session(&self, session_id: &SessionId) -> AgentResult<()> {
        if self.session_manager.session(session_id).await.is_none() {
            return Err(AgentError::session_not_found(session_id.id));
        }
        Ok(())
    }

    pub fn session_store(&self) -> Arc<dyn SessionStore> {
        self.session_manager.session_store().clone()
    }
}
