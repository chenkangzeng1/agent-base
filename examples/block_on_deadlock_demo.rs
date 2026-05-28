/// 模拟 `block_on` 死锁场景及解决方案
///
/// 问题：`ToolPolicy.evaluate_approval` 是同步方法，但需要调用异步操作
///
/// 运行方式：
/// cargo run --example block_on_deadlock_demo
///
/// 结论：
/// - 使用 `handle.block_on()` 会导致死锁或 panic
/// - 解决方案：将 `ToolPolicy.evaluate_approval` 改为 async

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// 模拟 PlanStore
struct MockPlanStore {
    plans: RwLock<std::collections::HashMap<String, String>>,
}

impl MockPlanStore {
    fn new() -> Self {
        let mut plans = std::collections::HashMap::new();
        plans.insert("plan-1".to_string(), "plan_data_1".to_string());

        Self {
            plans: RwLock::new(plans),
        }
    }

    async fn load_plan(&self, plan_id: &str) -> Option<String> {
        // 模拟异步 IO
        tokio::time::sleep(Duration::from_millis(100)).await;
        let plans = self.plans.read().await;
        plans.get(plan_id).cloned()
    }
}

/// 当前的 ToolPolicy trait（同步）
trait SyncToolPolicy: Send + Sync {
    fn evaluate_approval(&self, tool_name: &str, plan_id: &str) -> Option<String>;
}

/// 改进的 ToolPolicy trait（异步）
#[async_trait::async_trait]
trait AsyncToolPolicy: Send + Sync {
    async fn evaluate_approval(&self, tool_name: &str, plan_id: &str) -> Option<String>;
}

/// 方式1: 使用 block_on（有问题）
struct BlockOnPolicy {
    plan_store: Arc<MockPlanStore>,
}

impl SyncToolPolicy for BlockOnPolicy {
    fn evaluate_approval(&self, tool_name: &str, plan_id: &str) -> Option<String> {
        println!("[block_on] evaluate_approval: {} - {}", tool_name, plan_id);

        // 尝试获取 runtime handle
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => {
                println!("[block_on] 无法获取 runtime handle");
                return None;
            }
        };

        // 这会导致 panic: "Cannot start a runtime from within a runtime"
        println!("[block_on] 调用 block_on...");
        let result = handle.block_on(self.plan_store.load_plan(plan_id));
        println!("[block_on] 完成: {:?}", result);
        result
    }
}

/// 方式2: 使用 async trait（推荐）
struct AsyncPolicy {
    plan_store: Arc<MockPlanStore>,
}

#[async_trait::async_trait]
impl AsyncToolPolicy for AsyncPolicy {
    async fn evaluate_approval(&self, tool_name: &str, plan_id: &str) -> Option<String> {
        println!("[async] evaluate_approval: {} - {}", tool_name, plan_id);

        // 直接 await，没有死锁风险
        let result = self.plan_store.load_plan(plan_id).await;
        println!("[async] 完成: {:?}", result);
        result
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== ToolPolicy 死锁问题演示 ===\n");

    let plan_store = Arc::new(MockPlanStore::new());

    // 测试方式2: async trait（推荐）
    println!("--- 方式2: async trait (推荐) ---");
    let policy2 = AsyncPolicy {
        plan_store: plan_store.clone(),
    };
    match tokio::time::timeout(
        Duration::from_secs(2),
        policy2.evaluate_approval("execute_plan", "plan-1")
    ).await {
        Ok(result) => println!("方式2 结果: {:?}\n", result),
        Err(_) => println!("方式2 超时!\n"),
    }

    // 测试方式1: block_on（会 panic）
    println!("--- 方式1: block_on (会 panic) ---");
    println!("注意: 会 panic 'Cannot start a runtime from within a runtime'\n");

    let policy1 = BlockOnPolicy {
        plan_store: plan_store.clone(),
    };

    // 使用 catch_unwind 捕获 panic
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        policy1.evaluate_approval("execute_plan", "plan-1")
    }));

    match result {
        Ok(val) => println!("方式1 结果: {:?}\n", val),
        Err(e) => {
            if let Some(msg) = e.downcast_ref::<String>() {
                println!("方式1 panic: {}\n", msg);
            } else if let Some(msg) = e.downcast_ref::<&str>() {
                println!("方式1 panic: {}\n", msg);
            } else {
                println!("方式1 panic: (unknown)\n");
            }
        }
    }

    println!("=== 结论 ===");
    println!("1. 在同步函数中使用 block_on 调用异步操作会导致 panic");
    println!("2. 错误信息: 'Cannot start a runtime from within a runtime'");
    println!("3. 解决方案: 将 ToolPolicy.evaluate_approval 改为 async");
    println!();
    println!("=== 建议的 trait 改进 ===");
    println!("// 当前（有问题）:");
    println!("pub trait ToolPolicy: Send + Sync {{");
    println!("    fn evaluate_approval(&self, tool_name: &str, args: &Value) -> Option<ApprovalRequest>;");
    println!("}}");
    println!();
    println!("// 改进后:");
    println!("#[async_trait]");
    println!("pub trait ToolPolicy: Send + Sync {{");
    println!("    async fn evaluate_approval(&self, tool_name: &str, args: &Value) -> Option<ApprovalRequest>;");
    println!("}}");

    Ok(())
}
