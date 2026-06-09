//! Plan Replan + Step Retry + LLM 决策示例
//!
//! 模拟场景：查询本月销售数据。
//!   - query_database 内部维护状态，每次调用自动尝试不同策略
//!   - Recovery::retry(2) 允许步骤级重试 2 次（共 3 次尝试）
//!   - 3 种方案全部失败 → step 失败 → plan 终止
//!   - 应用层捕获 RunOutcome::Failed → 收集上下文 (已完成/失败/剩余步骤)
//!   - 询问 LLM 决策: 完全重新规划 or 部分调整
//!   - 根据 LLM 决策分两条路径继续执行
//!
//! 关键设计：
//!   - 所有工具从一开始就注册，生成 plan 时全部可见
//!   - Tool 内部维护状态，每次被调用自动换方案，模拟 "LLM 尝试不同方法"
//!   - Recovery::retry 实现步骤级重试
//!   - 应用层：通过事件追踪步骤状态 → LLM 决策 → full/partial replan
//!
//! 运行方式：
//!   cp .env.example .env   # 填写 OPENAI_API_KEY
//!   cargo run --example plan_replan_retry_demo

use std::sync::{Arc, Mutex};

use agent_base::{
    AgentBuilder, AgentError, AgentResult, ChatMessage, LlmClient, LlmPlanGenerator,
    OpenAiClient, PlanConfig, PlanGenerator, Recovery, ResponseFormat, RunOutcome,
    RuntimeEvent, Tool, ToolContext, ToolControlFlow, ToolOutput,
};
use async_trait::async_trait;
use dotenvy::dotenv;
use serde_json::{json, Value};

// ============================================================================
// PlanStrategy —— LLM 决策结果
// ============================================================================

#[derive(Debug)]
enum PlanStrategy {
    /// 完全重新规划：丢弃当前 plan 所有步骤，重新生成
    Full,
    /// 部分调整：保留已完成步骤的结果，只重新规划剩余步骤
    Partial,
}

// ============================================================================
// decide_strategy —— 询问 LLM 应该用哪种策略
// ============================================================================

async fn decide_strategy(
    llm_client: &dyn LlmClient,
    objective: &str,
    completed: &[(String, String)],
    failed_step: &str,
    failed_error: &str,
    remaining: &[(String, String)],
) -> AgentResult<PlanStrategy> {
    let completed_text = if completed.is_empty() {
        "(无)".to_string()
    } else {
        completed
            .iter()
            .enumerate()
            .map(|(i, (id, desc))| format!("  {}. [{}] {}", i + 1, id, desc))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let remaining_text = if remaining.is_empty() {
        "(无)".to_string()
    } else {
        remaining
            .iter()
            .enumerate()
            .map(|(i, (id, desc))| format!("  {}. [{}] {}", i + 1, id, desc))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let prompt = format!(
        "你是一个计划决策助手。当前任务执行情况如下:\n\
         \n\
         原始目标: {objective}\n\
         \n\
         已成功完成的步骤:\n{completed_text}\n\
         \n\
         失败的步骤: {failed_step}\n\
         失败原因: {failed_error}\n\
         \n\
         尚未执行的步骤:\n{remaining_text}\n\
         \n\
         请判断: 是需要完全重新规划 (full) 还是只需要调整剩余步骤 (partial)？\n\
         \n\
         判断依据:\n\
         - full: 失败原因说明整体思路有问题，需要推翻重建\n\
         - partial: 已完成步骤仍然有效，只需调整/替换剩余步骤即可\n\
         \n\
         请以 JSON 格式回答: {{\"decision\": \"full\"}} 或 {{\"decision\": \"partial\"}}"
    );

    let messages = vec![ChatMessage::User {
        content: prompt.to_string(),
        images: vec![],
    }];

    let strategy = llm_client
        .chat(&messages, &[], None, Some(&ResponseFormat::JsonObject))
        .await?;

    // 从 OpenAI 风格响应中提取 content
    let content = strategy["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or(r#"{"decision": "full"}"#);

    let parsed: Value = serde_json::from_str(content).unwrap_or(json!({"decision": "full"}));
    let decision_str = parsed["decision"].as_str().unwrap_or("full");

    match decision_str {
        "partial" => Ok(PlanStrategy::Partial),
        _ => Ok(PlanStrategy::Full),
    }
}

// ============================================================================
// DatabaseState: 共享状态
// ============================================================================

struct DatabaseState {
    attempt: usize,       // query_database 的调用次数计数器
    db_fixed: bool,       // 数据库是否已修复
    force_fail: bool,     // 强制查询失败 (确保首次 plan 必然失败，触发 replan)
}

// ============================================================================
// query_database — 每次调用自动尝试不同方案，全部失败后返回 Err
// ============================================================================

struct QueryDatabaseTool {
    state: Arc<Mutex<DatabaseState>>,
}

impl QueryDatabaseTool {
    fn try_approach(&self, attempt: usize, _query: &str) -> AgentResult<ToolOutput> {
        match attempt {
            0 => Err(AgentError::plan_execution(
                "方案A(直连查询): 数据库连接超时，请检查网络或重建连接",
            )),
            1 => Err(AgentError::plan_execution(
                "方案B(缓存查询): 缓存未命中，数据已过期",
            )),
            2 => Err(AgentError::plan_execution(
                "方案C(备用数据源): 备用数据源不可用，所有自动重试方案已耗尽",
            )),
            _ => Err(AgentError::plan_execution(
                "所有自动重试方案已耗尽，建议重建数据库连接后重试",
            )),
        }
    }
}

#[async_trait]
impl Tool for QueryDatabaseTool {
    fn name(&self) -> &'static str {
        "query_database"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "query_database",
                "description": "查询数据库（支持 SQL 查询）",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "SQL 查询语句" }
                    },
                    "required": ["query"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let query = args["query"].as_str().unwrap_or("unknown");
        let mut state = self.state.lock().unwrap();

        // force_fail: 确保首次 plan 中即使 db 已修复，查询也失败
        // 这样能保证触发 replan + LLM 决策流程
        if state.force_fail {
            let attempt = state.attempt;
            state.attempt += 1;
            drop(state);
            return self.try_approach(attempt, query);
        }

        if state.db_fixed {
            return Ok(ToolOutput {
                summary: format!("查询成功: {query} → 本月销售额 1,234,567 元"),
                raw: Some(json!({"query": query, "result": "success", "amount": 1234567})),
                control_flow: ToolControlFlow::Continue,
                truncation: None,
            });
        }

        let attempt = state.attempt;
        state.attempt += 1;
        drop(state);

        self.try_approach(attempt, query)
    }
}

// ============================================================================
// rebuild_database_connection — 修复数据库连接
// ============================================================================

struct RebuildDatabaseTool {
    state: Arc<Mutex<DatabaseState>>,
}

#[async_trait]
impl Tool for RebuildDatabaseTool {
    fn name(&self) -> &'static str {
        "rebuild_database_connection"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "rebuild_database_connection",
                "description": "重建数据库连接并刷新索引，解决连接超时问题",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            }
        })
    }

    async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let mut state = self.state.lock().unwrap();
        state.db_fixed = true;
        Ok(ToolOutput {
            summary: "数据库连接已重建，索引已刷新，现在可以正常查询".to_string(),
            raw: Some(json!({"status": "rebuilt"})),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

// ============================================================================
// check_database_status — 诊断工具
// ============================================================================

struct CheckDatabaseStatusTool;

#[async_trait]
impl Tool for CheckDatabaseStatusTool {
    fn name(&self) -> &'static str {
        "check_database_status"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "check_database_status",
                "description": "检查数据库连接状态和健康度",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            }
        })
    }

    async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        Ok(ToolOutput {
            summary: "数据库状态: 连接超时，建议重建连接".to_string(),
            raw: Some(json!({"status": "timeout", "suggestion": "rebuild"})),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

// ============================================================================
// 主流程
// ============================================================================

#[tokio::main]
async fn main() -> AgentResult<()> {
    dotenv().ok();

    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| AgentError::internal("请设置 OPENAI_API_KEY 环境变量"))?;
    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let base_url = std::env::var("OPENAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║  Plan Replan + LLM 自主决策 (full vs partial)            ║");
    println!("╚════════════════════════════════════════════════════════════╝\n");
    println!("场景: 查询本月销售数据 (数据库连接超时)");
    println!("  流程:");
    println!("    1. LLM 生成 Plan → 执行");
    println!("    2. 步骤失败 → 收集上下文(已完成/失败/剩余步骤)");
    println!("    3. 询问 LLM: full replan or partial?");
    println!("    4. 根据 LLM 决策走不同分支\n");
    println!("Model: {model}\n");

    let llm_client = Arc::new(OpenAiClient::new(api_key, model, Some(base_url)));

    let db_state = Arc::new(Mutex::new(DatabaseState {
        attempt: 0,
        db_fixed: false,
        force_fail: true, // 确保首次查询必然失败
    }));

    // 构建 Runtime，注册全部工具（从一开始就可见）
    let mut runtime = AgentBuilder::new(llm_client.clone())
        .system_prompt("你是一个助手，有工具可以用。当被要求执行任务时，必须实际调用工具来完成，不要只描述你要做什么。")
        .register_tool(QueryDatabaseTool {
            state: db_state.clone(),
        })
        .register_tool(RebuildDatabaseTool {
            state: db_state.clone(),
        })
        .register_tool(CheckDatabaseStatusTool)
        .build()?;

    let _session_id = runtime.create_session().await; // 预留, 实际每轮都会创建新 session

    // 获取全部工具定义 —— 生成 plan 时全部暴露
    let tool_defs: Vec<Value> = runtime.tools_mut().definitions();

    let objective = "查询本月销售数据";

    const MAX_REPLAN_COUNT: usize = 3;

    let mut replan_count = 0;
    let mut current_objective = objective.to_string();

    loop {
        replan_count += 1;
        println!("═══════════════════════════════════════════════════════");
        println!("  第 {replan_count} 轮: generate + execute");
        println!("═══════════════════════════════════════════════════════\n");
        println!("  目标: {current_objective}\n");

        // 可用工具（全量）
        println!("  可用工具 ({}):", tool_defs.len());
        for td in tool_defs.iter() {
            println!("     - {}", td["function"]["name"].as_str().unwrap_or("-"));
        }
        println!();

        // ----------------------------------------------------------------
        // 1. 生成 Plan
        // ----------------------------------------------------------------
        let generator = LlmPlanGenerator::new(llm_client.clone()).with_max_steps(3);
        let plan = generator
            .generate_plan(&current_objective, "", &tool_defs, None)
            .await
            .map_err(|e| AgentError::plan_generation(e.to_string()))?;

        // 记录当前 plan 的所有步骤描述 (用于 LLM 决策时展示剩余步骤)
        let all_step_descs: Vec<(String, String)> = plan
            .all_steps()
            .map(|s| (s.id.clone(), s.description.clone()))
            .collect();

        println!("  📋 生成的 Plan ({} 步):", plan.total_steps());
        for (id, desc) in &all_step_descs {
            println!("     - {id}: {desc}");
        }
        println!();

        // 为每轮创建新 session，避免旧上下文影响 LLM 判断
        let session_id = runtime.create_session().await;

        // ----------------------------------------------------------------
        // 2. 执行 Plan + 追踪步骤状态
        // ----------------------------------------------------------------
        println!("  --- 执行 ---\n");

        // 通过事件回调追踪步骤状态 (Arc<Mutex> 共享给闭包和外部)
        let completed_steps = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let failed_step_info: Arc<Mutex<Option<(String, String, String)>>> =
            Arc::new(Mutex::new(None));
        let failed_step_id_for_print = Arc::new(Mutex::new(String::new()));
        let all_step_descs_shared = Arc::new(all_step_descs.clone());

        let outcome = {
            let completed = completed_steps.clone();
            let failed_info = failed_step_info.clone();
            let failed_id = failed_step_id_for_print.clone();
            let all_descs = all_step_descs_shared.clone();
            runtime
                .run_plan(
                    session_id.clone(),
                    plan,
                    PlanConfig::new()
                        .recovery(Recovery::retry(2)),
                    move |event| {
                        match &event {
                            RuntimeEvent::PlanStepStarted {
                                step_id,
                                step_description,
                                ..
                            } => {
                                println!("  ▶️  {} - {}", step_id, step_description);
                            }
                            RuntimeEvent::PlanStepCompleted {
                                step_id,
                                success,
                                result,
                                ..
                            } => {
                                let icon = if *success { "✅" } else { "❌" };
                                println!(
                                    "  {}  {} → {}\n",
                                    icon,
                                    step_id,
                                    result.as_deref().unwrap_or("-")
                                );
                                if *success {
                                    if let Some(desc) = all_descs.iter().find_map(
                                        |(id, d)| if id == step_id { Some(d) } else { None },
                                    ) {
                                        completed
                                            .lock()
                                            .unwrap()
                                            .push((step_id.clone(), desc.clone()));
                                    }
                                } else {
                                    *failed_id.lock().unwrap() = step_id.clone();
                                    if let Some(desc) = all_descs.iter().find_map(
                                        |(id, d)| if id == step_id { Some(d) } else { None },
                                    ) {
                                        *failed_info.lock().unwrap() = Some((
                                            step_id.clone(),
                                            desc.clone(),
                                            result.clone().unwrap_or_default(),
                                        ));
                                    }
                                }
                            }
                            _ => {}
                        }
                        Ok(())
                    },
                )
                .await
        };

        // ----------------------------------------------------------------
        // 3. 判断结果
        // ----------------------------------------------------------------
        match outcome {
            Ok(RunOutcome::Completed) => {
                println!("═══════════════════════════════════════════════════════");
                if replan_count == 1 {
                    println!("  ✅ 一次完成！");
                } else {
                    println!(
                        "  ✅ 完成！（{} 轮，首轮 step 级重试用尽后 replan）",
                        replan_count
                    );
                }
                println!("═══════════════════════════════════════════════════════\n");
                break;
            }
            Ok(RunOutcome::Failed { error }) => {
                let e = error.trim();
                println!(
                    "═══════════════════════════════════════════════════════"
                );
                println!("  ⚠️  第 {replan_count} 轮失败: {e}");
                println!("═══════════════════════════════════════════════════════\n");

                if replan_count >= MAX_REPLAN_COUNT {
                    println!("  ❌ 已达最大 replan 次数 ({MAX_REPLAN_COUNT})，终止");
                    break;
                }

                // 首次失败后，关闭强制失败模式，让后续 replan 能成功
                db_state.lock().unwrap().force_fail = false;

                // --------------------------------------------------------
                // 3a. 收集上下文，计算剩余步骤
                // --------------------------------------------------------
                let completed_steps = std::mem::take(&mut *completed_steps.lock().unwrap());
                let failed_step_info = failed_step_info.lock().unwrap().take();
                let failed_id = failed_step_id_for_print.lock().unwrap().clone();
                let remaining_steps: Vec<(String, String)> = all_step_descs
                    .iter()
                    .filter(|(id, _)| {
                        *id != failed_id
                            && !completed_steps.iter().any(|(cid, _)| cid == id)
                    })
                    .map(|(id, d)| (id.clone(), d.clone()))
                    .collect();

                println!("  📊 上下文收集:");
                println!(
                    "     已完成 ({}):",
                    completed_steps.len()
                );
                for (id, desc) in &completed_steps {
                    println!("       ✅ [{}] {}", id, desc);
                }
                println!("     失败: [{}]", failed_id);
                println!(
                    "     剩余 ({}):",
                    remaining_steps.len()
                );
                for (id, desc) in &remaining_steps {
                    println!("       ⏳ [{}] {}", id, desc);
                }
                println!();

                // --------------------------------------------------------
                // 3b. 询问 LLM 决策
                // --------------------------------------------------------
                let failed_desc = failed_step_info
                    .as_ref()
                    .map(|(id, desc, _)| format!("[{}] {}", id, desc))
                    .unwrap_or(failed_id.clone());
                let failed_error = failed_step_info
                    .as_ref()
                    .map(|(_, _, err)| err.clone())
                    .unwrap_or(e.to_string());

                println!("  🤔 询问 LLM 决策...");
                let strategy = decide_strategy(
                    llm_client.as_ref(),
                    objective,
                    &completed_steps,
                    &failed_desc,
                    &failed_error,
                    &remaining_steps,
                )
                .await?;
                println!("  🧠 LLM 决策: {:?}\n", strategy);

                // --------------------------------------------------------
                // 3c. 根据决策构造新的 objective
                // --------------------------------------------------------
                current_objective = match strategy {
                    PlanStrategy::Full => {
                        println!("  🔄 完全重新规划 (full replan)\n");
                        format!(
                            "{} (注意: 上一次查询失败 — {})",
                            objective, e
                        )
                    }
                    PlanStrategy::Partial => {
                        println!("  🔄 部分调整 (partial replan)\n");
                        // Partial replan: 直接构造确定性 plan 步骤 + 用 executor 执行
                        // 不走 agentic 模式，避免 LLM 不调用工具（qwen-flash 不稳定）
                        let plan = {
                            let mut plan = agent_base::types::ExecutionPlan::new("partial-plan", "查询本月销售数据");
                            let step = agent_base::types::PlanStep {
                                id: "step-1".to_string(),
                                description: "查询本月销售数据".to_string(),
                                payload: json!({
                                    "tool_name": "query_database",
                                    "args": {
                                        "query": "SELECT * FROM sales WHERE month = CURRENT_MONTH"
                                    }
                                }),
                                dependencies: vec![],
                                status: agent_base::types::StepStatus::Pending,
                                result: None,
                            };
                            plan.phases.push(agent_base::types::PlanPhase::new("phase-1", "查询", vec![step]));
                            plan
                        };
                        println!("  📋 构造确定性 Plan (1 步，通过 executor 执行): query_database\n");

                        let session_id = runtime.create_session().await;
                        let executor = Arc::new(runtime.create_step_executor());
                        println!("  --- 执行 (deterministic) ---\n");
                        let outcome = runtime
                            .run_plan(
                                session_id,
                                plan,
                                PlanConfig::new().with_executor(executor),
                                |event| {
                                    if let RuntimeEvent::PlanStepCompleted { step_id, success, result, .. } = &event {
                                        let icon = if *success { "✅" } else { "❌" };
                                        println!("  {}  {} → {}\n", icon, step_id, result.as_deref().unwrap_or("-"));
                                    }
                                    Ok(())
                                },
                            )
                            .await;
                        match outcome {
                            Ok(RunOutcome::Completed) => {
                                println!("═══════════════════════════════════════════════════════");
                                println!("  ✅ 完成！（2 轮，partial replan 确定性执行成功）");
                                println!("═══════════════════════════════════════════════════════\n");
                            }
                            _ => {
                                println!("  ❌ partial replan 失败: {:?}", outcome);
                            }
                        }
                        break;
                    }
                };
            }
            _ => {
                println!("  ❌ 意外结果: {:?}", outcome);
                break;
            }
        }
    }

    Ok(())
}