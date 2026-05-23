use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::broadcast;

use crate::engine::approval::ApprovalHandler;
use crate::engine::middleware::MiddlewareRef;
use crate::engine::recovery::ToolErrorRecovery;
use crate::engine::runtime::event_bus::EventBus;
use crate::engine::runtime::session_manager::SessionManager;
use crate::tool::{ToolContext, ToolControlFlow, ToolOutput, ToolPolicy, ToolRegistry};
use crate::types::{AgentError, AgentEvent, AgentResult, Language, SessionId};

pub struct ToolEngine {
    tools: ToolRegistry,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
    tool_policy: Option<Arc<dyn ToolPolicy>>,
    middlewares: Vec<MiddlewareRef>,
    error_recovery: Arc<dyn ToolErrorRecovery>,
    event_bus: EventBus,
}

impl ToolEngine {
    pub fn new(
        tools: ToolRegistry,
        approval_handler: Option<Arc<dyn ApprovalHandler>>,
        tool_policy: Option<Arc<dyn ToolPolicy>>,
        middlewares: Vec<MiddlewareRef>,
        error_recovery: Arc<dyn ToolErrorRecovery>,
        event_bus: EventBus,
    ) -> Self {
        Self {
            tools,
            approval_handler,
            tool_policy,
            middlewares,
            error_recovery,
            event_bus,
        }
    }

    pub fn definitions(&self) -> Vec<Value> {
        self.tools.definitions()
    }

    pub async fn execute_tool(
        &self,
        session_id: &SessionId,
        id: &str,
        name: &str,
        args: &Value,
        tool_args_json: &str,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut (dyn FnMut(AgentEvent) -> AgentResult<()> + Send),
        session_manager: &SessionManager,
        llm_client: Option<Arc<dyn crate::llm::LlmClient>>,
        language: Language,
        tool_timeout_ms: Option<u64>,
        max_output_chars: Option<usize>,
    ) -> AgentResult<ToolExecutionResult> {
        self.event_bus.emit(AgentEvent::ToolCallStarted {
            session_id: session_id.clone(),
            tool_name: name.to_string(),
            args_json: tool_args_json.to_string(),
        });
        EventBus::drain_async_events(event_rx, on_event)?;

        let tool_context = ToolContext {
            session_id: session_id.clone(),
            event_bus: self.event_bus.sender(),
            llm_client: llm_client.clone(),
            session_store: Some(session_manager.session_store().clone()),
            language: language.clone(),
        };

        if let Some(policy) = self.tool_policy.as_ref() {
            policy.before_call(name, args, &tool_context)?;
        }

        let tool_result = match self.tools.get(name) {
            Some(tool) => {
                let timeout_duration = tool_timeout_ms.map(Duration::from_millis);
                let future = tool.call(args, &tool_context);
                let output = if let Some(duration) = timeout_duration {
                    match tokio::time::timeout(duration, future).await {
                        Ok(result) => result,
                        Err(_) => {
                            tracing::warn!(session_id = session_id.id, tool_name = name, "Tool execution timed out");
                            Ok(ToolOutput {
                                summary: "[Tool Timeout]".to_string(),
                                raw: None,
                                control_flow: ToolControlFlow::Continue,
                                truncation: None,
                            })
                        }
                    }
                } else {
                    future.await
                };

                match output {
                    Ok(mut output) => {
                        if let Some(max_chars) = max_output_chars {
                            if output.summary.len() > max_chars {
                                let original_summary_len = output.summary.len();
                                let original_raw_len = output.raw.as_ref().map(|v| v.to_string().len());
                                let truncated_len = max_chars.saturating_sub("...(truncated)".len());
                                output.summary.truncate(truncated_len);
                                output.summary.push_str("...(truncated)");
                                output.truncation = Some(crate::tool::TruncationInfo {
                                    original_summary_len,
                                    original_raw_len,
                                    max_allowed_chars: max_chars,
                                });
                            }
                        }
                        output
                    }
                    Err(e) => {
                        tracing::error!(session_id = session_id.id, tool_name = name, error = %e, "Tool execution failed");
                        return Err(AgentError::ToolExecution {
                            name: name.to_string(),
                            source: Box::new(e),
                        });
                    }
                }
            }
            None => ToolOutput {
                summary: format!("Tool {} not found", name),
                raw: None,
                control_flow: ToolControlFlow::Break,
                truncation: None,
            },
        };

        if let Some(policy) = self.tool_policy.as_ref() {
            policy.after_call(name, args, &tool_result, &tool_context)?;
        }

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

    pub async fn process_approval(
        &self,
        session_id: &SessionId,
        tool_name: &str,
        args: &Value,
        tool_args_json: &str,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut (dyn FnMut(AgentEvent) -> AgentResult<()> + Send),
        session_manager: &SessionManager,
    ) -> AgentResult<()> {
        let approval_request = self.tool_policy.as_ref().and_then(|policy| {
            policy.evaluate_approval(tool_name, args)
        });

        let Some(request) = approval_request else {
            return Ok(());
        };

        let approved = if let Some(key) = request.action_key.as_deref() {
            session_manager.cached_approval(session_id, key).await
        } else {
            false
        };

        if approved {
            return Ok(());
        }

        self.event_bus.emit(AgentEvent::AwaitingApproval {
            session_id: session_id.clone(),
            request: request.clone(),
        });
        EventBus::drain_async_events(event_rx, on_event)?;

        let decision = match self.approval_handler.as_ref() {
            Some(handler) => {
                let timeout = Duration::from_secs(300);
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

    pub fn tools_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tools
    }
}

pub struct ToolExecutionResult {
    pub id: String,
    pub name: String,
    pub output: ToolOutput,
}
