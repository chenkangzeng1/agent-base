# Getting Started

5 minutes to your first phi-agent.

## Prerequisites

- [Rust](https://rustup.rs) (stable, edition 2024)
- An LLM API key (OpenAI-compatible endpoint)

## Install

```bash
cargo install phi-agent
```

## Option 1: REPL (recommended)

**1. Create project**

```bash
phi init my-agent
```

**2. Configure API key**

```bash
cd my-agent
cp .env.example .env
```

Edit `.env` with your key:

```
LLM_API_KEY=sk-your-key-here
LLM_BASE_URL=https://api.openai.com/v1
LLM_MODEL=gpt-4o
```

**3. Run**

```bash
cargo run
```

```
phi> What time is it?
🔧 get_time
Current time: 2025-07-30 19:30:00

phi> /exit
```

Open `src/main.rs` — you'll see three parts:

**1. Define a tool** — implement the `Tool` trait:

```rust
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
```

**2. Register the tool** — attach it to the Agent:

```rust
let agent = PhiAgent::build(
    base_agent_builder(llm)
        .system_prompt(build_system_prompt())
        .register_tool(ClockTool),      // ← register here
    PhiAgentConfig { ... },
)?;
```

**3. REPL** — the Agent decides when to call your tool.

Model your own tool after `ClockTool`. See [Custom Tools](custom-tool.md) for more examples.

## Option 2: Library integration

**1. Create project**

```bash
phi init --lib my-agent
```

**2. Configure API key**

```bash
cd my-agent
cp .env.example .env
```

Edit `.env` — same as above.

**3. Run**

```bash
cargo run
```

Open `src/main.rs` — same ClockTool, but runs as a single call instead of a REPL:

```rust
// ClockTool definition (same as Option 1)

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let api_key = std::env::var("LLM_API_KEY")?;
    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o".into());
    let llm = Arc::new(OpenAiClient::new(api_key, model.clone(), std::env::var("LLM_BASE_URL").ok()));

    let agent = PhiAgent::build(
        base_agent_builder(llm)
            .system_prompt(build_system_prompt())
            .register_tool(ClockTool),
        PhiAgentConfig {
            model,
            enable_thinking: true,
            thinking_budget: None,
            thinking_effort: ReasoningEffort::Medium,
            safety: SafetyConfig::default(),
        },
    )?;

    let session = agent.create_session().await;
    let mut renderer = create_stdout_renderer(&OutputFormat::Terminal {
        show_thinking: true, show_tool_args: true, color: true,
    });
    agent.run_turn(session, "What time is it?", |event| renderer.render(event)).await?;
    Ok(())
}
```

Difference from Option 1: no `rustyline`, no REPL loop, just a single `run_turn()` call.
See [Custom Tools](custom-tool.md) for more examples.
