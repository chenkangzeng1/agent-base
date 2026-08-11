use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{RwLock, broadcast, mpsc};

use crate::engine::approval::ApprovalHandler;
use crate::engine::pipeline::{DefaultPipeline, ToolExecutionPipeline};
use crate::engine::recovery::ToolErrorRecovery;
use crate::engine::runtime::event_bus::EventBus;
use crate::engine::runtime::session_manager::SessionManager;
use crate::tool::{ToolContext, ToolControlFlow, ToolOutput, ToolPolicy, ToolRegistry};
use crate::types::{AgentError, AgentResult, Language, RuntimeEvent, SessionId, UserEvent};

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

    pub async fn definitions(&self) -> Vec<Value> {
        self.tools.read().await.definitions()
    }

    /// Inject EventBus into framework tools in an external ToolRegistry.
    pub fn inject_event_bus_into(&self, tools: &crate::tool::ToolRegistry) {
        tools.inject_event_bus(&self.event_bus);
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn execute_tool<F>(
        &self,
        session_id: &SessionId,
        id: &str,
        name: &str,
        args: &Value,
        tool_args_json: &str,
        ctx: &ExecutionContext,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: &mut F,
    ) -> AgentResult<ToolExecutionResult>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        tracing::debug!(
            session_id = session_id.id,
            tool = name,
            args_len = tool_args_json.len(),
            "execute tool start"
        );

        // Emit ToolCallStarted via internal EventBus
        self.event_bus.emit(RuntimeEvent::ToolCallStarted {
            session_id: session_id.clone(),
            tool_name: name.to_string(),
            args_json: tool_args_json.to_string(),
            agent_id: None,
            trace_id: None,
        });
        EventBus::drain_async_events(event_rx, on_event)?;

        // Build ToolContext with UserEvent channel for tool-produced events
        let (user_event_tx, mut user_event_rx) = mpsc::unbounded_channel::<UserEvent>();
        let tool_context = ToolContext {
            session_id: session_id.clone(),
            user_event_tx,
            llm_client: ctx.llm_client.clone(),
            session_store: Some(ctx.session_manager.session_store().clone()),
            language: ctx.language.clone(),
            cancel_token: ctx.cancel_token.clone(),
        };

        // Lookup tool and execute via pipeline.
        // The pipeline handles: before_call hook → timeout → truncation → after_call hook.
        // ToolEngine handles: event emission and UserEvent forwarding.
        tracing::debug!(
            session_id = session_id.id,
            tool = name,
            "looking up tool in registry"
        );
        let tools_guard = self.tools.read().await;
        let tool_result = match tools_guard.get(name) {
            Some(tool) => {
                tracing::debug!(
                    session_id = session_id.id,
                    tool = name,
                    "tool found, executing via pipeline"
                );

                // Per-call pipeline: inherits policy from self.pipeline, adds caller's timeout/truncation.
                let pipeline = DefaultPipeline::new(
                    self.pipeline.policy(),
                    ctx.tool_timeout_ms,
                    ctx.max_output_chars,
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
                                agent_id: None,
                                trace_id: None,
                            })?;
                        }
                        _ = ctx.cancel_token.cancelled() => {
                            tracing::info!(session_id = session_id.id, tool = name, "tool execution cancelled");
                            return Err(crate::types::AgentError::Cancelled);
                        }
                    }
                };

                // Drain remaining UserEvents after tool completes
                while let Ok(user_event) = user_event_rx.try_recv() {
                    on_event(RuntimeEvent::UserEvent {
                        session_id: session_id.clone(),
                        event: user_event,
                        agent_id: None,
                        trace_id: None,
                    })?;
                }

                match output {
                    Ok(output) => output,
                    Err(e) => {
                        tracing::error!(session_id = session_id.id, tool_name = name, error = %e, "Tool execution failed");
                        // Emit ToolCallFinished with error summary before returning error
                        let error_summary = if ctx.language == Language::Zh {
                            format!("❌ 执行失败: {}", e)
                        } else {
                            format!("❌ Tool execution failed: {}", e)
                        };
                        self.event_bus.emit(RuntimeEvent::ToolCallFinished {
                            session_id: session_id.clone(),
                            tool_name: name.to_string(),
                            summary: error_summary,
                            agent_id: None,
                            trace_id: None,
                        });
                        // Use `let _ =` to avoid masking the original tool error
                        // if the event callback fails
                        let _ = EventBus::drain_async_events(event_rx, on_event);
                        return Err(AgentError::ToolExecution {
                            name: name.to_string(),
                            source: Box::new(e),
                        });
                    }
                }
            }
            None => {
                tracing::warn!(
                    session_id = session_id.id,
                    tool = name,
                    "tool not found in registry"
                );
                ToolOutput {
                    summary: if ctx.language == Language::Zh {
                        format!("工具 {} 未找到", name)
                    } else {
                        format!("Tool {} not found", name)
                    },
                    raw: None,
                    control_flow: ToolControlFlow::Break,
                    truncation: None,
                }
            }
        };

        // Emit ToolCallFinished via internal EventBus
        self.event_bus.emit(RuntimeEvent::ToolCallFinished {
            session_id: session_id.clone(),
            tool_name: name.to_string(),
            summary: tool_result.summary.clone(),
            agent_id: None,
            trace_id: None,
        });
        EventBus::drain_async_events(event_rx, on_event)?;

        Ok(ToolExecutionResult {
            id: id.to_string(),
            name: name.to_string(),
            output: tool_result,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn process_approval<F>(
        &self,
        session_id: &SessionId,
        tool_name: &str,
        args: &Value,
        _tool_args_json: &str,
        ctx: &ExecutionContext,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: &mut F,
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
            ctx.session_manager.cached_approval(session_id, key).await
        } else {
            false
        };

        if approved {
            tracing::debug!(
                session_id = session_id.id,
                tool = tool_name,
                "approval cached, skipping"
            );
            return Ok(());
        }

        tracing::debug!(session_id = session_id.id, tool = tool_name, risk = ?request.risk_level, "requesting approval");

        self.event_bus.emit(RuntimeEvent::AwaitingApproval {
            session_id: session_id.clone(),
            request: request.clone(),
            agent_id: None,
            trace_id: None,
        });
        EventBus::drain_async_events(event_rx, on_event)?;

        let decision = match self.approval_handler.as_ref() {
            Some(handler) => {
                let timeout = std::time::Duration::from_secs(
                    std::env::var("APPROVAL_TIMEOUT_SECS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(300),
                );
                let result = tokio::time::timeout(
                    timeout,
                    handler.approve(request.clone(), ctx.cancel_token.clone()),
                )
                .await;
                match result {
                    Ok(result) => result.map_err(|e| {
                        AgentError::internal(format!("Approval handler failed: {e}"))
                    })?,
                    Err(_) => {
                        tracing::warn!(
                            session_id = session_id.id,
                            ?timeout,
                            "Approval timed out, defaulting to Deny"
                        );
                        crate::types::ApprovalDecision::Deny
                    }
                }
            }
            None => crate::types::ApprovalDecision::Deny,
        };

        match decision {
            crate::types::ApprovalDecision::AllowOnce => {
                tracing::info!(
                    session_id = session_id.id,
                    tool = tool_name,
                    decision = "AllowOnce",
                    "approval granted"
                );
            }
            crate::types::ApprovalDecision::AllowAlways => {
                tracing::info!(
                    session_id = session_id.id,
                    tool = tool_name,
                    decision = "AllowAlways",
                    "approval granted (cached)"
                );
                if let Some(action_key) = request.action_key.clone() {
                    ctx.session_manager
                        .cache_approval(session_id, action_key)
                        .await;
                }
            }
            crate::types::ApprovalDecision::Deny => {
                tracing::warn!(
                    session_id = session_id.id,
                    tool = tool_name,
                    decision = "Deny",
                    "approval denied"
                );
                let denial_summary =
                    format!("[Action Denied]: tool {} rejected by approval", tool_name);
                // 不记录到 session 历史 — 用户拒绝是 UI 层交互，不需要 LLM 看到
                self.event_bus.emit(RuntimeEvent::ToolCallFinished {
                    session_id: session_id.clone(),
                    tool_name: tool_name.to_string(),
                    summary: denial_summary,
                    agent_id: None,
                    trace_id: None,
                });
                let _ = EventBus::drain_async_events(event_rx, on_event);
                return Err(AgentError::ApprovalDenied {
                    tool_name: tool_name.to_string(),
                });
            }
        }

        Ok(())
    }

    /// Orchestrate a batch of tool calls: parse args, check approval, execute.
    /// Returns results in order. The caller handles session push and control flow.
    pub async fn orchestrate<F>(
        &self,
        session_id: &SessionId,
        tool_calls: &[(String, String, String)],
        ctx: &ExecutionContext,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: &mut F,
    ) -> AgentResult<Vec<ToolExecutionResult>>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let mut results = Vec::with_capacity(tool_calls.len());

        for (id, name, args_str) in tool_calls {
            let args: Value =
                serde_json::from_str(args_str).map_err(|_| AgentError::ToolArgsInvalid {
                    name: name.clone(),
                    raw: args_str.clone(),
                })?;

            self.process_approval(session_id, name, &args, args_str, ctx, event_rx, on_event)
                .await?;

            let result = self
                .execute_tool(
                    session_id, id, name, &args, args_str, ctx, event_rx, on_event,
                )
                .await?;

            results.push(result);
        }

        Ok(results)
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
    #[allow(dead_code)]
    pub name: String,
    pub output: ToolOutput,
}

/// Grouped context passed through the tool execution call chain.
/// Reduces parameter count on `execute_tool` and `process_approval`.
pub(crate) struct ExecutionContext {
    pub session_manager: SessionManager,
    pub llm_client: Option<Arc<dyn crate::llm::StreamClient>>,
    pub language: Language,
    pub tool_timeout_ms: Option<u64>,
    pub max_output_chars: Option<usize>,
    pub cancel_token: tokio_util::sync::CancellationToken,
}
