use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{broadcast, mpsc, RwLock};

use crate::engine::approval::ApprovalHandler;
use crate::engine::pipeline::{DefaultPipeline, ToolExecutionPipeline};
use crate::engine::recovery::ToolErrorRecovery;
use crate::engine::runtime::event_bus::EventBus;
use crate::engine::runtime::session_manager::SessionManager;
use crate::tool::{ToolContext, ToolControlFlow, ToolOutput, ToolPolicy, ToolRegistry};
use crate::types::{AgentError, AgentEvent, AgentResult, Language, RuntimeEvent, SessionId, UserEvent};

pub(crate) struct ToolEngine {
    tools: Arc<RwLock<ToolRegistry>>,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
    tool_policy: Option<Arc<dyn ToolPolicy>>,
    error_recovery: Arc<dyn ToolErrorRecovery>,
    event_bus: EventBus,
    pipeline: DefaultPipeline,
}

impl ToolEngine {
    pub fn new(
        tools: ToolRegistry,
        approval_handler: Option<Arc<dyn ApprovalHandler>>,
        tool_policy: Option<Arc<dyn ToolPolicy>>,
        error_recovery: Arc<dyn ToolErrorRecovery>,
        event_bus: EventBus,
    ) -> Self {
        let pipeline = DefaultPipeline::new(tool_policy.clone(), None, None);
        Self {
            tools: Arc::new(RwLock::new(tools)),
            approval_handler,
            tool_policy,
            error_recovery,
            event_bus,
            pipeline,
        }
    }

    /// Inject PlanRunner synchronously during build (before tokio runtime).
    pub fn inject_plan_runner_sync(&self, runner: &Arc<crate::engine::PlanRunner>) {
        // During build, we are the only holder of the Arc<RwLock<>>, so try_write is safe.
        let mut tools = self.tools.try_write()
            .expect("inject_plan_runner_sync: failed to acquire write lock");
        tools.inject_plan_runner(runner);
    }

    pub async fn definitions(&self) -> Vec<Value> {
        self.tools.read().await.definitions()
    }

    /// Get the inner pipeline (policy hooks only, no timeout/truncation).
    ///
    /// Used by [`AgentRuntime::create_step_executor`] to construct a per-call
    /// pipeline that inherits the policy but adds timeout/truncation from config.
    pub fn execution_pipeline(&self) -> DefaultPipeline {
        self.pipeline.clone()
    }

    pub async fn execute_tool<F>(
        &self,
        session_id: &SessionId,
        id: &str,
        name: &str,
        args: &Value,
        tool_args_json: &str,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
        session_manager: &SessionManager,
        llm_client: Option<Arc<dyn crate::llm::LlmClient>>,
        language: Language,
        tool_timeout_ms: Option<u64>,
        max_output_chars: Option<usize>,
    ) -> AgentResult<ToolExecutionResult>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        tracing::debug!(session_id = session_id.id, tool = name, args_len = tool_args_json.len(), "execute tool start");

        // Emit ToolCallStarted via internal EventBus
        self.event_bus.emit(AgentEvent::ToolCallStarted {
            session_id: session_id.clone(),
            tool_name: name.to_string(),
            args_json: tool_args_json.to_string(),
        });
        EventBus::drain_async_events(event_rx, on_event)?;

        // Build ToolContext with UserEvent channel for tool-produced events
        let (user_event_tx, mut user_event_rx) = mpsc::unbounded_channel::<UserEvent>();
        let tool_context = ToolContext {
            session_id: session_id.clone(),
            user_event_tx,
            llm_client: llm_client.clone(),
            session_store: Some(session_manager.session_store().clone()),
            language: language.clone(),
        };

        // Lookup tool and execute via pipeline.
        // The pipeline handles: before_call hook → timeout → truncation → after_call hook.
        // ToolEngine handles: event emission and UserEvent forwarding.
        tracing::debug!(session_id = session_id.id, tool = name, "looking up tool in registry");
        let tools_guard = self.tools.read().await;
        let tool_result = match tools_guard.get(name) {
            Some(tool) => {
                tracing::debug!(session_id = session_id.id, tool = name, "tool found, executing via pipeline");

                // Per-call pipeline: inherits policy from self.pipeline, adds caller's timeout/truncation.
                let pipeline = DefaultPipeline::new(
                    self.pipeline.policy(),
                    tool_timeout_ms,
                    max_output_chars,
                );

                let future = pipeline.execute(tool.as_ref(), args, &tool_context);
                tokio::pin!(future);

                // Execute with UserEvent forwarding interleaved via tokio::select!
                let output = loop {
                    tokio::select! {
                        result = &mut future => break result,
                        Some(user_event) = user_event_rx.recv() => {
                            on_event(RuntimeEvent::UserEvent {
                                session_id: session_id.clone(),
                                event: user_event,
                            })?;
                        }
                    }
                };

                // Drain remaining UserEvents after tool completes
                while let Ok(user_event) = user_event_rx.try_recv() {
                    on_event(RuntimeEvent::UserEvent {
                        session_id: session_id.clone(),
                        event: user_event,
                    })?;
                }

                match output {
                    Ok(output) => output,
                    Err(e) => {
                        tracing::error!(session_id = session_id.id, tool_name = name, error = %e, "Tool execution failed");
                        return Err(AgentError::ToolExecution {
                            name: name.to_string(),
                            source: Box::new(e),
                        });
                    }
                }
            }
            None => {
                tracing::warn!(session_id = session_id.id, tool = name, "tool not found in registry");
                ToolOutput {
                    summary: format!("Tool {} not found", name),
                    raw: None,
                    control_flow: ToolControlFlow::Break,
                    truncation: None,
                }
            }
        };

        // Emit ToolCallFinished via internal EventBus
        self.event_bus.emit(AgentEvent::ToolCallFinished {
            session_id: session_id.clone(),
            tool_name: name.to_string(),
            summary: tool_result.summary.clone(),
        });
        EventBus::drain_async_events(event_rx, on_event)?;

        Ok(ToolExecutionResult {
            id: id.to_string(),
            name: name.to_string(),
            output: tool_result,
        })
    }

    pub async fn process_approval<F>(
        &self,
        session_id: &SessionId,
        tool_name: &str,
        args: &Value,
        tool_args_json: &str,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
        session_manager: &SessionManager,
    ) -> AgentResult<()>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let approval_request = match self.tool_policy.as_ref() {
            Some(policy) => policy.evaluate_approval(tool_name, args).await,
            None => None,
        };

        let Some(request) = approval_request else {
            return Ok(());
        };

        let approved = if let Some(key) = request.action_key.as_deref() {
            session_manager.cached_approval(session_id, key).await
        } else {
            false
        };

        if approved {
            tracing::debug!(session_id = session_id.id, tool = tool_name, "approval cached, skipping");
            return Ok(());
        }

        tracing::debug!(session_id = session_id.id, tool = tool_name, risk = ?request.risk_level, "requesting approval");

        self.event_bus.emit(AgentEvent::AwaitingApproval {
            session_id: session_id.clone(),
            request: request.clone(),
        });
        EventBus::drain_async_events(event_rx, on_event)?;

        let decision = match self.approval_handler.as_ref() {
            Some(handler) => {
                let timeout = std::time::Duration::from_secs(300);
                match tokio::time::timeout(timeout, handler.approve(request.clone())).await {
                    Ok(result) => result.map_err(|e| AgentError::internal(format!("Approval handler failed: {e}")))?,
                    Err(_) => {
                        tracing::warn!(session_id = session_id.id, ?timeout, "Approval timed out, defaulting to Deny");
                        crate::types::ApprovalDecision::Deny
                    }
                }
            }
            None => crate::types::ApprovalDecision::Deny,
        };

        match decision {
            crate::types::ApprovalDecision::AllowOnce => {}
            crate::types::ApprovalDecision::AllowAlways => {
                if let Some(action_key) = request.action_key.clone() {
                    session_manager.cache_approval(session_id, action_key).await;
                }
            }
            crate::types::ApprovalDecision::Deny => {
                let denial_summary = format!("[Action Denied]: tool {} rejected by approval", tool_name);
                session_manager.with_session_mut(session_id, |session| {
                    session.push_assistant_tool_call("", tool_name, tool_args_json);
                    session.push_tool_result("", denial_summary.clone());
                }).await?;
                self.event_bus.emit(AgentEvent::ToolCallFinished {
                    session_id: session_id.clone(),
                    tool_name: tool_name.to_string(),
                    summary: denial_summary,
                });
                EventBus::drain_async_events(event_rx, on_event)?;
                return Err(AgentError::ApprovalDenied {
                    tool_name: tool_name.to_string(),
                });
            }
        }

        Ok(())
    }

    pub fn error_recovery(&self) -> &Arc<dyn ToolErrorRecovery> {
        &self.error_recovery
    }

    pub fn approval_handler(&self) -> Option<&Arc<dyn ApprovalHandler>> {
        self.approval_handler.as_ref()
    }

    pub fn tools_arc(&self) -> Arc<RwLock<ToolRegistry>> {
        self.tools.clone()
    }
}

pub struct ToolExecutionResult {
    pub id: String,
    pub name: String,
    pub output: ToolOutput,
}
