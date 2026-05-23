use serde_json::Value;
use tokio::sync::broadcast;

use crate::types::{AgentResult, AgentError, AgentEvent, ApprovalDecision, SessionId};

use super::AgentRuntime;

impl AgentRuntime {
    pub(super) async fn process_approval<F>(
        &self,
        session_id: &SessionId,
        tool_name: &str,
        args: &Value,
        tool_args_json: &str,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<()>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let approval_request = self.tool_policy.as_ref().and_then(|policy| {
            policy.evaluate_approval(tool_name, args)
        });

        let Some(request) = approval_request else {
            return Ok(());
        };

        let approved = if let Some(key) = request.action_key.as_deref() {
            self.cached_approval(session_id, key).await
        } else {
            false
        };

        if approved {
            return Ok(());
        }

        self.emit_event(AgentEvent::AwaitingApproval {
            session_id: session_id.clone(),
            request: request.clone(),
        });
        Self::drain_async_events(event_rx, on_event)?;

        let decision = match self.approval_handler() {
            Some(handler) => handler
                .approve(request.clone())
                .await
                .map_err(|e| AgentError::internal(format!("Approval handler failed: {e}")))?,
            None => ApprovalDecision::Deny,
        };

        match decision {
            ApprovalDecision::AllowOnce => {}
            ApprovalDecision::AllowAlways => {
                if let Some(action_key) = request.action_key.clone() {
                    self.cache_approval(session_id, action_key).await;
                }
            }
            ApprovalDecision::Deny => {
                let denial_summary =
                    format!("[Action Denied]: tool {} rejected by approval", tool_name);
                self.with_session_mut(session_id, |session| {
                    session.push_assistant_tool_call("", tool_name, tool_args_json);
                    session.push_tool_result("", denial_summary.clone());
                }).await?;
                self.emit_event(AgentEvent::ToolCallFinished {
                    session_id: session_id.clone(),
                    tool_name: tool_name.to_string(),
                    summary: denial_summary,
                });
                Self::drain_async_events(event_rx, on_event)?;
                return Err(AgentError::ApprovalDenied {
                    tool_name: tool_name.to_string(),
                });
            }
        }

        Ok(())
    }
}
