//! 快速上手 Demo —— 对应 QUICKSTART_CN.md 教程
//!
//! 一个完整的服务器健康检查 Agent，演示：
//!   - Tool 定义（磁盘检查、内存检查、服务重启）
//!   - 审批流程（ToolPolicy + ApprovalHandler）
//!   - Middleware（反幻觉推动）
//!   - 实时事件流
//!   - 多轮 REPL 对话
//!
//! 运行方式：
//!   cp .env.example .env
//!   # 编辑 .env 填入你的 API Key
//!   cargo run --example quickstart_demo

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_base::{
    AgentBuilder, AgentError, AgentResult, ApprovalDecision, ApprovalHandler,
    ApprovalRequest, Middleware, OpenAiClient, PostLlmCtx, RiskLevel, RuntimeEvent, Tool,
    ToolContext, ToolControlFlow, ToolOutput, ToolPolicy,
};
use async_trait::async_trait;
use dotenvy::dotenv;
use serde_json::{json, Value};

// ============================================================================
// 工具定义
// ============================================================================

/// 磁盘检查工具
struct DiskCheckTool;

#[async_trait]
impl Tool for DiskCheckTool {
    fn name(&self) -> &'static str {
        "check_disk"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "check_disk",
                "description": "检查服务器磁盘使用情况。返回已用/总空间和使用百分比。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "要检查的文件系统路径（如 '/'、'/home'、'/var'）"
                        }
                    },
                    "required": ["path"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let path = args["path"].as_str().unwrap_or("/");
        let output = format!(
            "文件系统: {}\n总计: 50G  已用: 32G  可用: 18G  使用率: 64%",
            path
        );
        Ok(ToolOutput {
            summary: output,
            raw: Some(json!({ "path": path, "used_gb": 32, "total_gb": 50, "percent": 64 })),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

/// 内存检查工具
struct MemCheckTool;

#[async_trait]
impl Tool for MemCheckTool {
    fn name(&self) -> &'static str {
        "check_memory"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "check_memory",
                "description": "检查服务器内存使用情况。返回已用/总内存和使用百分比。",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }
        })
    }

    async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        Ok(ToolOutput {
            summary: "总计: 16G  已用: 12G  可用: 4G  使用率: 75%\nSwap: 总计 4G  已用 512M".into(),
            raw: Some(json!({ "total_gb": 16, "used_gb": 12, "percent": 75, "swap_total_gb": 4, "swap_used_gb": 0.5 })),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

/// 服务重启工具（敏感操作，需要审批）
struct RestartServiceTool;

#[async_trait]
impl Tool for RestartServiceTool {
    fn name(&self) -> &'static str {
        "restart_service"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "restart_service",
                "description": "重启指定的系统服务。此操作会导致服务短暂中断，需要人工审批。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "service": {
                            "type": "string",
                            "description": "服务名称（如 nginx、mysql、redis）"
                        }
                    },
                    "required": ["service"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let service = args["service"].as_str().unwrap_or("unknown");
        Ok(ToolOutput {
            summary: format!("服务 '{}' 已成功重启。状态: active (running)", service),
            raw: Some(json!({ "service": service, "status": "restarted", "success": true })),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

// ============================================================================
// 审批：ToolPolicy + ApprovalHandler
// ============================================================================

/// 审批策略：restart_service 需要人工审批，其他工具自动放行
struct HealthCheckPolicy;

#[async_trait]
impl ToolPolicy for HealthCheckPolicy {
    async fn evaluate_approval(
        &self,
        tool_name: &str,
        args: &Value,
    ) -> Option<ApprovalRequest> {
        match tool_name {
            "restart_service" => {
                let service = args
                    .get("service")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                Some(ApprovalRequest {
                    title: "重启服务".into(),
                    message: format!("是否允许重启服务 '{}'？此操作会导致服务短暂中断。", service),
                    risk_level: RiskLevel::Sensitive,
                    action_key: Some(format!("restart:{}", service)),
                    raw: None,
                })
            }
            _ => None,
        }
    }

    fn before_call(&self, _tool_name: &str, _args: &Value, _ctx: &ToolContext) -> AgentResult<()> {
        Ok(())
    }

    fn after_call(
        &self,
        _tool_name: &str,
        _args: &Value,
        _result: &ToolOutput,
        _ctx: &ToolContext,
    ) -> AgentResult<()> {
        Ok(())
    }
}

/// CLI 审批交互
struct CliApproval;

#[async_trait]
impl ApprovalHandler for CliApproval {
    async fn approve(&self, request: ApprovalRequest) -> AgentResult<ApprovalDecision> {
        println!();
        println!("⚠️  审批请求: {}", request.title);
        println!("   风险等级: {:?}", request.risk_level);
        println!("   详情: {}", request.message);

        loop {
            print!("   选择 [y=本次允许 / a=总是允许 / n=拒绝]: ");
            io::stdout()
                .flush()
                .map_err(|e| AgentError::internal(format!("flush stdout failed: {e}")))?;

            let mut input = String::new();
            match io::stdin().read_line(&mut input) {
                Ok(0) => {
                    // stdin EOF（管道模式），默认拒绝
                    println!("   [stdin EOF，默认拒绝]");
                    return Ok(ApprovalDecision::Deny);
                }
                Ok(_) => {}
                Err(e) => {
                    return Err(AgentError::internal(format!("read stdin failed: {e}")));
                }
            }
            match input.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" => return Ok(ApprovalDecision::AllowOnce),
                "a" | "always" => return Ok(ApprovalDecision::AllowAlways),
                "n" | "no" => return Ok(ApprovalDecision::Deny),
                "" => {
                    // 空行（可能来自管道末尾），默认拒绝
                    return Ok(ApprovalDecision::Deny);
                }
                _ => println!("   无效输入，请输入 y / a / n"),
            }
        }
    }
}

// ============================================================================
// Middleware：反幻觉推动
// ============================================================================

/// 当 LLM 有工具可用却不调用、只在"描述它会做什么"时，强制它去调用工具
struct ToolEnforcement {
    max_nudges: usize,
    nudge_count: AtomicUsize,
}

impl ToolEnforcement {
    fn new(max_nudges: usize) -> Self {
        Self {
            max_nudges,
            nudge_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Middleware for ToolEnforcement {
    async fn on_post_llm(&self, ctx: &mut PostLlmCtx) -> AgentResult<()> {
        if ctx.available_tools.is_empty()
            || ctx.is_tool_call
            || ctx.full_text.is_empty()
            || ctx.total_tool_calls > 0
        {
            return Ok(());
        }

        let count = self.nudge_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.max_nudges {
            return Ok(());
        }

        println!(
            "\n[Middleware] 检测到 LLM 未调用工具，推动第 {} 次...",
            count + 1
        );

        ctx.skip_push = true;
        ctx.follow_up_message = Some(
            "你有可用的工具。请直接调用工具获取数据，不要只描述你会做什么。".into(),
        );
        Ok(())
    }
}

// ============================================================================
// 事件打印
// ============================================================================

struct CliEventPrinter {
    assistant_prefix_printed: bool,
}

impl CliEventPrinter {
    fn new() -> Self {
        Self {
            assistant_prefix_printed: false,
        }
    }

    fn handle(&mut self, event: RuntimeEvent) -> AgentResult<()> {
        match event {
            RuntimeEvent::TextDelta { text, .. } => {
                if !self.assistant_prefix_printed {
                    print!("Assistant > ");
                    self.assistant_prefix_printed = true;
                }
                print!("{}", text);
                io::stdout()
                    .flush()
                    .map_err(|e| AgentError::internal(format!("flush failed: {e}")))?;
            }
            RuntimeEvent::ThoughtDelta { text, .. } => {
                print!("\x1b[90m[思考] {} \x1b[0m", text);
                io::stdout()
                    .flush()
                    .map_err(|e| AgentError::internal(format!("flush failed: {e}")))?;
            }
            RuntimeEvent::ToolCallStarted {
                tool_name,
                args_json,
                ..
            } => {
                self.finish();
                println!("[工具调用] {} ({})", tool_name, args_json);
            }
            RuntimeEvent::ToolCallFinished {
                tool_name, summary, ..
            } => {
                self.finish();
                let display = if summary.len() > 300 {
                    format!("{}...", &summary[..300])
                } else {
                    summary.clone()
                };
                println!("[工具完成] {}", tool_name);
                println!("  → {}", display);
            }
            RuntimeEvent::AwaitingApproval { .. } => {
                self.finish();
            }
            RuntimeEvent::RunFinished { .. } => {
                self.finish();
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(&mut self) {
        if self.assistant_prefix_printed {
            println!();
            self.assistant_prefix_printed = false;
        }
    }
}

// ============================================================================
// System Prompt
// ============================================================================

const SYSTEM_PROMPT: &str = r#"你是一个服务器健康检查助手。

你拥有以下工具：
- check_disk: 检查指定路径的磁盘使用情况
- check_memory: 检查内存使用情况
- restart_service: 重启系统服务（需要人工审批）

当用户询问服务器健康状况时，**必须调用工具**获取数据，不要凭空编造数据。
回答要简洁，用要点列表汇报结果。
如果发现使用率过高，主动提醒用户并给出建议。"#;

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> AgentResult<()> {
    dotenv().ok();

    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("DASHSCOPE_API_KEY"))
        .map_err(|_| {
            AgentError::internal(
                "请在 .env 文件中设置 OPENAI_API_KEY 或 DASHSCOPE_API_KEY",
            )
        })?;

    let model = std::env::var("OPENAI_MODEL")
        .or_else(|_| std::env::var("DASHSCOPE_MODEL"))
        .unwrap_or_else(|_| "gpt-4o-mini".to_string());

    let base_url = std::env::var("OPENAI_BASE_URL")
        .or_else(|_| std::env::var("DASHSCOPE_BASE_URL"))
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

    let llm = Arc::new(OpenAiClient::new(api_key, model.clone(), Some(base_url)));

    let runtime = AgentBuilder::new(llm)
        .system_prompt(SYSTEM_PROMPT)
        .enable_thought(false)
        .enable_thinking(false)
        .register_tool(DiskCheckTool)
        .register_tool(MemCheckTool)
        .register_tool(RestartServiceTool)
        .tool_policy(Arc::new(HealthCheckPolicy))
        .approval_handler(Arc::new(CliApproval))
        .middleware(ToolEnforcement::new(3))
        .build()?;

    let mut session_id = runtime.create_session().await;

    println!("╔══════════════════════════════════════════════════════╗");
    println!("║       agent-base 快速上手 Demo (服务器健康检查)       ║");
    println!("╠══════════════════════════════════════════════════════╣");
    println!("║  模型: {:<46} ║", model);
    println!("║                                                      ║");
    println!("║  可用工具:                                           ║");
    println!("║    · check_disk      检查磁盘使用情况                ║");
    println!("║    · check_memory    检查内存使用情况                ║");
    println!("║    · restart_service 重启服务（需审批）              ║");
    println!("║                                                      ║");
    println!("║  试一试:                                             ║");
    println!("║    \"检查一下磁盘\"                                    ║");
    println!("║    \"看看内存够不够用\"                                ║");
    println!("║    \"帮我重启 nginx\"                                  ║");
    println!("║    \"全面检查一下服务器状态\"                          ║");
    println!("║                                                      ║");
    println!("║  命令: exit=退出  reset=重置会话  session=查看历史   ║");
    println!("╚══════════════════════════════════════════════════════╝");
    println!();

    loop {
        print!("User > ");
        io::stdout()
            .flush()
            .map_err(|e| AgentError::internal(format!("flush failed: {e}")))?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| AgentError::internal(format!("read stdin failed: {e}")))?;
        let input = input.trim().to_string();

        if input.is_empty() {
            continue;
        }
        if matches!(input.as_str(), "exit" | "quit") {
            println!("再见！");
            break;
        }
        if input == "reset" {
            session_id = runtime.create_session().await;
            println!("已重置会话\n");
            continue;
        }
        if input == "session" {
            if let Some(session) = runtime.session(&session_id).await {
                println!("\n--- 会话历史 ---");
                for msg in session.chat_messages() {
                    match msg {
                        agent_base::ChatMessage::System { content, .. } => {
                            println!("[系统] {}...", &content[..content.len().min(80)]);
                        }
                        agent_base::ChatMessage::User { content, .. } => {
                            println!("[用户] {}", content);
                        }
                        agent_base::ChatMessage::Assistant {
                            content, tool_calls, ..
                        } => {
                            if let Some(tc) = tool_calls {
                                println!(
                                    "[助手] 工具调用: {:?}",
                                    tc.iter()
                                        .map(|t| format!("{}({})", t.name, t.arguments))
                                        .collect::<Vec<_>>()
                                );
                            } else if let Some(c) = content {
                                let display = if c.len() > 120 {
                                    format!("{}...", &c[..120])
                                } else {
                                    c.clone()
                                };
                                println!("[助手] {}", display);
                            }
                        }
                        agent_base::ChatMessage::Tool {
                            tool_call_id, content, ..
                        } => {
                            let display = if content.len() > 120 {
                                format!("{}...", &content[..120])
                            } else {
                                content.clone()
                            };
                            println!("[工具:{}] {}", tool_call_id, display);
                        }
                    }
                }
                println!("---------------\n");
            }
            continue;
        }

        let mut printer = CliEventPrinter::new();
        match runtime
            .run_turn_with_handler(session_id.clone(), &input, |event| printer.handle(event))
            .await
        {
            Ok(_outcome) => {
                printer.finish();
            }
            Err(e) => {
                printer.finish();
                if e.is_cancelled() {
                    println!("[已取消]");
                } else {
                    eprintln!("[错误] {}", e);
                }
            }
        }
    }

    Ok(())
}