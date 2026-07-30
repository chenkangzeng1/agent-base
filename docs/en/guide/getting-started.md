# Getting Started

5 minutes to your first phi-agent.

## Prerequisites

- [Rust](https://rustup.rs) (stable, edition 2024)
- An LLM API key (OpenAI-compatible endpoint)

## Install

```bash
cargo install phi-agent
```

## Option 1: Scaffold a project (recommended)

Use `phi init` to generate a complete project with an example tool and REPL:

```bash
phi init my-agent
cd my-agent
cp .env.example .env   # edit with your API key
cargo run
```

```
phi> What time is it?
🔧 get_time
 Current time: 2025-07-30 19:30:00

phi> /exit
```

Open `src/main.rs` — you'll see the full `ClockTool` implementation. Write your own tool the same way, register it with the agent, done.

See [Custom Tools](custom-tool.md) for details.

## Option 2: Library integration

Add phi-agent as a library to an existing project:

```bash
cargo add phi-agent tokio --features full anyhow dotenvy async-trait serde_json chrono
```

Full example `src/main.rs`:

```rust
use phi_agent::{
    base_agent_builder, build_system_prompt,
    PhiAgent, PhiAgentConfig, OpenAiClient,
    SafetyConfig, ReasoningEffort,
    OutputFormat, create_stdout_renderer,
    AgentResult, Tool, ToolContext, ToolControlFlow, ToolOutput,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

// 1. Define your tool
struct ClockTool;

#[async_trait]
impl Tool for ClockTool {
    fn name(&self) -> &'static str { "get_time" }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "get_time",
                "description": "Get the current date and time",
                "parameters": { "type": "object", "properties": {} }
            }
        })
    }

    async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        Ok(ToolOutput {
            summary: format!("Current time: {}", now),
            control_flow: ToolControlFlow::Continue,
            raw: None, truncation: None,
        })
    }
}

// 2. Register tool, build Agent
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o".into());
    let llm = Arc::new(OpenAiClient::new(
        std::env::var("LLM_API_KEY")?,
        model.clone(),
        std::env::var("LLM_BASE_URL").ok(),
    ));

    let agent = PhiAgent::build(
        base_agent_builder(llm)
            .system_prompt(build_system_prompt())
            .register_tool(ClockTool),      // register your tool
        PhiAgentConfig {
            model,
            enable_thinking: true,
            thinking_budget: None,
            thinking_effort: ReasoningEffort::Medium,
            safety: SafetyConfig::default(),
        },
    )?;

    // 3. Run
    let session = agent.create_session().await;
    let mut renderer = create_stdout_renderer(&OutputFormat::Terminal {
        show_thinking: true, show_tool_args: true, color: true,
    });
    agent.run_turn(session, "What time is it?", |event| renderer.render(event)).await?;
    Ok(())
}
```

Three steps: Define Tool → Register → Run. See [Custom Tools](custom-tool.md) for more examples.