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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::{ApprovalRequest, RiskLevel};

    #[tokio::test]
    async fn test_auto_mode_approves_all() {
        let handler = AutoApprovalHandler::new(ApprovalMode::Auto);
        let request = ApprovalRequest {
            title: "Delete file".into(),
            message: "This will delete /tmp/important.txt".into(),
            action_key: None,
            risk_level: RiskLevel::Destructive,
            raw: None,
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        let decision = handler.approve(request, cancel).await.unwrap();
        assert!(matches!(decision, ApprovalDecision::AllowAlways));
    }

    #[tokio::test]
    async fn test_deny_all_mode_denies() {
        let handler = AutoApprovalHandler::new(ApprovalMode::DenyAll);
        let request = ApprovalRequest {
            title: "Read file".into(),
            message: "Read /tmp/safe.txt".into(),
            action_key: None,
            risk_level: RiskLevel::Safe,
            raw: None,
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        let decision = handler.approve(request, cancel).await.unwrap();
        assert!(matches!(decision, ApprovalDecision::Deny));
    }

    #[tokio::test]
    async fn test_deny_all_denies_even_safe_operations() {
        let handler = AutoApprovalHandler::new(ApprovalMode::DenyAll);
        let request = ApprovalRequest {
            title: "Safe thing".into(),
            message: "Perfectly safe".into(),
            action_key: None,
            risk_level: RiskLevel::Safe,
            raw: None,
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        let decision = handler.approve(request, cancel).await.unwrap();
        assert!(matches!(decision, ApprovalDecision::Deny));
    }
}
