# Getting Started

5 minutes to your first phi-agent.

## Prerequisites

- [Rust](https://rustup.rs) (stable, edition 2024)
- An LLM API key (OpenAI-compatible endpoint)

## 1. Create a project

```bash
cargo new my-agent
cd my-agent
```

## 2. Add dependencies

```bash
cargo add phi-agent
cargo add tokio --features full
cargo add anyhow
```

## 3. Set your API key

```bash
echo 'LLM_API_KEY=sk-your-key-here' > .env
```

## 4. Write the code

```rust
// src/main.rs
use std::sync::Arc;
use phi_agent::{
    base_agent_builder, build_system_prompt, create_stdout_renderer,
    PhiAgent, PhiAgentConfig, OpenAiClient, OutputFormat,
    ReasoningEffort, SafetyConfig,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Resolve API key from environment
    let api_key = std::env::var("LLM_API_KEY")?;
    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "opus".into());
    let base_url = std::env::var("LLM_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".into());

    // Create LLM client
    let llm_client = Arc::new(OpenAiClient::new(api_key, model.clone(), Some(base_url)));

    // Build agent
    let builder = base_agent_builder(llm_client)
        .system_prompt(build_system_prompt());

    let agent = PhiAgent::build(builder, PhiAgentConfig {
        model,
        enable_thinking: true,
        thinking_budget: None,
        thinking_effort: ReasoningEffort::Medium,
        safety: SafetyConfig::default(),
    })?;

    // Run
    let session = agent.create_session().await;
    let renderer = create_stdout_renderer(&OutputFormat::Terminal {
        show_thinking: true, show_tool_args: true, color: true,
    });

    agent.run_turn(session, "Hello, world!", |event| {
        renderer.render(event)
    }).await?;

    Ok(())
}
```

## 5. Run

```bash
cargo run
```

## What's Next

- [Custom Tools](custom-tool.md) — add your own tools to the agent
- [Browser Tools](browser-tools.md) — automate browser interactions
- [Configuration](configuration.md) — understand all config options
- [Examples](/examples/) — runnable examples in the repo
