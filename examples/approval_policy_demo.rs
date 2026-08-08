use serde_json::{Value, json};
use std::collections::HashMap;
/// 演示 ToolPolicy 如何根据计划状态决定是否需要审批
///
/// 问题场景：
/// - create_plan 成功后，react_loop 自动执行 execute_plan
/// - 但 ToolPolicy 不知道用户已确认，仍然请求审批
///
/// 解决方案：
/// - ToolPolicy 检查计划状态
/// - 如果计划状态是 "awaiting_confirmation"，跳过审批
///
/// 运行方式：
/// cargo run --example approval_policy_demo
use std::sync::Arc;
use tokio::sync::RwLock;

/// 模拟 PlanStore
struct MockPlanStore {
    plans: RwLock<HashMap<String, PlanData>>,
}

#[derive(Clone)]
struct PlanData {
    plan_id: String,
    status: String, // "awaiting_confirmation", "approved", "executing", etc.
    steps: Vec<StepData>,
}

#[derive(Clone)]
#[allow(dead_code)]
struct StepData {
    id: String,
    command: String,
    risk_level: String, // "safe", "sensitive", "destructive"
}

impl MockPlanStore {
    fn new() -> Self {
        Self {
            plans: RwLock::new(HashMap::new()),
        }
    }

    async fn save_plan(&self, plan: PlanData) {
        let mut plans = self.plans.write().await;
        plans.insert(plan.plan_id.clone(), plan);
    }

    async fn load_plan(&self, plan_id: &str) -> Option<PlanData> {
        let plans = self.plans.read().await;
        plans.get(plan_id).cloned()
    }
}

/// 模拟 ToolPolicy
struct TestToolPolicy {
    plan_store: Arc<MockPlanStore>,
}

impl TestToolPolicy {
    fn new(plan_store: Arc<MockPlanStore>) -> Self {
        Self { plan_store }
    }

    /// 评估是否需要审批
    async fn evaluate_approval(&self, tool_name: &str, args: &Value) -> Option<String> {
        if tool_name != "execute_plan" {
            return None;
        }

        let plan_id = args.get("plan_id")?.as_str()?;

        // 加载计划
        let plan_data = self.plan_store.load_plan(plan_id).await?;

        // 检查计划状态
        if plan_data.status == "approved" {
            // 用户已确认计划，跳过审批
            println!("[ToolPolicy] 计划状态为 approved，跳过审批（用户已确认）");
            return None;
        }

        // 检查命令风险等级
        let has_sensitive = plan_data.steps.iter().any(|s| s.risk_level == "sensitive");
        let has_destructive = plan_data
            .steps
            .iter()
            .any(|s| s.risk_level == "destructive");

        if has_destructive {
            println!("[ToolPolicy] 检测到破坏性命令，需要审批");
            return Some("destructive".to_string());
        }

        if has_sensitive {
            println!("[ToolPolicy] 检测到敏感命令，需要审批");
            return Some("sensitive".to_string());
        }

        println!("[ToolPolicy] 所有命令都是安全的，跳过审批");
        None
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== ToolPolicy 审批决策演示 ===\n");

    let plan_store = Arc::new(MockPlanStore::new());
    let policy = TestToolPolicy::new(plan_store.clone());

    // 场景 1: 计划状态为 approved（用户已确认）
    println!("--- 场景 1: 用户已确认计划（状态为 approved）---");
    plan_store
        .save_plan(PlanData {
            plan_id: "plan-1".to_string(),
            status: "approved".to_string(),
            steps: vec![StepData {
                id: "step-1".to_string(),
                command: "df -h".to_string(),
                risk_level: "safe".to_string(),
            }],
        })
        .await;

    let args = json!({"plan_id": "plan-1"});
    let result = policy.evaluate_approval("execute_plan", &args).await;
    println!("结果: {:?}\n", result);

    // 场景 2: 计划状态为 awaiting_confirmation（等待用户确认），包含敏感命令
    println!("--- 场景 2: 等待用户确认，计划包含敏感命令 ---");
    plan_store
        .save_plan(PlanData {
            plan_id: "plan-2".to_string(),
            status: "awaiting_confirmation".to_string(),
            steps: vec![StepData {
                id: "step-1".to_string(),
                command: "systemctl restart nginx".to_string(),
                risk_level: "sensitive".to_string(),
            }],
        })
        .await;

    let args = json!({"plan_id": "plan-2"});
    let result = policy.evaluate_approval("execute_plan", &args).await;
    println!("结果: {:?}\n", result);

    // 场景 3: 计划状态为 awaiting_confirmation（等待用户确认），所有命令都是安全的
    println!("--- 场景 3: 等待用户确认，所有命令都是安全的 ---");
    plan_store
        .save_plan(PlanData {
            plan_id: "plan-3".to_string(),
            status: "awaiting_confirmation".to_string(),
            steps: vec![
                StepData {
                    id: "step-1".to_string(),
                    command: "df -h".to_string(),
                    risk_level: "safe".to_string(),
                },
                StepData {
                    id: "step-2".to_string(),
                    command: "free -m".to_string(),
                    risk_level: "safe".to_string(),
                },
            ],
        })
        .await;

    let args = json!({"plan_id": "plan-3"});
    let result = policy.evaluate_approval("execute_plan", &args).await;
    println!("结果: {:?}\n", result);

    // 场景 4: 计划状态为 awaiting_confirmation（等待用户确认），包含破坏性命令
    println!("--- 场景 4: 等待用户确认，包含破坏性命令 ---");
    plan_store
        .save_plan(PlanData {
            plan_id: "plan-4".to_string(),
            status: "awaiting_confirmation".to_string(),
            steps: vec![StepData {
                id: "step-1".to_string(),
                command: "rm -rf /tmp/*".to_string(),
                risk_level: "destructive".to_string(),
            }],
        })
        .await;

    let args = json!({"plan_id": "plan-4"});
    let result = policy.evaluate_approval("execute_plan", &args).await;
    println!("结果: {:?}\n", result);

    println!("=== 演示完成 ===");
    println!("\n结论:");
    println!("- ToolPolicy 根据计划状态和命令风险等级决定是否需要审批");
    println!("- 如果计划状态是 approved（用户已确认），跳过审批");
    println!("- 如果计划状态是 awaiting_confirmation（等待确认），需要审批");
    println!("- 如果命令是安全的，跳过审批");
    println!("- 如果命令是敏感的或破坏性的，需要审批");

    Ok(())
}
