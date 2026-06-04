use std::sync::Arc;

use crate::engine::context::ContextWindowManager;
use crate::engine::middleware::MiddlewareRef;
use crate::engine::session_store::SessionStore;
use crate::engine::AgentSession;
use crate::types::{AgentConfig, AgentError, AgentEvent, AgentResult, CheckpointData, CheckpointStep, MessageRole, RunOutcome, RuntimeEvent, SessionId};

use super::approval::ApprovalHandler;

mod event_bus;
pub(crate) use event_bus::EventBus;
mod llm_engine;
mod plan;
mod react_loop;
mod session_manager;
mod tool_engine;

pub(super) const DEFAULT_MAX_TURNS: u32 = 50;

pub use llm_engine::LlmEngine;
pub use session_manager::SessionManager;
pub(crate) use tool_engine::ToolEngine;

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

    pub(crate) fn emit_event(&self, event: AgentEvent) {
        self.event_bus.emit(event);
    }

    pub(crate) fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<AgentEvent> {
        self.event_bus.subscribe()
    }

    /// Subscribe to runtime events as `RuntimeEvent`.
    ///
    /// This is the public API for external consumers (frontends, CLIs) to
    /// receive events from the agent runtime.
    pub fn subscribe_runtime_events(&self) -> tokio::sync::broadcast::Receiver<RuntimeEvent> {
        let (tx, rx) = tokio::sync::broadcast::channel(256);
        let mut internal_rx = self.event_bus.subscribe();
        tokio::spawn(async move {
            while let Ok(event) = internal_rx.recv().await {
                let _ = tx.send(RuntimeEvent::from(event));
            }
        });
        rx
    }

    pub fn session_manager(&self) -> &SessionManager {
        &self.session_manager
    }

    pub fn llm_engine(&self) -> &LlmEngine {
        &self.llm_engine
    }

    pub fn client(&self) -> Arc<dyn crate::llm::LlmClient> {
        self.llm_engine.client.clone()
    }

    pub fn tools_mut(&mut self) -> &mut crate::tool::ToolRegistry {
        self.tool_engine.tools_mut()
    }

    /// Create a `ToolCallingStepExecutor` with the engine's execution pipeline.
    ///
    /// Plan steps executed through this executor get the same guarantees as
    /// direct tool calls: policy hooks, timeout (`config.tool.tool_timeout_ms`),
    /// and output truncation (`config.tool.max_tool_output_chars`).
    ///
    /// ```rust,no_run
    /// # use std::sync::Arc;
    /// # use agent_base::engine::{PlanExecTool, PlanStore, InMemoryPlanStore, AbortOnFailure};
    /// # fn example(runtime: agent_base::engine::AgentRuntime) {
    /// let step_executor = Arc::new(runtime.create_step_executor());
    /// let plan_store = Arc::new(InMemoryPlanStore::new()) as Arc<dyn PlanStore>;
    /// let recovery = Arc::new(AbortOnFailure);
    /// let exec_tool = PlanExecTool::new(step_executor, plan_store, recovery);
    /// # }
    /// ```
    pub fn create_step_executor(&self) -> crate::engine::ToolCallingStepExecutor {
        use crate::engine::pipeline::DefaultPipeline;
        let registry = Arc::new(self.tool_engine.tools().clone());
        let base = self.tool_engine.execution_pipeline();
        let pipeline = DefaultPipeline::new(
            base.policy(),
            self.config.tool.tool_timeout_ms,
            self.config.tool.max_tool_output_chars,
        );
        crate::engine::ToolCallingStepExecutor::new(registry).with_pipeline(pipeline)
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
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
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
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let span = tracing::info_span!("agent_run", session_id = session_id.id);
        let _enter = span.enter();

        let mut event_rx = self.subscribe_events();

        if let Err(e) = self.validate_session(&session_id).await {
            tracing::warn!(session_id = session_id.id, error = %e, "session validation failed");
            self.emit_event(AgentEvent::RunFinished { session_id: session_id.clone() });
            EventBus::drain_async_events(&mut event_rx, &mut on_event)?;
            return Err(e);
        }

        let tool_definitions = self.tool_engine.definitions();
        tracing::debug!(session_id = session_id.id, tool_count = tool_definitions.len(), "agent run start");
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
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
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
    ) -> AgentResult<(Vec<RuntimeEvent>, RunOutcome)> {
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
