use agent_base::{AgentResult, ApprovalDecision, ApprovalHandler, ApprovalRequest};
use async_trait::async_trait;

/// Approval strategy.
#[derive(Clone, Debug)]
pub enum ApprovalMode {
    /// Automatically approve all tool calls without user interaction.
    Auto,
    /// Reject all tool calls that require approval.
    DenyAll,
}

/// I/O-free approval handler — pure strategy, suitable for all consumers.
///
/// Use [`ApprovalMode::Auto`] for automated/CI scenarios, or
/// [`ApprovalMode::DenyAll`] for read-only / preview modes.
pub struct AutoApprovalHandler {
    mode: ApprovalMode,
}

impl AutoApprovalHandler {
    /// Create a new handler with the given approval strategy.
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
            },
            ApprovalMode::DenyAll => {
                tracing::info!(decision = "Deny", tool = %request.title, "auto-denied");
                Ok(ApprovalDecision::Deny)
            },
        }
    }
}
