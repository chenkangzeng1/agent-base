use agent_base::{AgentResult, ApprovalDecision, ApprovalHandler, ApprovalRequest};
use async_trait::async_trait;

/// Approval mode
#[derive(Clone, Debug)]
pub enum ApprovalMode {
    /// Auto-approve everything
    Auto,
    /// Deny everything
    DenyAll,
}

/// I/O-free approval handler — pure strategy, suitable for all consumers.
pub struct AutoApprovalHandler {
    mode: ApprovalMode,
}

impl AutoApprovalHandler {
    pub fn new(mode: ApprovalMode) -> Self {
        Self { mode }
    }
}

#[async_trait]
impl ApprovalHandler for AutoApprovalHandler {
    async fn approve(
        &self,
        request: ApprovalRequest,
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> AgentResult<ApprovalDecision> {
        match &self.mode {
            ApprovalMode::Auto => {
                tracing::info!(decision = "AllowAlways", tool = %request.title, "auto-approved");
                Ok(ApprovalDecision::AllowAlways)
            }
            ApprovalMode::DenyAll => {
                tracing::info!(decision = "Deny", tool = %request.title, "auto-denied");
                Ok(ApprovalDecision::Deny)
            }
        }
    }
}
