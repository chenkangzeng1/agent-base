use anyhow::Result;
use std::fs;
use std::path::Path;

const ENV_EXAMPLE: &str = r#"# phi-agent LLM configuration
LLM_API_KEY=sk-your-key-here
LLM_BASE_URL=https://api.openai.com/v1
LLM_MODEL=gpt-4o
"#;

const MAIN_RS: &str = r#"use phi_agent::{
    base_agent_builder, build_system_prompt,
    PhiAgent, PhiAgentConfig, OpenAiClient,
    SafetyConfig, ReasoningEffort,
    OutputFormat, create_stdout_renderer,
};
use rustyline::DefaultEditor;
use std::sync::Arc;

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
        base_agent_builder(llm).system_prompt(build_system_prompt()),
        PhiAgentConfig {
            model,
            enable_thinking: true,
            thinking_budget: None,
            thinking_effort: ReasoningEffort::Medium,
            safety: SafetyConfig::default(),
        },
    )?;

    let mut rl = DefaultEditor::new()?;
    let mut renderer = create_stdout_renderer(&OutputFormat::Terminal {
        show_thinking: true,
        show_tool_args: true,
        color: true,
    });

    println!("phi-agent REPL — type /exit to quit\n");
    loop {
        let line = rl.readline("phi> ")?;
        let input = line.trim().to_string();
        if input.is_empty() { continue; }
        if input == "/exit" { break; }
        rl.add_history_entry(&input)?;

        let session = agent.create_session().await;
        agent.run_turn(session, &input, |event| renderer.render(event)).await?;
        println!();
    }

    Ok(())
}
"#;

pub fn run(name: &str) -> Result<()> {
    let dir = Path::new(name);
    let project_name = dir.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(name);

    if dir.exists() {
        anyhow::bail!("directory '{}' already exists", name);
    }

    fs::create_dir_all(dir.join("src"))?;

    // Cargo.toml
    fs::write(
        dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{}"
version = "0.1.0"
edition = "2021"

[dependencies]
phi-agent = "{}"
tokio = {{ version = "1", features = ["full"] }}
anyhow = "1"
dotenvy = "0.15"
rustyline = "15"
"#,
            project_name,
            env!("CARGO_PKG_VERSION")
        ),
    )?;

    // .env.example
    fs::write(dir.join(".env.example"), ENV_EXAMPLE)?;

    // src/main.rs
    fs::write(dir.join("src").join("main.rs"), MAIN_RS)?;

    println!("✅ Created project: {}", name);
    println!();
    println!("Next steps:");
    println!("  cd {}", name);
    println!("  cp .env.example .env   # edit with your API key");
    println!("  cargo run");

    Ok(())
}
