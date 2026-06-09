mod traits;
mod executor;
mod llm_generator;
mod store;
mod streaming_parser;
mod adaptive_recovery;

pub use traits::{PlanGenerator, StepContinuePolicy, StepExecutor, RecoveryStrategy, AdaptiveRecoveryStrategy};
pub use executor::ToolCallingStepExecutor;
pub use llm_generator::{LlmPlanGenerator, PlanOptions};
pub use store::{InMemoryPlanStore, PlanStore};
pub use streaming_parser::StreamingJsonParser;
pub use adaptive_recovery::LlmAdaptiveRecovery;

use std::sync::Arc;
use serde_json::Value;

use crate::types::{AgentError, ErrorKind, RecoveryAction};

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

    /// Create an adaptive recovery policy that decides based on error kind.
    pub fn adaptive() -> Arc<RecoveryPolicy> {
        Arc::new(RecoveryPolicy::default())
    }
}

// ---------------------------------------------------------------------------
// RecoveryPolicy — error-kind-aware recovery decision maker
// ---------------------------------------------------------------------------

/// A concrete recovery policy that makes retry/skip/abort decisions based on
/// the *kind* of error that occurred, rather than a fixed strategy.
///
/// This complements the trait-based `RecoveryStrategy`: while `RecoveryStrategy`
/// implementations are opaque and user-supplied, `RecoveryPolicy` provides a
/// structured, configurable approach where retry rules map to `ErrorKind` variants.
///
/// # Defaults
///
/// - `max_retries = 3`
/// - `retry_on_tool_failure = true`
/// - `retry_on_overload = true`
/// - `abort_on_cancel = true`
///
/// # Environment overrides
///
/// Use [`RecoveryPolicy::build_from_env`] to read configuration from environment
/// variables (`AGENT_MAX_RETRIES`, `AGENT_RETRY_ON_TOOL_FAILURE`, etc.).
#[derive(Debug, Clone)]
pub struct RecoveryPolicy {
    /// Maximum number of retries before forcing abort.
    pub max_retries: usize,
    /// Whether to retry on tool execution failures.
    pub retry_on_tool_failure: bool,
    /// Whether to retry on model overload / rate limit.
    pub retry_on_overload: bool,
    /// Whether to immediately abort on cancellation.
    pub abort_on_cancel: bool,
}

impl Default for RecoveryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_on_tool_failure: true,
            retry_on_overload: true,
            abort_on_cancel: true,
        }
    }
}

impl RecoveryPolicy {
    /// Decide the recovery action for a given error kind.
    ///
    /// This is the core decision method — it does **not** check retry count.
    /// Use [`with_context`](Self::with_context) to also enforce `max_retries`.
    pub fn for_error(&self, error: &ErrorKind) -> RecoveryAction {
        match error {
            ErrorKind::ToolCallFailed { .. } => {
                if self.retry_on_tool_failure {
                    RecoveryAction::Retry
                } else {
                    RecoveryAction::Abort
                }
            }
            ErrorKind::ToolNotFound => RecoveryAction::Abort,
            ErrorKind::ToolArgsInvalid => RecoveryAction::Abort,
            ErrorKind::ToolTimeout => {
                if self.retry_on_tool_failure {
                    RecoveryAction::Retry
                } else {
                    RecoveryAction::Abort
                }
            }
            ErrorKind::ModelOverloaded => {
                if self.retry_on_overload {
                    RecoveryAction::Retry
                } else {
                    RecoveryAction::Abort
                }
            }
            ErrorKind::RateLimited => {
                if self.retry_on_overload {
                    RecoveryAction::Retry
                } else {
                    RecoveryAction::Abort
                }
            }
            ErrorKind::Internal => RecoveryAction::Abort,
        }
    }

    /// Make a recovery decision considering both error kind and retry count.
    ///
    /// Returns `Abort` if `retry_count >= max_retries`, otherwise delegates
    /// to [`for_error`](Self::for_error).
    pub fn with_context(&self, error: &AgentError, retry_count: usize) -> RecoveryAction {
        if retry_count >= self.max_retries {
            return RecoveryAction::Abort;
        }
        self.for_error(&error.kind())
    }

    /// Build a `RecoveryPolicy` from environment variables.
    ///
    /// Supported variables:
    /// - `AGENT_MAX_RETRIES` — `usize`, default 3
    /// - `AGENT_RETRY_ON_TOOL_FAILURE` — `true`/`false`, default true
    /// - `AGENT_RETRY_ON_OVERLOAD` — `true`/`false`, default true
    /// - `AGENT_ABORT_ON_CANCEL` — `true`/`false`, default true
    pub fn build_from_env() -> Self {
        let max_retries = std::env::var("AGENT_MAX_RETRIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);

        let retry_on_tool_failure = std::env::var("AGENT_RETRY_ON_TOOL_FAILURE")
            .ok()
            .map(|v| v.to_lowercase() == "true" || v == "1")
            .unwrap_or(true);

        let retry_on_overload = std::env::var("AGENT_RETRY_ON_OVERLOAD")
            .ok()
            .map(|v| v.to_lowercase() == "true" || v == "1")
            .unwrap_or(true);

        let abort_on_cancel = std::env::var("AGENT_ABORT_ON_CANCEL")
            .ok()
            .map(|v| v.to_lowercase() == "true" || v == "1")
            .unwrap_or(true);

        Self {
            max_retries,
            retry_on_tool_failure,
            retry_on_overload,
            abort_on_cancel,
        }
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
    pub recovery_policy: Option<Arc<RecoveryPolicy>>,
    /// Framework-level max retries for Level 0 (simple retry with linear backoff).
    ///
    /// Only applies when `adaptive_recovery` is configured — the framework retries
    /// within the progressive pipeline before handing off to the adaptive strategy.
    ///
    /// **Note:** This is independent of [`RetryOnFailure::max_retries`] which is an
    /// older, strategy-level retry mechanism. If you use `adaptive_recovery`, prefer
    /// this field for retry control; the strategy-level `RetryOnFailure` is best
    /// suited for simple non-adaptive setups.
    ///
    /// Default: 0 (no framework-level retries).
    pub max_retries: usize,
    /// Maximum alternative steps to attempt per failed step. Default: 2.
    pub max_alternatives: usize,
    /// Maximum replan attempts per plan execution. Default: 1.
    pub max_replans: usize,
    /// Adaptive recovery strategy for intelligent failure recovery.
    /// When set, the progressive recovery pipeline is activated:
    /// Level 0 (retry) → Level 1/2 (adaptive) → Level 3 (fallback to `recovery`).
    pub adaptive_recovery: Option<Arc<dyn traits::AdaptiveRecoveryStrategy>>,
}

impl PlanConfig {
    pub fn new() -> Self {
        Self {
            executor: None,
            continue_policy: Arc::new(AlwaysContinue),
            recovery: Arc::new(AbortOnFailure),
            plan_store: None,
            recovery_policy: None,
            max_retries: 0,
            max_alternatives: 2,
            max_replans: 1,
            adaptive_recovery: None,
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

    /// Set an adaptive recovery policy for error-kind-aware decisions.
    pub fn recovery_policy(mut self, p: Arc<RecoveryPolicy>) -> Self {
        self.recovery_policy = Some(p);
        self
    }

    /// Set the maximum number of framework-level retries (Level 0).
    pub fn max_retries(mut self, n: usize) -> Self {
        self.max_retries = n;
        self
    }

    /// Set the maximum number of alternative step attempts (Level 1).
    pub fn max_alternatives(mut self, n: usize) -> Self {
        self.max_alternatives = n;
        self
    }

    /// Set the maximum number of replan attempts (Level 2).
    pub fn max_replans(mut self, n: usize) -> Self {
        self.max_replans = n;
        self
    }

    /// Set the adaptive recovery strategy.
    pub fn adaptive_recovery(mut self, s: Arc<dyn traits::AdaptiveRecoveryStrategy>) -> Self {
        self.adaptive_recovery = Some(s);
        self
    }
}

impl Default for PlanConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentError;

    // ── RecoveryPolicy::for_error tests ──────────────────────────

    #[test]
    fn for_error_tool_call_failed_retries_by_default() {
        let policy = RecoveryPolicy::default();
        let kind = ErrorKind::ToolCallFailed {
            tool_name: "t".to_string(),
        };
        assert_eq!(policy.for_error(&kind), RecoveryAction::Retry);
    }

    #[test]
    fn for_error_tool_call_failed_aborts_when_disabled() {
        let policy = RecoveryPolicy {
            retry_on_tool_failure: false,
            ..Default::default()
        };
        let kind = ErrorKind::ToolCallFailed {
            tool_name: "t".to_string(),
        };
        assert_eq!(policy.for_error(&kind), RecoveryAction::Abort);
    }

    #[test]
    fn for_error_tool_not_found_always_aborts() {
        let policy = RecoveryPolicy::default();
        assert_eq!(
            policy.for_error(&ErrorKind::ToolNotFound),
            RecoveryAction::Abort
        );
    }

    #[test]
    fn for_error_tool_args_invalid_always_aborts() {
        let policy = RecoveryPolicy::default();
        assert_eq!(
            policy.for_error(&ErrorKind::ToolArgsInvalid),
            RecoveryAction::Abort
        );
    }

    #[test]
    fn for_error_tool_timeout_retries_by_default() {
        let policy = RecoveryPolicy::default();
        assert_eq!(
            policy.for_error(&ErrorKind::ToolTimeout),
            RecoveryAction::Retry
        );
    }

    #[test]
    fn for_error_model_overloaded_retries_by_default() {
        let policy = RecoveryPolicy::default();
        assert_eq!(
            policy.for_error(&ErrorKind::ModelOverloaded),
            RecoveryAction::Retry
        );
    }

    #[test]
    fn for_error_model_overloaded_aborts_when_disabled() {
        let policy = RecoveryPolicy {
            retry_on_overload: false,
            ..Default::default()
        };
        assert_eq!(
            policy.for_error(&ErrorKind::ModelOverloaded),
            RecoveryAction::Abort
        );
    }

    #[test]
    fn for_error_rate_limited_retries_by_default() {
        let policy = RecoveryPolicy::default();
        assert_eq!(
            policy.for_error(&ErrorKind::RateLimited),
            RecoveryAction::Retry
        );
    }

    #[test]
    fn for_error_internal_always_aborts() {
        let policy = RecoveryPolicy::default();
        assert_eq!(
            policy.for_error(&ErrorKind::Internal),
            RecoveryAction::Abort
        );
    }

    // ── RecoveryPolicy::with_context tests ───────────────────────

    #[test]
    fn with_context_retries_within_limit() {
        let policy = RecoveryPolicy::default(); // max_retries = 3
        let err = AgentError::service_unavailable("overloaded");
        assert_eq!(policy.with_context(&err, 0), RecoveryAction::Retry);
        assert_eq!(policy.with_context(&err, 1), RecoveryAction::Retry);
        assert_eq!(policy.with_context(&err, 2), RecoveryAction::Retry);
    }

    #[test]
    fn with_context_aborts_at_limit() {
        let policy = RecoveryPolicy::default(); // max_retries = 3
        let err = AgentError::service_unavailable("overloaded");
        assert_eq!(policy.with_context(&err, 3), RecoveryAction::Abort);
        assert_eq!(policy.with_context(&err, 4), RecoveryAction::Abort);
    }

    #[test]
    fn with_context_aborts_for_non_retryable() {
        let policy = RecoveryPolicy::default();
        let err = AgentError::tool_not_found("missing");
        assert_eq!(policy.with_context(&err, 0), RecoveryAction::Abort);
    }

    // ── RecoveryPolicy::build_from_env tests ─────────────────────

    #[test]
    fn build_from_env_defaults() {
        // SAFETY: test-only env var manipulation, single-threaded
        unsafe {
            std::env::remove_var("AGENT_MAX_RETRIES");
            std::env::remove_var("AGENT_RETRY_ON_TOOL_FAILURE");
            std::env::remove_var("AGENT_RETRY_ON_OVERLOAD");
            std::env::remove_var("AGENT_ABORT_ON_CANCEL");
        }

        let policy = RecoveryPolicy::build_from_env();
        assert_eq!(policy.max_retries, 3);
        assert!(policy.retry_on_tool_failure);
        assert!(policy.retry_on_overload);
        assert!(policy.abort_on_cancel);
    }

    #[test]
    fn build_from_env_custom() {
        // SAFETY: test-only env var manipulation, single-threaded
        unsafe {
            std::env::set_var("AGENT_MAX_RETRIES", "5");
            std::env::set_var("AGENT_RETRY_ON_TOOL_FAILURE", "false");
            std::env::set_var("AGENT_RETRY_ON_OVERLOAD", "true");
            std::env::set_var("AGENT_ABORT_ON_CANCEL", "false");
        }

        let policy = RecoveryPolicy::build_from_env();
        assert_eq!(policy.max_retries, 5);
        assert!(!policy.retry_on_tool_failure);
        assert!(policy.retry_on_overload);
        assert!(!policy.abort_on_cancel);

        // SAFETY: cleanup after test
        unsafe {
            std::env::remove_var("AGENT_MAX_RETRIES");
            std::env::remove_var("AGENT_RETRY_ON_TOOL_FAILURE");
            std::env::remove_var("AGENT_RETRY_ON_OVERLOAD");
            std::env::remove_var("AGENT_ABORT_ON_CANCEL");
        }
    }

    // ── Recovery::adaptive test ──────────────────────────────────

    #[test]
    fn recovery_adaptive_returns_default_policy() {
        let policy = Recovery::adaptive();
        assert_eq!(policy.max_retries, 3);
        assert!(policy.retry_on_tool_failure);
    }

    // ── PlanConfig::recovery_policy builder test ─────────────────

    #[test]
    fn plan_config_with_recovery_policy() {
        let policy = Arc::new(RecoveryPolicy::default());
        let config = PlanConfig::new().recovery_policy(policy);
        assert!(config.recovery_policy.is_some());
    }
}
