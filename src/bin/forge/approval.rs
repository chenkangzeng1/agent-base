//! CLI interactive approval handler — terminal stdin/stderr interaction.
//!
//! Only for CLI consumers. Web apps should use `phi_agent::AutoApprovalHandler`.

use std::io::{self, Write};

use agent_base::{AgentError, AgentResult, ApprovalDecision, ApprovalHandler, ApprovalRequest, RiskLevel};
use async_trait::async_trait;

/// CLI interactive approval handler
pub struct CliApprovalHandler;

impl CliApprovalHandler {
    pub fn new() -> Self {
        Self
    }

    fn risk_badge(level: &RiskLevel) -> &'static str {
        match level {
            RiskLevel::Safe => "\u{1F7E2} Safe",
            RiskLevel::Sensitive => "\u{1F7E1} Sensitive",
            RiskLevel::Destructive => "\u{1F534} Destructive",
        }
    }

    async fn interactive_prompt(
        &self,
        request: &ApprovalRequest,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> AgentResult<ApprovalDecision> {
        eprintln!();
        eprintln!("  ⚠️  {} ", request.title);
        eprintln!("     Risk: {}", Self::risk_badge(&request.risk_level));
        eprintln!("     {}", request.message);
        eprintln!();

        loop {
            if cancel_token.is_cancelled() {
                return Err(AgentError::Cancelled);
            }

            eprint!("     Confirm execution? [y=allow / n=deny]: ");
            io::stderr()
                .flush()
                .map_err(|e| AgentError::internal(format!("flush stderr failed: {e}")))?;

            let line = read_stdin_line_cancellable(cancel_token).await?;

            match line.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" => {
                    tracing::info!(decision = "AllowOnce", tool = %request.title, "user approved");
                    return Ok(ApprovalDecision::AllowOnce);
                }
                "n" | "no" => {
                    tracing::info!(decision = "Deny", tool = %request.title, "user denied");
                    return Ok(ApprovalDecision::Deny);
                }
                _ => eprintln!("     Invalid input — enter y or n"),
            }
        }
    }
}

#[async_trait]
impl ApprovalHandler for CliApprovalHandler {
    async fn approve(
        &self,
        request: ApprovalRequest,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> AgentResult<ApprovalDecision> {
        self.interactive_prompt(&request, &cancel_token).await
    }
}

async fn read_stdin_line_cancellable(
    cancel_token: &tokio_util::sync::CancellationToken,
) -> AgentResult<String> {
    use tokio::io::AsyncBufReadExt;

    tokio::select! {
        _ = cancel_token.cancelled() => Err(AgentError::Cancelled),
        result = async {
            let stdin = tokio::io::BufReader::new(tokio::io::stdin());
            let mut lines = stdin.lines();
            match lines.next_line().await {
                Ok(Some(line)) => Ok(line),
                Ok(None) => Err(AgentError::Cancelled),
                Err(e) => Err(AgentError::internal(format!("read stdin failed: {e}"))),
            }
        } => result,
    }
}
