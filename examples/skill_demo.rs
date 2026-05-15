use std::sync::Arc;

use agent_core::{
    AgentBuilder, AgentEvent, AgentResult, OpenAiClient,
    Skill,
    Tool, ToolContext, ToolControlFlow, ToolOutput,
};
use async_trait::async_trait;
use dotenvy::dotenv;
use serde_json::{json, Value};

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
        Ok(ToolOutput {
            summary: format!("{} + {} = {}", a, b, a + b),
            raw: Some(json!({ "result": a + b })),
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
                "description": "计算两个整数之差 (a - b)",
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
        Ok(ToolOutput {
            summary: format!("{} - {} = {}", a, b, a - b),
            raw: Some(json!({ "result": a - b })),
            control_flow: ToolControlFlow::Break,
            truncated: false,
        })
    }
}

struct MathSkill;

impl Skill for MathSkill {
    fn name(&self) -> &'static str {
        "math"
    }

    fn brief_description(&self) -> String {
        "数学计算：支持加减法运算".to_string()
    }

    fn detailed_description(&self) -> String {
        r#"## 数学计算 Skill

### 可用工具
- **add**: 计算两个整数之和。参数: a (第一个加数), b (第二个加数)
- **subtract**: 计算两个整数之差。参数: a (被减数), b (减数)

### 使用说明
- 处理整数运算时使用 add 或 subtract 工具
- 复杂计算可以多次调用工具组合使用
"#.trim().to_string()
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(AddTool),
            Arc::new(SubtractTool),
        ]
    }
}

struct UppercaseTool;

#[async_trait]
impl Tool for UppercaseTool {
    fn name(&self) -> &'static str {
        "uppercase"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "uppercase",
                "description": "将文本转换为大写",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "要转换的文本" }
                    },
                    "required": ["text"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let text = args["text"].as_str().unwrap_or("");
        Ok(ToolOutput {
            summary: text.to_uppercase(),
            raw: Some(json!({ "result": text.to_uppercase() })),
            control_flow: ToolControlFlow::Break,
            truncated: false,
        })
    }
}

struct TextSkill;

impl Skill for TextSkill {
    fn name(&self) -> &'static str {
        "text"
    }

    fn brief_description(&self) -> String {
        "文本处理：支持大小写转换".to_string()
    }

    fn detailed_description(&self) -> String {
        r#"## 文本处理 Skill

### 可用工具
- **uppercase**: 将文本转换为大写。参数: text (要转换的文本)

### 使用说明
- 需要将文本转为大写时使用 uppercase 工具
"#.trim().to_string()
    }

    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(UppercaseTool),
        ]
    }
}

#[tokio::main]
async fn main() {
    dotenv().ok();

    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("DASHSCOPE_API_KEY"))
        .expect("需要设置 OPENAI_API_KEY 或 DASHSCOPE_API_KEY");

    let model = std::env::var("OPENAI_MODEL")
        .or_else(|_| std::env::var("DASHSCOPE_MODEL"))
        .unwrap_or_else(|_| "gpt-4o".to_string());

    let base_url = std::env::var("OPENAI_BASE_URL")
        .or_else(|_| std::env::var("DASHSCOPE_BASE_URL"))
        .ok();

    let client = Arc::new(OpenAiClient::new(api_key, model, base_url));

    let mut runtime = AgentBuilder::new(client)
        .system_prompt("你是一个通用助手，请根据需要使用 Skill 中提供的工具。")
        .register_skill(MathSkill)
        .register_skill(TextSkill)
        .build();

    println!("=== Skills Demo ===");
    println!("已注册 {} 个 Skill:", runtime.skills().len());
    for skill in runtime.skills() {
        println!("  - {}: {}", skill.name(), skill.brief_description());
    }
    println!();

    let session_id = runtime.create_session();

    let user_input = "帮我计算 123 + 456，然后把结果转为大写文本";
    println!("用户: {}", user_input);
    println!("---");

    let (events, _outcome) = runtime
        .run_turn_stream(session_id.clone(), user_input)
        .await
        .expect("执行失败");

    for event in &events {
        match event {
            AgentEvent::TextDelta { text, .. } => print!("{}", text),
            AgentEvent::ToolCallStarted { tool_name, args_json, .. } => {
                println!("\n🔧 调用工具: {} ({})", tool_name, args_json);
            }
            AgentEvent::ToolCallFinished { tool_name, summary, .. } => {
                println!("✅ {} → {}", tool_name, summary);
            }
            AgentEvent::Custom { payload, .. } => {
                if payload.get("type").and_then(Value::as_str) == Some("skill_detail_loaded") {
                    if let Some(skill) = payload.get("skill").and_then(Value::as_str) {
                        println!("📖 加载 Skill 手册: {}", skill);
                    }
                }
            }
            _ => {}
        }
    }

    println!();
    println!("---");
    println!("完成!");
}
