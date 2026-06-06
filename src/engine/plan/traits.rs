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
