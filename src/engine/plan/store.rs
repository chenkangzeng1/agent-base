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
        let plan = ExecutionPlan::with_single_phase(
            "plan-1",
            "Test",
            vec![
                PlanStep::new("s1", "Step 1", Value::Null),
                PlanStep::new("s2", "Step 2", Value::Null),
                PlanStep::new("s3", "Step 3", Value::Null),
            ],
        );
        // Use into_inner pattern to mutate steps
        let mut plan = plan;
        assert_eq!(plan.progress(), (0, 3));
        assert!(!plan.is_completed());

        plan.find_step_mut("s1").unwrap().status = crate::types::StepStatus::Completed;
        assert_eq!(plan.progress(), (1, 3));

        plan.find_step_mut("s2").unwrap().status = crate::types::StepStatus::Skipped;
        assert_eq!(plan.progress(), (2, 3));

        plan.find_step_mut("s3").unwrap().status = crate::types::StepStatus::Completed;
        assert_eq!(plan.progress(), (3, 3));
        assert!(plan.is_completed());
    }

    #[test]
    fn test_execution_plan_has_failed() {
        let mut plan = ExecutionPlan::with_single_phase(
            "plan-1",
            "Test",
            vec![PlanStep::new("s1", "Step 1", Value::Null)],
        );

        assert!(!plan.has_failed());

        plan.find_step_mut("s1").unwrap().status = crate::types::StepStatus::Failed;
        assert!(plan.has_failed());
    }

    #[test]
    fn test_multi_phase_plan() {
        use crate::types::{PhaseStatus, PlanPhase};

        let mut plan = ExecutionPlan::new("plan-multi", "Multi-phase test");
        plan.phases = vec![
            PlanPhase::new(
                "phase-1",
                "系统资源检查",
                vec![
                    PlanStep::new("s1", "检查CPU", Value::Null),
                    PlanStep::new("s2", "检查内存", Value::Null),
                ],
            ),
            PlanPhase::new(
                "phase-2",
                "服务检查",
                vec![
                    PlanStep::new("s3", "检查sshd", Value::Null)
                        .with_dependencies(vec!["s1".to_string()]),
                ],
            ),
        ];

        // Initial state
        assert_eq!(plan.total_steps(), 3);
        assert_eq!(plan.progress(), (0, 3));
        assert!(!plan.is_completed());
        assert!(!plan.has_failed());

        // Phase 1: complete first step
        plan.find_step_mut("s1").unwrap().status = crate::types::StepStatus::Completed;
        assert_eq!(plan.progress(), (1, 3));
        assert!(!plan.is_completed());

        // Cross-phase dependency: s3 depends on s1 (now completed)
        let s3 = plan.find_step("s3").unwrap();
        assert!(s3.dependencies.iter().all(|dep| {
            plan.find_step(dep)
                .map(|s| matches!(s.status, crate::types::StepStatus::Completed))
                .unwrap_or(false)
        }));

        // Complete all steps
        plan.find_step_mut("s2").unwrap().status = crate::types::StepStatus::Completed;
        plan.find_step_mut("s3").unwrap().status = crate::types::StepStatus::Completed;
        assert_eq!(plan.progress(), (3, 3));

        // Set phase statuses (simulating execution)
        plan.phases[0].status = PhaseStatus::Completed;
        plan.phases[1].status = PhaseStatus::Completed;
        assert!(plan.is_completed());

        // Test phase progress
        assert_eq!(plan.phases[0].progress(), (2, 2));
        assert_eq!(plan.phases[1].progress(), (1, 1));
        assert!(plan.phases[0].is_completed());
        assert!(plan.phases[1].is_completed());

        // Test all_steps ordering: phase-1 steps before phase-2 steps
        let step_ids: Vec<&str> = plan.all_steps().map(|s| s.id.as_str()).collect();
        assert_eq!(step_ids, vec!["s1", "s2", "s3"]);
    }

    #[test]
    fn test_empty_phases_not_completed() {
        let plan = ExecutionPlan::new("plan-empty", "Empty");
        assert!(!plan.is_completed());
        assert_eq!(plan.total_steps(), 0);
    }
}
