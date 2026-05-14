use serde_json::Value;
use tokio::sync::broadcast;
use tracing::info_span;

use crate::types::{AgentResult, AgentError, AgentEvent, ApprovalDecision, SessionId};

use super::AgentRuntime;

impl AgentRuntime {
    pub(super) async fn process_approval<F>(
        &mut self,
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
        let _span = info_span!("approval", session_id = session_id.id, tool = %tool_name).entered();

        let approval_request = self.tool_policy.as_ref().and_then(|policy| {
            policy.evaluate_approval(tool_name, args, tool_args_json)
        });

        let Some(request) = approval_request else {
            return Ok(());
        };

        let approved = request
            .action_key
            .as_deref()
            .is_some_and(|key| self.cached_approval(session_id, key));

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
                .map_err(|e| AgentError::internal(format!("审批处理失败: {e}")))?,
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
                let denial_summary =
                    format!("[Action Denied]: 审批拒绝执行工具 {tool_name}。");
                let session = self.session_mut_or_err(session_id)?;
                session.push_assistant_tool_call("", tool_name, tool_args_json);
                session.push_tool_result("", denial_summary.clone());
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
