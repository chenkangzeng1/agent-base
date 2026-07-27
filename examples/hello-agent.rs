//! Hello Agent — the simplest phi-agent example.
//!
//! This demonstrates the minimum code needed to create an agent,
//! run a turn, and stream the response to the terminal.
//!
//! Run with:
//! ```bash
//! LLM_API_KEY=your-key cargo run --example hello-agent
//! ```

use std::sync::Arc;

use phi_agent::{
    OpenAiClient, OutputFormat, PhiAgent, PhiAgentConfig, ReasoningEffort, SafetyConfig, base_agent_builder,
    build_system_prompt, create_stdout_renderer,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Resolve API key from environment
    let api_key = std::env::var("LLM_API_KEY")
        .or_else(|_| std::env::var("OPENAI_API_KEY"))
        .expect("Set LLM_API_KEY or OPENAI_API_KEY environment variable");

    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "opus".into());
    let base_url = std::env::var("LLM_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());

    // 2. Create LLM client
    let llm_client = Arc::new(OpenAiClient::new(api_key, model.clone(), Some(base_url)));

    // 3. Build agent
    let builder = base_agent_builder(llm_client).system_prompt(build_system_prompt());

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

    // 4. Run one turn
    let session = agent.create_session().await;
    let mut renderer =
        create_stdout_renderer(&OutputFormat::Terminal { show_thinking: true, show_tool_args: true, color: true });

    agent.run_turn(session, "Hello! Introduce yourself in one sentence.", |event| renderer.render(event)).await?;

    Ok(())
}
