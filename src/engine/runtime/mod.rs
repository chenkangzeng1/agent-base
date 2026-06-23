use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::engine::session_store::SessionStore;
use crate::engine::AgentSession;
use crate::types::{
    AgentConfig, AgentError, AgentResult, CheckpointData,
    MessageRole, RunOutcome, RuntimeEvent, SessionId, ExecutionPlan
};
use crate::engine::plan::PlanConfig;

use super::approval::ApprovalHandler;

mod event_bus;
pub(crate) use event_bus::EventBus;
mod llm_engine;
mod plan;
mod react_loop;
mod session_manager;
mod tool_engine;
mod plan_runner;

pub(super) const DEFAULT_MAX_TURNS: u32 = 50;

pub use llm_engine::LlmEngine;
pub use session_manager::SessionManager;
pub(crate) use tool_engine::ToolEngine;
pub(crate) use plan_runner::PlanRunner;

#[derive(Clone)]
pub struct AgentRuntime {
    pub(crate) runner: Arc<PlanRunner>,
}

impl AgentRuntime {
    pub async fn create_session(&self) -> SessionId {
        let config = self.runner.config.read().await;
        self.runner.session_manager.create_session(config.system_prompt.as_deref()).await
    }

    pub async fn restore_session(&self, session_id: &SessionId) -> Option<AgentSession> {
        self.runner.session_manager.restore_session(session_id).await
    }

    pub async fn session(&self, session_id: &SessionId) -> Option<AgentSession> {
        self.runner.session_manager.session(session_id).await
    }

    pub async fn session_or_err(&self, session_id: &SessionId) -> AgentResult<AgentSession> {
        self.runner.session_manager.session_or_err(session_id).await
    }

    pub async fn with_session_mut<F, R>(&self, session_id: &SessionId, f: F) -> AgentResult<R>
    where
        F: FnOnce(&mut AgentSession) -> R,
    {
        self.runner.session_manager.with_session_mut(session_id, f).await
    }

    pub(crate) fn emit_event(&self, event: RuntimeEvent) {
        self.runner.event_bus.emit(event);
    }

    pub(crate) fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<RuntimeEvent> {
        self.runner.event_bus.subscribe()
    }

    /// Subscribe to runtime events from the internal broadcast channel.
    ///
    /// Events are delivered directly from the runtime's event bus (capacity 2048).
    /// Slow consumers may receive `Lagged(n)` errors if they cannot keep up —
    /// ensure the receiver loop processes events promptly or use a buffering
    /// layer in the consumer if backpressure is a concern.
    pub fn subscribe_runtime_events(&self) -> tokio::sync::broadcast::Receiver<RuntimeEvent> {
        self.runner.event_bus.subscribe()
    }

    pub fn session_manager(&self) -> &SessionManager {
        &self.runner.session_manager
    }

    pub fn llm_engine(&self) -> &LlmEngine {
        &self.runner.llm_engine
    }

    pub fn client(&self) -> Arc<dyn crate::llm::LlmClient> {
        self.runner.llm_engine.get_client()
    }

    /// Replace the LLM client at runtime (e.g., model switch).
    /// Requires `&mut self` — obtain via `runtime.lock().await`.
    pub fn set_client(&mut self, client: Arc<dyn crate::llm::LlmClient>) {
        self.runner.llm_engine.set_client(client);
    }

    pub fn tools_mut(&self) -> Arc<tokio::sync::RwLock<crate::tool::ToolRegistry>> {
        self.runner.tool_engine.tools_arc()
    }

    pub async fn create_step_executor(&self) -> crate::engine::ToolCallingStepExecutor {
        use crate::engine::pipeline::DefaultPipeline;
        let tools_arc = self.runner.tool_engine.tools_arc();
        let registry = Arc::new(tools_arc.read().await.clone());
        let base = self.runner.tool_engine.execution_pipeline();
        let config = self.runner.config.read().await;
        let pipeline = DefaultPipeline::new(
            base.policy(),
            config.tool.tool_timeout_ms,
            config.tool.max_tool_output_chars,
        );
        crate::engine::ToolCallingStepExecutor::new(registry).with_pipeline(pipeline)
    }

    /// Inject the internal EventBus and PlanRunner into framework tools in the given registry.
    /// Call this after replacing tools in the registry (e.g., in `build_tools`).
    pub fn inject_framework_deps(&self, tools: &crate::tool::ToolRegistry) {
        self.runner.tool_engine.inject_event_bus_into(tools);
        tools.inject_plan_runner(&self.runner);
    }

    pub fn config(&self) -> tokio::sync::RwLockReadGuard<'_, AgentConfig> {
        self.runner.config.blocking_read()
    }

    /// 设置 reasoning effort（异步版本）
    pub async fn set_reasoning_effort(&self, effort: crate::llm::ReasoningEffort) {
        let mut config = self.runner.config.write().await;
        let mut reasoning = config.reasoning.take().unwrap_or_default();
        reasoning.effort = Some(effort);
        config.reasoning = Some(reasoning);
    }

    /// 设置 reasoning effort（同步版本，只在同步上下文中使用）
    pub fn set_reasoning_effort_sync(&self, effort: crate::llm::ReasoningEffort) {
        let mut config = self.runner.config.blocking_write();
        let mut reasoning = config.reasoning.take().unwrap_or_default();
        reasoning.effort = Some(effort);
        config.reasoning = Some(reasoning);
    }

    pub fn approval_handler(&self) -> Option<&Arc<dyn ApprovalHandler>> {
        self.runner.tool_engine.approval_handler()
    }

    pub async fn cached_approval(&self, session_id: &SessionId, action_key: &str) -> bool {
        self.runner.session_manager.cached_approval(session_id, action_key).await
    }

    pub async fn cache_approval(&self, session_id: &SessionId, action_key: String) {
        self.runner.session_manager.cache_approval(session_id, action_key).await
    }

    pub async fn save_checkpoint(&self, session_id: &SessionId, checkpoint: CheckpointData) -> AgentResult<()> {
        self.emit_event(RuntimeEvent::Checkpoint {
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
        on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        self.runner.resume_from_checkpoint(checkpoint, on_event).await
    }

    pub async fn run<F>(
        &self,
        session_id: SessionId,
        on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        self.runner.run(session_id, on_event).await
    }

    pub async fn run_turn<F>(
        &self,
        session_id: SessionId,
        user_input: &str,
        on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        self.runner.run_turn(session_id, user_input, on_event).await
    }

    pub async fn run_turn_collect(
        &self,
        session_id: SessionId,
        user_input: &str,
    ) -> AgentResult<(Vec<RuntimeEvent>, RunOutcome)> {
        self.runner.run_turn_collect(session_id, user_input).await
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
        if self.runner.session_manager.session(session_id).await.is_none() {
            return Err(AgentError::session_not_found(session_id.id));
        }
        Ok(())
    }

    pub fn session_store(&self) -> Arc<dyn SessionStore> {
        self.runner.session_manager.session_store().clone()
    }

    pub async fn run_plan<F>(
        &self,
        session_id: SessionId,
        plan: ExecutionPlan,
        config: PlanConfig,
        on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        self.runner.run_plan(session_id, plan, config, on_event).await
    }

    pub async fn run_plan_with_generator<F>(
        &self,
        session_id: SessionId,
        objective: &str,
        generator: Arc<dyn crate::engine::PlanGenerator>,
        config: PlanConfig,
        on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        self.runner.run_plan_with_generator(session_id, objective, generator, config, on_event).await
    }

    /// Generate an execution plan with real-time streaming events.
    ///
    /// Unlike `run_plan_with_generator` which buffers plan generation events and
    /// emits `PlanGenerated` only at completion, this method forwards plan generation
    /// progress (`PlanGenerating`, `PlanStepParsed`, `ThoughtDelta`) to the callback
    /// as they arrive from the generator.
    pub async fn generate_plan_streaming<F>(
        &self,
        session_id: &SessionId,
        objective: &str,
        context: &str,
        generator: Arc<dyn crate::engine::PlanGenerator>,
        on_event: &mut F,
    ) -> AgentResult<ExecutionPlan>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let (plan_event_tx, mut plan_event_rx) = tokio::sync::mpsc::unbounded_channel();
        let tools = self.runner.tool_engine.definitions().await;

        let plan_future = generator.generate_plan(objective, context, &tools, Some(plan_event_tx));
        tokio::pin!(plan_future);

        // Stream events in real-time while the plan is being generated
        let mut plan = loop {
            tokio::select! {
                result = &mut plan_future => {
                    break match result {
                        Ok(plan) => plan,
                        Err(e) => return Err(e),
                    };
                }
                Some(event) = plan_event_rx.recv() => {
                    let runtime_event = match event {
                        RuntimeEvent::PlanGenerating { .. } => RuntimeEvent::PlanGenerating {
                            session_id: session_id.clone(),
                            plan_id: String::new(),
                        },
                        RuntimeEvent::PlanStepParsed { step_index, step_id, step_description, .. } => {
                            RuntimeEvent::PlanStepParsed {
                                session_id: session_id.clone(),
                                plan_id: String::new(),
                                step_index,
                                step_id,
                                step_description,
                            }
                        }
                        RuntimeEvent::ThoughtDelta { text, .. } => RuntimeEvent::ThoughtDelta {
                            session_id: session_id.clone(),
                            text,
                        },
                        other => other,
                    };
                    on_event(runtime_event)?;
                }
            }
        };

        // Drain any remaining events
        while let Ok(event) = plan_event_rx.try_recv() {
            let runtime_event = match event {
                RuntimeEvent::PlanGenerating { .. } => RuntimeEvent::PlanGenerating {
                    session_id: session_id.clone(),
                    plan_id: plan.id.clone(),
                },
                RuntimeEvent::PlanStepParsed { step_index, step_id, step_description, .. } => {
                    RuntimeEvent::PlanStepParsed {
                        session_id: session_id.clone(),
                        plan_id: plan.id.clone(),
                        step_index,
                        step_id,
                        step_description,
                    }
                }
                RuntimeEvent::ThoughtDelta { text, .. } => RuntimeEvent::ThoughtDelta {
                    session_id: session_id.clone(),
                    text,
                },
                other => other,
            };
            on_event(runtime_event)?;
        }

        // Emit final PlanGenerated
        on_event(RuntimeEvent::PlanGenerated {
            session_id: session_id.clone(),
            plan: plan.clone(),
        })?;

        Ok(plan)
    }

    // --- Cancellation support ---

    /// Cancel the currently executing run_turn / run / run_plan.
    /// No-op if there is no current execution.
    pub fn cancel(&self) {
        self.runner.cancel();
    }

    /// Reset the cancel token (called automatically before each run_turn)
    pub fn reset_cancel(&self) {
        self.runner.reset_cancel();
    }

    /// Get a clone of the cancel token
    pub fn cancel_token(&self) -> CancellationToken {
        self.runner.cancel_token()
    }

    /// Check if cancellation has been requested
    pub fn is_cancelled(&self) -> bool {
        self.runner.is_cancelled()
    }
}
