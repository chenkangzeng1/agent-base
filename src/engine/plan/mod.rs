mod traits;
mod executor;
mod llm_generator;
mod store;
mod streaming_parser;

pub use traits::{PlanGenerator, StepContinuePolicy, StepExecutor, RecoveryStrategy};
pub use executor::ToolCallingStepExecutor;
pub use llm_generator::LlmPlanGenerator;
pub use store::{InMemoryPlanStore, PlanStore};
pub use streaming_parser::StreamingJsonParser;

use std::sync::Arc;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Built-in StepContinuePolicy implementations
// ---------------------------------------------------------------------------

/// Always continues.
pub struct AlwaysContinue;

#[async_trait::async_trait]
impl StepContinuePolicy for AlwaysContinue {
    async fn should_continue(
        &self,
        _plan: &crate::types::ExecutionPlan,
        _current_step: &crate::types::PlanStep,
        _step_outputs: &Value,
    ) -> crate::types::AgentResult<bool> {
        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// Built-in RecoveryStrategy implementations
// ---------------------------------------------------------------------------

/// Always aborts on failure.
pub struct AbortOnFailure;

#[async_trait::async_trait]
impl RecoveryStrategy for AbortOnFailure {
    async fn handle_step_failure(
        &self,
        _step: &crate::types::PlanStep,
        _error: &str,
        _retry_count: usize,
        _plan: &crate::types::ExecutionPlan,
        _step_outputs: &Value,
    ) -> crate::types::AgentResult<crate::types::RecoveryAction> {
        Ok(crate::types::RecoveryAction::Abort)
    }
}

/// Always skips on failure.
pub struct SkipOnFailure;

#[async_trait::async_trait]
impl RecoveryStrategy for SkipOnFailure {
    async fn handle_step_failure(
        &self,
        _step: &crate::types::PlanStep,
        _error: &str,
        _retry_count: usize,
        _plan: &crate::types::ExecutionPlan,
        _step_outputs: &Value,
    ) -> crate::types::AgentResult<crate::types::RecoveryAction> {
        Ok(crate::types::RecoveryAction::Skip)
    }
}

/// Retries up to `max_retries` times, then aborts.
pub struct RetryOnFailure {
    pub max_retries: usize,
}

#[async_trait::async_trait]
impl RecoveryStrategy for RetryOnFailure {
    async fn handle_step_failure(
        &self,
        _step: &crate::types::PlanStep,
        _error: &str,
        retry_count: usize,
        _plan: &crate::types::ExecutionPlan,
        _step_outputs: &Value,
    ) -> crate::types::AgentResult<crate::types::RecoveryAction> {
        if retry_count < self.max_retries {
            Ok(crate::types::RecoveryAction::Retry)
        } else {
            Ok(crate::types::RecoveryAction::Abort)
        }
    }
}

/// Custom recovery strategy backed by a closure.
pub struct CustomRecovery(
    Box<
        dyn Fn(
                &crate::types::PlanStep,
                &str,
                usize,
                &crate::types::ExecutionPlan,
                &Value,
            ) -> crate::types::RecoveryAction
            + Send
            + Sync,
    >,
);

#[async_trait::async_trait]
impl RecoveryStrategy for CustomRecovery {
    async fn handle_step_failure(
        &self,
        step: &crate::types::PlanStep,
        error: &str,
        retry_count: usize,
        plan: &crate::types::ExecutionPlan,
        step_outputs: &Value,
    ) -> crate::types::AgentResult<crate::types::RecoveryAction> {
        Ok((self.0)(step, error, retry_count, plan, step_outputs))
    }
}

// ---------------------------------------------------------------------------
// Recovery — convenience constructors
// ---------------------------------------------------------------------------

/// Convenience constructors for `RecoveryStrategy` implementations.
///
/// # Examples
///
/// ```ignore
/// use agent_base::{Recovery, PlanConfig};
///
/// PlanConfig::new().recovery(Recovery::abort());
/// PlanConfig::new().recovery(Recovery::skip());
/// PlanConfig::new().recovery(Recovery::retry(3));
/// PlanConfig::new().recovery(Recovery::custom(|step, err, count, plan, step_outputs| {
///     if count < 2 {
///         crate::types::RecoveryAction::Retry
///     } else {
///         crate::types::RecoveryAction::Skip
///     }
/// }));
/// ```
pub struct Recovery;

impl Recovery {
    pub fn abort() -> Arc<dyn RecoveryStrategy> {
        Arc::new(AbortOnFailure)
    }

    pub fn skip() -> Arc<dyn RecoveryStrategy> {
        Arc::new(SkipOnFailure)
    }

    pub fn retry(max_retries: usize) -> Arc<dyn RecoveryStrategy> {
        Arc::new(RetryOnFailure { max_retries })
    }

    pub fn custom(
        f: impl Fn(
                &crate::types::PlanStep,
                &str,
                usize,
                &crate::types::ExecutionPlan,
                &Value,
            ) -> crate::types::RecoveryAction
            + Send
            + Sync
            + 'static,
    ) -> Arc<dyn RecoveryStrategy> {
        Arc::new(CustomRecovery(Box::new(f)))
    }
}

// ---------------------------------------------------------------------------
// PlanConfig — unified plan execution configuration
// ---------------------------------------------------------------------------

/// Configuration for `run_plan` and `run_plan_with_generator`.
///
/// Builder pattern with sensible defaults:
/// - `executor = None` → agentic mode (each step becomes an agent turn)
/// - `executor = Some(e)` → deterministic mode (steps executed by `e`)
/// - Runtime adaptive: steps with `tool_name` in payload use executor if available,
///   otherwise fall back to agentic mode
/// - `continue_policy` defaults to `AlwaysContinue`
/// - `recovery` defaults to `AbortOnFailure`
pub struct PlanConfig {
    pub(crate) executor: Option<Arc<dyn StepExecutor>>,
    pub continue_policy: Arc<dyn StepContinuePolicy>,
    pub recovery: Arc<dyn RecoveryStrategy>,
    pub plan_store: Option<Arc<dyn PlanStore>>,
}

impl PlanConfig {
    pub fn new() -> Self {
        Self {
            executor: None,
            continue_policy: Arc::new(AlwaysContinue),
            recovery: Arc::new(AbortOnFailure),
            plan_store: None,
        }
    }

    /// Set the step executor (enables deterministic mode).
    pub fn with_executor(mut self, e: Arc<dyn StepExecutor>) -> Self {
        self.executor = Some(e);
        self
    }

    /// Get the step executor (if set).
    pub fn executor(&self) -> Option<&Arc<dyn StepExecutor>> {
        self.executor.as_ref()
    }

    /// Set the continue policy.
    pub fn continue_policy(mut self, p: Arc<dyn StepContinuePolicy>) -> Self {
        self.continue_policy = p;
        self
    }

    /// Set the recovery strategy.
    pub fn recovery(mut self, r: Arc<dyn RecoveryStrategy>) -> Self {
        self.recovery = r;
        self
    }

    /// Set the plan store for persistence.
    pub fn store(mut self, s: Arc<dyn PlanStore>) -> Self {
        self.plan_store = Some(s);
        self
    }
}

impl Default for PlanConfig {
    fn default() -> Self {
        Self::new()
    }
}
