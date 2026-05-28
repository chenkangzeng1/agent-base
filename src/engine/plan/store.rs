use async_trait::async_trait;
use serde_json::Value;

use crate::types::{AgentResult, ExecutionPlan, PlanStoreData};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ExecutionPlan, PlanStep};
    use serde_json::Value;

    #[test]
    fn test_in_memory_plan_store() {
        use tokio::runtime::Runtime;
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemoryPlanStore::new();
            let plan = ExecutionPlan::new("plan-1", "Test Plan");

            store
                .save_plan(&plan, serde_json::json!({"key": "value"}))
                .await
                .unwrap();

            let loaded = store.load_plan("plan-1").await.unwrap();
            assert!(loaded.is_some());
            let data = loaded.unwrap();
            assert_eq!(data.plan.id, "plan-1");
            assert_eq!(data.plan.objective, "Test Plan");

            let plans = store.list_plans().await.unwrap();
            assert_eq!(plans.len(), 1);
            assert_eq!(plans[0], "plan-1");

            store.delete_plan("plan-1").await.unwrap();
            let loaded = store.load_plan("plan-1").await.unwrap();
            assert!(loaded.is_none());
        });
    }

    #[test]
    fn test_execution_plan_progress() {
        let mut plan = ExecutionPlan::new("plan-1", "Test");
        plan.steps
            .push(PlanStep::new("s1", "Step 1", Value::Null));
        plan.steps
            .push(PlanStep::new("s2", "Step 2", Value::Null));
        plan.steps
            .push(PlanStep::new("s3", "Step 3", Value::Null));

        assert_eq!(plan.progress(), (0, 3));
        assert!(!plan.is_completed());

        plan.steps[0].status = crate::types::StepStatus::Completed;
        assert_eq!(plan.progress(), (1, 3));

        plan.steps[1].status = crate::types::StepStatus::Skipped;
        assert_eq!(plan.progress(), (2, 3));

        plan.steps[2].status = crate::types::StepStatus::Completed;
        assert_eq!(plan.progress(), (3, 3));
        assert!(plan.is_completed());
    }

    #[test]
    fn test_execution_plan_has_failed() {
        let mut plan = ExecutionPlan::new("plan-1", "Test");
        plan.steps
            .push(PlanStep::new("s1", "Step 1", Value::Null));

        assert!(!plan.has_failed());

        plan.steps[0].status = crate::types::StepStatus::Failed;
        assert!(plan.has_failed());
    }
}
