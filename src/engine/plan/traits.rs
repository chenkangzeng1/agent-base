use async_trait::async_trait;
use serde_json::Value;

use crate::tool::ToolContext;
use crate::types::{AgentResult, ExecutionPlan, PlanStep, StepResult};

/// Generates an `ExecutionPlan` from a high-level objective.
///
/// The generator may use LLM prompting, rule engines, or templates.
#[async_trait]
pub trait PlanGenerator: Send + Sync {
    async fn generate_plan(
        &self,
        objective: &str,
        context: &str,
        tools: &[Value],
    ) -> AgentResult<ExecutionPlan>;

    async fn generate_plan_streaming(
        &self,
        objective: &str,
        context: &str,
        tools: &[Value],
        on_generating: Box<dyn Fn() + Send>,
        on_step_parsed: Box<dyn Fn(usize, String, String) + Send>,
        _on_thought: Box<dyn Fn(String) + Send>,
    ) -> AgentResult<ExecutionPlan> {
        // Default implementation falls back to non-streaming
        let plan = self.generate_plan(objective, context, tools).await?;
        on_generating();
        for (i, step) in plan.all_steps().enumerate() {
            on_step_parsed(i, step.id.clone(), step.description.clone());
        }
        Ok(plan)
    }
}

/// Executes a single `PlanStep` and returns its result.
///
/// Implementors know how to interpret `step.payload` for their domain.
#[async_trait]
pub trait StepExecutor: Send + Sync {
    async fn execute_step(
        &self,
        step: &PlanStep,
        plan_context: &Value,
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
    ) -> AgentResult<crate::types::RecoveryAction>;
}
