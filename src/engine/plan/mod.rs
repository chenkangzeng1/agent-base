mod traits;
mod executor;
mod store;
mod streaming_parser;

pub use traits::{PlanGenerator, StepContinuePolicy, StepExecutor, RecoveryStrategy};
pub use executor::ToolCallingStepExecutor;
pub use store::{InMemoryPlanStore, PlanStore};
pub use streaming_parser::StreamingJsonParser;

/// Always continues.
pub struct AlwaysContinue;

#[async_trait::async_trait]
impl StepContinuePolicy for AlwaysContinue {
    async fn should_continue(
        &self,
        _plan: &crate::types::ExecutionPlan,
        _current_step: &crate::types::PlanStep,
    ) -> crate::types::AgentResult<bool> {
        Ok(true)
    }
}

/// Always aborts on failure.
pub struct AbortOnFailure;

#[async_trait::async_trait]
impl RecoveryStrategy for AbortOnFailure {
    async fn handle_step_failure(
        &self,
        _step: &crate::types::PlanStep,
        _error: &str,
        _retry_count: usize,
    ) -> crate::types::AgentResult<crate::types::RecoveryAction> {
        Ok(crate::types::RecoveryAction::Abort)
    }
}
