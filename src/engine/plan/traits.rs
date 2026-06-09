use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::tool::ToolContext;
use crate::types::{AgentResult, ExecutionPlan, PlanStep, RuntimeEvent, StepResult};

/// Generates an `ExecutionPlan` from a high-level objective.
///
/// The generator may use LLM prompting, rule engines, or templates.
///
/// `on_event` is an optional channel for emitting progress events during
/// plan generation (e.g. `PlanGenerating`, `PlanStepParsed`, `ThoughtDelta`).
/// Implementors that don't support streaming can ignore it.
#[async_trait]
pub trait PlanGenerator: Send + Sync {
    async fn generate_plan(
        &self,
        objective: &str,
        context: &str,
        tools: &[Value],
        on_event: Option<mpsc::UnboundedSender<RuntimeEvent>>,
    ) -> AgentResult<ExecutionPlan>;
}

/// Executes a single `PlanStep` and returns its result.
///
/// Implementors know how to interpret `step.payload` for their domain.
#[async_trait]
pub trait StepExecutor: Send + Sync {
    async fn execute_step(
        &self,
        step: &PlanStep,
        step_outputs: &Value,
        ctx: &ToolContext,
    ) -> AgentResult<StepResult>;
}

/// Decides whether the plan should continue executing a given step.
#[async_trait]
pub trait StepContinuePolicy: Send + Sync {
    async fn should_continue(
        &self,
        plan: &ExecutionPlan,
        current_step: &PlanStep,
        step_outputs: &Value,
    ) -> AgentResult<bool>;
}

/// Decides what to do when a step fails.
#[async_trait]
pub trait RecoveryStrategy: Send + Sync {
    async fn handle_step_failure(
        &self,
        step: &PlanStep,
        error: &str,
        retry_count: usize,
        plan: &ExecutionPlan,
        step_outputs: &Value,
    ) -> AgentResult<crate::types::RecoveryAction>;
}

/// Adaptive recovery strategy — provides intelligent recovery decisions
/// when steps fail repeatedly.
///
/// Unlike [`RecoveryStrategy`] which handles simple Retry/Skip/Abort decisions,
/// `AdaptiveRecoveryStrategy` can generate alternative steps or replan the
/// remaining execution.
///
/// The framework's execution loop handles progressive recovery orchestration:
/// 1. **Level 0**: Framework-level retries (`max_retries`)
/// 2. **Level 1/2**: This strategy (`max_alternatives` / `max_replans`)
/// 3. **Level 3**: Fallback to [`RecoveryStrategy`] as final safety net
///
/// The strategy can read quota information from [`RecoveryContext`](crate::types::RecoveryContext)
/// for soft decision-making, but the framework's `max_*` limits are hard guarantees
/// that the strategy cannot exceed.
#[async_trait]
pub trait AdaptiveRecoveryStrategy: Send + Sync {
    /// Attempt to recover from a failed step.
    ///
    /// Should return one of:
    /// - `RecoveryAction::Alternative { step, root_step_id }` — try a different approach
    /// - `RecoveryAction::Replan { steps, clear_future_phases }` — replan remaining work
    /// - `RecoveryAction::Skip` — skip the failed step
    /// - `RecoveryAction::Abort` — give up on the plan
    ///
    /// Note: should NOT return `RecoveryAction::Retry` — entering this method
    /// means retries are already exhausted; returning Retry is treated as Abort.
    async fn recover(
        &self,
        ctx: &crate::types::RecoveryContext,
    ) -> AgentResult<crate::types::RecoveryAction>;
}
