use phi_agent::{
    base_agent_builder, build_system_prompt,
    PhiAgent, PhiAgentConfig, OpenAiClient,
    SafetyConfig, ReasoningEffort,
    OutputFormat, create_stdout_renderer,
};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let llm = Arc::new(OpenAiClient::new(
        std::env::var("LLM_API_KEY")?,
        std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o".into()),
        std::env::var("LLM_BASE_URL").ok(),
    ));

    let agent = PhiAgent::build(
        base_agent_builder(llm).system_prompt(build_system_prompt()),
        PhiAgentConfig {
            model: std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o".into()),
            enable_thinking: true,
            thinking_budget: None,
            thinking_effort: ReasoningEffort::Medium,
            safety: SafetyConfig::default(),
        },
    )?;

    let session = agent.create_session().await;
    let mut renderer = create_stdout_renderer(&OutputFormat::Terminal {
        show_thinking: true,
        show_tool_args: true,
        color: true,
    });

    agent.run_turn(session, "Hello, who are you?", |event| renderer.render(event)).await?;
    Ok(())
}
