//! Multi-Tool — register multiple tools and let the agent choose.
//!
//! Demonstrates registering several tools and shows how the agent decides
//! which tool to use based on the user's request.
//!
//! Run with:
//! ```bash
//! LLM_API_KEY=your-key cargo run --example multi-tool
//! ```

use std::sync::Arc;

use agent_base::{AgentResult, Tool, ToolContext, ToolControlFlow, ToolOutput};
use async_trait::async_trait;
use phi_agent::{
    OpenAiClient, OutputFormat, PhiAgent, PhiAgentConfig, ReasoningEffort, SafetyConfig, base_agent_builder,
    build_system_prompt, create_stdout_renderer,
};
use serde_json::{Value, json};

// ── Tool: System Info ──

struct SystemInfoTool;

#[async_trait]
impl Tool for SystemInfoTool {
    fn name(&self) -> &'static str {
        "system_info"
    }

    fn definition(&self) -> Value {
        json!({
            "name": "system_info",
            "description": "Get current system information (OS, time, working directory)",
            "parameters": { "type": "object", "properties": {}, "required": [] }
        })
    }

    async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let info = format!(
            "OS: {} | Time: {} | CWD: {}",
            std::env::consts::OS,
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| "unknown".into()),
        );
        Ok(ToolOutput { summary: info, control_flow: ToolControlFlow::Continue, ..Default::default() })
    }
}

// ── Tool: Env Var ──

struct EnvVarTool;

#[async_trait]
impl Tool for EnvVarTool {
    fn name(&self) -> &'static str {
        "env_var"
    }

    fn definition(&self) -> Value {
        json!({
            "name": "env_var",
            "description": "Read an environment variable value",
            "parameters": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Environment variable name" }
                },
                "required": ["name"]
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let name = args["name"].as_str().unwrap_or("");
        match std::env::var(name) {
            Ok(val) => Ok(ToolOutput {
                summary: format!("{}={}", name, val),
                control_flow: ToolControlFlow::Continue,
                ..Default::default()
            }),
            Err(_) => Ok(ToolOutput {
                summary: format!("{} is not set", name),
                control_flow: ToolControlFlow::Continue,
                ..Default::default()
            }),
        }
    }
}

// ── Main ──

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let api_key = std::env::var("LLM_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .expect("Set LLM_API_KEY or OPENAI_API_KEY environment variable");
    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "opus".into());
    let base_url = std::env::var("LLM_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());

    let llm_client = Arc::new(OpenAiClient::new(api_key, model.clone(), Some(base_url)));

    // Register multiple tools — the agent picks the right one for each request
    let builder = base_agent_builder(llm_client)
        .system_prompt(build_system_prompt())
        .register_tool(SystemInfoTool)
        .register_tool(EnvVarTool);

    let agent = PhiAgent::build(
        builder,
        PhiAgentConfig {
            model,
            enable_thinking: true,
            thinking_budget: None,
            thinking_effort: ReasoningEffort::Medium,
            safety: SafetyConfig::default(),
        },
    )?;

    let session = agent.create_session().await;
    let mut renderer =
        create_stdout_renderer(&OutputFormat::Terminal { show_thinking: true, show_tool_args: true, color: true });

    println!("Agent with system_info + env_var tools. Try asking about the system!\n");

    agent
        .run_turn(session, "What's my current system info? Also check if HOME is set.", |event| renderer.render(event))
        .await?;

    Ok(())
}
