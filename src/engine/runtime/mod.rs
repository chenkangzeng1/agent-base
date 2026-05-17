use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::llm::LlmClient;
use crate::skill::{Skill, SkillPrompter};
use crate::tool::{ToolPolicy, ToolRegistry};
use crate::types::{AgentConfig, MessageRole, AgentError, CheckpointData, CheckpointStep};
use tokio::sync::broadcast;
use tracing::Span;

use crate::types::{AgentResult, AgentEvent, SessionId, RunOutcome};
use super::approval::ApprovalHandler;
use super::context::ContextWindowManager;
use super::middleware::MiddlewareRef;
use super::recovery::ToolErrorRecovery;
use super::session_store::SessionStore;
use super::AgentSession;

mod approval_flow;
mod llm;
mod tool_exec;
mod react_loop;

use tool_exec::ToolCallResult;

pub(super) const DEFAULT_MAX_TURNS: u32 = 50;

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
    pub(crate) skills: Vec<Arc<dyn Skill>>,
    #[allow(dead_code)]
    pub(crate) skill_prompter: Arc<dyn SkillPrompter>,
    pub(crate) error_recovery: Arc<dyn ToolErrorRecovery>,
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

    /// Restore an existing session from persistence into runtime memory
    ///
    /// On success, the session can be used for continued execution.
    /// Returns None if not found in persistence layer.
    pub async fn restore_session(&mut self, session_id: &SessionId) -> Option<&AgentSession> {
        if self.sessions.contains_key(session_id) {
            return self.sessions.get(session_id);
        }
        match self.session_store.load(session_id).await {
            Ok(Some(session)) => {
                self.sessions.insert(session_id.clone(), session);
                self.sessions.get(session_id)
            }
            _ => None,
        }
    }

    pub fn session(&self, session_id: &SessionId) -> Option<&AgentSession> {
        self.sessions.get(session_id)
    }

    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    pub fn tools_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tools
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

    pub fn skills(&self) -> &[Arc<dyn Skill>] {
        &self.skills
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
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let span = Span::current();
        let _guard = span.enter();
        tracing::info!(session_id = session_id.id, user_input = %user_input, "agent turn start");
        drop(_guard);

        let mut event_rx = self.subscribe_events();
        let tool_definitions = self.tools.definitions();

        let user_input_owned = self.apply_user_message_mw(&session_id, user_input.to_string()).await?;

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
        &mut self,
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

    pub async fn resume_from_checkpoint<F>(
        &mut self,
        checkpoint: CheckpointData,
        mut on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let session_id = checkpoint.session_id;
        let user_input = checkpoint.user_input;
        let turn_count = checkpoint.turn_count;

        tracing::info!(session_id = session_id.id, turn_count, step = ?checkpoint.step, "resuming from checkpoint");

        let mut event_rx = self.subscribe_events();
        let tool_definitions = self.tools.definitions();

        if let CheckpointStep::BeforeToolCalls { tool_calls } = checkpoint.step {
            match self
                .handle_tool_calls(&session_id, &tool_calls, &mut event_rx, &mut on_event)
                .await
            {
                Ok(ToolCallResult::Continue) => {}
                Ok(ToolCallResult::Break) => {
                    self.emit_event(AgentEvent::RunFinished { session_id: session_id.clone() });
                    Self::drain_async_events(&mut event_rx, &mut on_event)?;
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

        let (outcome, final_turn_count) = self
            .run_turn_loop(
                &session_id,
                &user_input,
                &tool_definitions,
                turn_count,
                &mut event_rx,
                &mut on_event,
            )
            .await?;

        tracing::info!(session_id = session_id.id, turn_count = final_turn_count, "agent resume completed");
        Ok(outcome)
    }

}
