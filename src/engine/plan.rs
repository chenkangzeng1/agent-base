use async_trait::async_trait;
use serde_json::Value;

use crate::types::{
    AgentResult, ExecutionPlan, PlanStep, PlanStoreData, RecoveryAction, StepResult,
};

#[async_trait]
pub trait PlanExecutor: Send + Sync {
    async fn generate_plan(
        &self,
        objective: &str,
        context: &str,
        tools: &[Value],
    ) -> AgentResult<ExecutionPlan>;

    async fn execute_step(
        &self,
        step: &PlanStep,
        plan_context: &Value,
    ) -> AgentResult<StepResult>;

    async fn should_continue(
        &self,
        plan: &ExecutionPlan,
        current_step: &PlanStep,
    ) -> AgentResult<bool>;

    async fn handle_step_failure(
        &self,
        step: &PlanStep,
        error: &str,
        retry_count: usize,
    ) -> AgentResult<RecoveryAction>;
}

#[async_trait]
pub trait PlanStore: Send + Sync {
    async fn save_plan(&self, plan: &ExecutionPlan, metadata: Value) -> AgentResult<()>;

    async fn load_plan(&self, plan_id: &str) -> AgentResult<Option<PlanStoreData>>;

    async fn delete_plan(&self, plan_id: &str) -> AgentResult<()>;

    async fn list_plans(&self) -> AgentResult<Vec<String>>;
}

pub struct InMemoryPlanStore {
    plans: tokio::sync::RwLock<std::collections::HashMap<String, PlanStoreData>>,
}

impl InMemoryPlanStore {
    pub fn new() -> Self {
        Self {
            plans: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for InMemoryPlanStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PlanStore for InMemoryPlanStore {
    async fn save_plan(&self, plan: &ExecutionPlan, metadata: Value) -> AgentResult<()> {
        let mut plans = self.plans.write().await;
        plans.insert(
            plan.id.clone(),
            PlanStoreData {
                plan: plan.clone(),
                metadata,
            },
        );
        Ok(())
    }

    async fn load_plan(&self, plan_id: &str) -> AgentResult<Option<PlanStoreData>> {
        let plans = self.plans.read().await;
        Ok(plans.get(plan_id).cloned())
    }

    async fn delete_plan(&self, plan_id: &str) -> AgentResult<()> {
        let mut plans = self.plans.write().await;
        plans.remove(plan_id);
        Ok(())
    }

    async fn list_plans(&self) -> AgentResult<Vec<String>> {
        let plans = self.plans.read().await;
        Ok(plans.keys().cloned().collect())
    }
}
