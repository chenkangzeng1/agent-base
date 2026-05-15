use std::io::{self, Write};
use std::sync::Arc;

use agent_core::{
    AgentBuilder, AgentError, AgentEvent, AgentResult, ApprovalDecision, ApprovalHandler,
    ApprovalRequest, OpenAiClient, RiskLevel, Tool, ToolContext, ToolControlFlow, ToolOutput,
    ToolPolicy,
};
use async_trait::async_trait;
use dotenvy::dotenv;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// 算术工具
// ---------------------------------------------------------------------------

struct AddTool;

#[async_trait]
impl Tool for AddTool {
    fn name(&self) -> &'static str {
        "add"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "add",
                "description": "计算两个整数之和",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "a": { "type": "integer", "description": "第一个加数" },
                        "b": { "type": "integer", "description": "第二个加数" }
                    },
                    "required": ["a", "b"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let a = args["a"].as_i64().unwrap_or(0);
        let b = args["b"].as_i64().unwrap_or(0);
        let result = a + b;
        Ok(ToolOutput {
            summary: format!("{} + {} = {}", a, b, result),
            raw: Some(json!({ "result": result })),
            control_flow: ToolControlFlow::Break,
            truncated: false,
        })
    }
}

struct SubtractTool;

#[async_trait]
impl Tool for SubtractTool {
    fn name(&self) -> &'static str {
        "subtract"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "subtract",
                "description": "计算两个整数之差（a - b）",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "a": { "type": "integer", "description": "被减数" },
                        "b": { "type": "integer", "description": "减数" }
                    },
                    "required": ["a", "b"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let a = args["a"].as_i64().unwrap_or(0);
        let b = args["b"].as_i64().unwrap_or(0);
        let result = a - b;
        Ok(ToolOutput {
            summary: format!("{} - {} = {}", a, b, result),
            raw: Some(json!({ "result": result })),
            control_flow: ToolControlFlow::Break,
            truncated: false,
        })
    }
}

struct MultiplyTool;

#[async_trait]
impl Tool for MultiplyTool {
    fn name(&self) -> &'static str {
        "multiply"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "multiply",
                "description": "计算两个整数之积",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "a": { "type": "integer", "description": "乘数" },
                        "b": { "type": "integer", "description": "被乘数" }
                    },
                    "required": ["a", "b"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let a = args["a"].as_i64().unwrap_or(0);
        let b = args["b"].as_i64().unwrap_or(0);
        let result = a * b;
        Ok(ToolOutput {
            summary: format!("{} × {} = {}", a, b, result),
            raw: Some(json!({ "result": result })),
            control_flow: ToolControlFlow::Break,
            truncated: false,
        })
    }
}

struct DivideTool;

#[async_trait]
impl Tool for DivideTool {
    fn name(&self) -> &'static str {
        "divide"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "divide",
                "description": "计算两个整数之商（a ÷ b），返回商和余数",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "a": { "type": "integer", "description": "被除数" },
                        "b": { "type": "integer", "description": "除数" }
                    },
                    "required": ["a", "b"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let a = args["a"].as_i64().unwrap_or(0);
        let b = args["b"].as_i64().unwrap_or(0);
        if b == 0 {
            return Ok(ToolOutput {
                summary: "错误：除数不能为 0".to_string(),
                raw: Some(json!({ "error": "division by zero" })),
                control_flow: ToolControlFlow::Break,
                truncated: false,
            });
        }
        let quotient = a / b;
        let remainder = a % b;
        Ok(ToolOutput {
            summary: format!("{} ÷ {} = {}（余 {}）", a, b, quotient, remainder),
            raw: Some(json!({ "quotient": quotient, "remainder": remainder })),
            control_flow: ToolControlFlow::Break,
            truncated: false,
        })
    }
}

// ---------------------------------------------------------------------------
// CLI 审批处理器
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
struct CliApprovalHandler;

#[async_trait]
impl ApprovalHandler for CliApprovalHandler {
    async fn approve(&self, request: ApprovalRequest) -> AgentResult<ApprovalDecision> {
        println!();
        println!("[审批请求] {}", request.title);
        println!("  风险等级: {:?}", request.risk_level);
        println!("  内容: {}", request.message);

        loop {
            print!("  选择 [y=允许 / a=总是允许 / n=拒绝]: ");
            io::stdout().flush().unwrap();

            let mut input = String::new();
            io::stdin()
                .read_line(&mut input)
                .map_err(|e| AgentError::internal(format!("读取输入失败: {e}")))?;
            match input.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" => return Ok(ApprovalDecision::AllowOnce),
                "a" | "always" => return Ok(ApprovalDecision::AllowAlways),
                "n" | "no" => return Ok(ApprovalDecision::Deny),
                _ => println!("  无效输入，请输入 y / a / n"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 事件打印器
// ---------------------------------------------------------------------------

struct EventPrinter;

impl EventPrinter {
    fn handle(event: AgentEvent) -> AgentResult<()> {
        match event {
            AgentEvent::TextDelta { text, .. } => {
                print!("{}", text);
                io::stdout().flush().unwrap();
            }
            AgentEvent::ThoughtDelta { text, .. } => {
                print!("[正在思考]：\x1b[90m{} \x1b[0m", text);
                println!();
                io::stdout().flush().unwrap();
            }
            AgentEvent::ToolCallStarted {
                tool_name, args_json, ..
            } => {
                println!();
                println!("[工具调用] {} (参数: {})", tool_name, args_json);
            }
            AgentEvent::ToolCallFinished {
                tool_name, summary, ..
            } => {
                println!("[工具完成] {} -> {}", tool_name, summary);
            }
            AgentEvent::AwaitingApproval { request, .. } => {
                println!(
                    "[等待审批] {} (风险: {:?})",
                    request.title, request.risk_level
                );
            }
            AgentEvent::RunFinished { .. } => {
                println!();
                println!("[运行完成]");
            }
            AgentEvent::Custom { payload, .. } => {
                println!("[自定义事件] {}", payload);
            }
            AgentEvent::Checkpoint { .. } => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 工具审批策略
// ---------------------------------------------------------------------------

struct ArithmeticToolPolicy;

impl ToolPolicy for ArithmeticToolPolicy {
    fn evaluate_approval(
        &self,
        tool_name: &str,
        _args: &Value,
        _args_json: &str,
    ) -> Option<ApprovalRequest> {
        if tool_name == "divide" {
            return Some(ApprovalRequest {
                title: "除法操作".to_string(),
                message: "是否允许执行除法计算？".to_string(),
                action_key: Some("divide".to_string()),
                risk_level: RiskLevel::Safe,
                raw: None,
            });
        }
        None
    }

    fn on_pre_call(&self, _tool_name: &str, _args: &Value, _ctx: &ToolContext) {}

    fn on_post_call(
        &self,
        _tool_name: &str,
        _args: &Value,
        _result: &ToolOutput,
        _ctx: &ToolContext,
    ) {
    }
}

// ---------------------------------------------------------------------------
// 主函数 - REPL 入口
// ---------------------------------------------------------------------------

const SYSTEM_PROMPT: &str = r#"你是一个算术助手，可以帮助用户完成各种算术运算。

你可以使用的工具包括：
- add: 计算两个整数之和
- subtract: 计算两个整数之差
- multiply: 计算两个整数之积
- divide: 计算两个整数之商

请根据用户的请求选择合适的工具来回答问题。如果用户的问题涉及多个步骤，
可以分步调用工具。每次完成一个计算后，向用户说明计算结果。"#;

#[tokio::main]
async fn main() -> AgentResult<()> {
    dotenv().ok();

    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("DASHSCOPE_API_KEY"))
        .map_err(|_| AgentError::internal("请设置 OPENAI_API_KEY 或 DASHSCOPE_API_KEY 环境变量"))?;

    let model = std::env::var("OPENAI_MODEL")
        .or_else(|_| std::env::var("DASHSCOPE_MODEL"))
        .unwrap_or_else(|_| "gpt-4o-mini".to_string());

    let base_url = std::env::var("OPENAI_BASE_URL")
        .or_else(|_| std::env::var("DASHSCOPE_BASE_URL"))
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

    let llm_client = Arc::new(OpenAiClient::new(api_key, model.clone(), Some(base_url)));

    let mut runtime = AgentBuilder::new(llm_client)
        .system_prompt(SYSTEM_PROMPT)
        .enable_thought(false)
        .enable_thinking(false)
        .register_tool(AddTool)
        .register_tool(SubtractTool)
        .register_tool(MultiplyTool)
        .register_tool(DivideTool)
        .tool_policy(Arc::new(ArithmeticToolPolicy))
        .approval_handler(Arc::new(CliApprovalHandler))
        .build();

    let mut session_id = runtime.create_session();

    println!("=== agent-core REPL (算术 Demo) ===");
    println!("模型: {}", model);
    println!("输入 'exit' 退出, 'reset' 重建会话");
    println!();

    loop {
        print!("> ");
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| AgentError::internal(format!("读取输入失败: {e}")))?;
        let input = input.trim().to_string();

        if input.is_empty() {
            continue;
        }
        if matches!(input.as_str(), "exit" | "quit") {
            break;
        }
        if input == "reset" {
            session_id = runtime.create_session();
            println!("已创建新会话");
            continue;
        }

        match runtime
            .run_turn_with_handler(session_id.clone(), &input, |event| EventPrinter::handle(event))
            .await
        {
            Ok(_outcome) => {}
            Err(e) => {
                if e.is_cancelled() {
                    println!("已取消");
                } else {
                    println!("错误: {}", e);
                }
            }
        }
    }

    Ok(())
}
