mod approval;
mod args;
mod tools;

use std::sync::Arc;

use anyhow::Result;
use args::{CliArgs, OutputFormatArg};
use clap::Parser;
use phi_agent::config::resolve_llm_config;
use phi_agent::render::{OutputFormat, create_stdout_renderer};
use phi_agent::{ApprovalMode, AutoApprovalHandler};
use phi_agent::{
    OpenAiClient, PhiAgent, SafetyConfig, SessionContext, TurnFactMiddleware, TurnToolLimitMiddleware,
    base_agent_builder, build_system_prompt, save_turn_log,
};

use approval::CliApprovalHandler;
use tools::LocalShellTool;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let args = CliArgs::parse();

    // 1. Resolve log directory
    let log_dir = args.log_dir.replace("~", &std::env::var("HOME").unwrap_or_default());
    let log_dir_path = std::path::PathBuf::from(&log_dir);

    // 2. Clean up expired sessions
    if !args.no_log {
        match phi_agent::session::cleanup_expired_sessions(&log_dir_path, 7) {
            Ok(count) => {
                if count > 0 {
                    eprintln!("[phi] cleaned up {} expired session(s)", count);
                }
            },
            Err(e) => {
                eprintln!("[phi] warning: failed to cleanup sessions: {}", e);
            },
        }
    }

    // 3. Resolve session
    let session_ctx = phi_agent::session::resolve_session(args.session_id.as_deref(), &log_dir_path)?;
    let session_id_str = session_ctx.session_id.clone();
    let is_new_session = session_ctx.is_new_session;

    // 4. Initialize logging
    if !args.no_log {
        init_logging(&session_ctx, &args.log_level).await?;
    }

    tracing::info!(
        session_id = %session_id_str,
        is_new = is_new_session,
        format = ?args.format,
        "phi starting"
    );

    // 5. Resolve LLM config
    let llm_config = resolve_llm_config(args.model.as_deref(), args.base_url.as_deref())?;

    // 6. Create LLM client
    let llm_client = Arc::new(OpenAiClient::new(
        llm_config.api_key.clone(),
        llm_config.model.clone(),
        Some(llm_config.base_url.clone()),
    ));

    // 7. Build system prompt
    let system_prompt = build_system_prompt();

    // 8. Approval handler
    let approval_handler: Arc<dyn phi_agent::ApprovalHandler> = if args.auto_approve {
        Arc::new(AutoApprovalHandler::new(ApprovalMode::Auto))
    } else {
        Arc::new(CliApprovalHandler::new())
    };

    // 9. Safety config
    let safety_config = SafetyConfig {
        max_tool_calls_per_turn: args.max_tool_calls.unwrap_or(64),
        max_consecutive_failures: args.max_failures.unwrap_or(3),
    };

    // 10. Output format
    let output_format = match args.format {
        OutputFormatArg::Terminal => OutputFormat::Terminal {
            show_thinking: !args.no_thinking,
            show_tool_args: !args.no_tool_args,
            color: !args.no_color,
        },
        OutputFormatArg::Json => OutputFormat::Json,
        OutputFormatArg::Quiet => OutputFormat::Quiet,
    };

    // 11. PhiAgent config
    let agent_config = phi_agent::PhiAgentConfig {
        model: llm_config.model.clone(),
        enable_thinking: !args.no_thinking,
        thinking_budget: args.thinking_budget,
        thinking_effort: args.thinking_effort.clone().into(),
        safety: safety_config.clone(),
    };

    // 12. Assemble builder — register tools here
    let builder = base_agent_builder(llm_client)
        .system_prompt(system_prompt)
        .register_tool(LocalShellTool::new(args.shell_timeout_ms))
        .register_tool(
            phi_agent::UpdatePlanTool::new().with_description(
                "Create or update a task plan to show the user a checklist with progress. \
                This is a presentation protocol.\n\n\
                [When to Use]\n\
                - Complex tasks (usually 3+ steps): call update_plan first to show the plan, \
                  then execute step by step.\n\
                - Simple tasks, Q&A, one-shot operations: do NOT call — handle directly.\n\n\
                [Requirements]\n\
                - Must provide an objective when creating a plan for the first time.\n\
                - plan is a full snapshot, not an incremental patch.\n\
                - At most one step can be in_progress at a time.\n\
                - Step text should be human-readable task descriptions only.\n\n\
                [Update Conventions]\n\
                - Update status promptly as you progress: pending → in_progress → completed.\n\
                - If blocked, explain the reason honestly in the explanation field."
                    .to_string(),
            ),
        )
        .approval_handler(approval_handler)
        .middleware(TurnFactMiddleware::new())
        .middleware(TurnToolLimitMiddleware::from_config(&safety_config))
        .apply_if(args.thinking_budget, |b, budget| b.thinking_budget(budget));

    // 13. Build agent
    let agent = PhiAgent::build(builder, agent_config)?;
    let agent_session_id = agent.create_session().await;

    tracing::info!(
        agent_session_id = %agent_session_id.id,
        session_id = %session_id_str,
        model = %llm_config.model,
        "session created"
    );

    // 14. Run
    if let Some(query) = args.query {
        run_one_shot(&agent, &agent_session_id, &session_ctx, &query, &output_format).await
    } else {
        run_repl(&agent, &agent_session_id, &session_ctx, &output_format).await
    }
}

// ── One-shot mode ──

async fn run_one_shot(
    agent: &PhiAgent,
    agent_session_id: &phi_agent::SessionId,
    session_ctx: &SessionContext,
    query: &str,
    format: &OutputFormat,
) -> Result<()> {
    let turn_start = std::time::Instant::now();
    tracing::debug!(input = %truncate_str(query, 80), "one-shot started");

    let mut renderer = create_stdout_renderer(format);
    let mut turn_events: Vec<phi_agent::RuntimeEvent> = Vec::new();

    // Ctrl+C cancellation
    let cancel_agent = agent.clone();
    let cancel_handle = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel_agent.cancel();
        }
    });

    let result = agent
        .run_turn(agent_session_id.clone(), query, |event| {
            turn_events.push(event.clone());
            renderer.render(event)
        })
        .await;

    cancel_handle.abort();
    renderer.finish_turn()?;

    save_turn_log(session_ctx, 1, &turn_events, query)?;

    if matches!(format, OutputFormat::Json) {
        let session_info = serde_json::json!({
            "type": "session_info",
            "session_id": session_ctx.session_id,
            "is_new_session": session_ctx.is_new_session,
        });
        println!("{}", serde_json::to_string(&session_info)?);
    }

    match result {
        Ok(_) => {
            tracing::info!(duration_ms = turn_start.elapsed().as_millis() as u64, "one-shot completed");
            Ok(())
        },
        Err(err) => {
            tracing::error!(error = %err, "one-shot failed");
            if matches!(format, OutputFormat::Terminal { .. }) {
                eprintln!("\n❌ Error: {}", err);
            }
            Err(err.into())
        },
    }
}

// ── REPL mode ──

async fn run_repl(
    agent: &PhiAgent,
    agent_session_id: &phi_agent::SessionId,
    session_ctx: &SessionContext,
    format: &OutputFormat,
) -> Result<()> {
    if matches!(format, OutputFormat::Terminal { .. }) {
        print_welcome_banner(agent, session_ctx);
    }

    let mut agent_session_id = agent_session_id.clone();
    let mut turn_number: u32 = 0;

    let mut rl = rustyline::Editor::<(), rustyline::history::FileHistory>::new()?;
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let history_path = std::path::PathBuf::from(home).join(".phi-agent").join("history");
    let _ = rl.load_history(&history_path);
    let prompt = format!("\n{}Phi > {}", "\x1b[1m", "\x1b[0m");

    loop {
        let input = match rl.readline(&prompt) {
            Ok(line) => {
                let trimmed = line.trim().to_string();
                if !trimmed.is_empty() {
                    let _ = rl.add_history_entry(&trimmed);
                }
                trimmed
            },
            Err(rustyline::error::ReadlineError::Eof) => break,
            Err(rustyline::error::ReadlineError::Interrupted) => continue,
            Err(_) => break,
        };

        if input.is_empty() {
            continue;
        }
        if matches!(input.as_str(), "exit" | "quit") {
            tracing::info!("user exit");
            break;
        }
        if input == "reset" {
            agent_session_id = agent.create_session().await;
            turn_number = 0;
            tracing::info!(new_session_id = %agent_session_id.id, "session reset");
            if matches!(format, OutputFormat::Terminal { .. }) {
                println!("\n✅ New session created");
            }
            continue;
        }

        let _ = rl.save_history(&history_path);

        turn_number += 1;
        let turn_start = std::time::Instant::now();
        tracing::debug!(turn = turn_number, input = %truncate_str(&input, 80), "turn started");

        let mut renderer = create_stdout_renderer(format);
        let mut turn_events: Vec<phi_agent::RuntimeEvent> = Vec::new();

        let cancel_agent = agent.clone();
        let cancel_handle = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                cancel_agent.cancel();
            }
        });

        match agent
            .run_turn(agent_session_id.clone(), &input, |event| {
                turn_events.push(event.clone());
                renderer.render(event)
            })
            .await
        {
            Ok(_) => {
                cancel_handle.abort();
                renderer.finish_turn()?;

                let is_cancelled = agent.is_cancelled();
                save_turn_log(session_ctx, turn_number, &turn_events, &input)?;

                if is_cancelled {
                    tracing::info!(turn = turn_number, "turn cancelled by user");
                } else {
                    tracing::info!(
                        turn = turn_number,
                        duration_ms = turn_start.elapsed().as_millis() as u64,
                        "turn completed"
                    );
                }
            },
            Err(err) => {
                cancel_handle.abort();
                renderer.finish_turn()?;

                save_turn_log(session_ctx, turn_number, &turn_events, &input)?;

                tracing::error!(error = %err, turn = turn_number, "agent turn failed");
                if matches!(format, OutputFormat::Terminal { .. }) {
                    eprintln!("\n❌ Error: {}", err);
                }
            },
        }
    }

    Ok(())
}

// ── Helpers ──

fn print_welcome_banner(agent: &PhiAgent, session_ctx: &SessionContext) {
    println!();
    println!("╔═══════════════════════════════════════════════════╗");
    println!("║  \x1b[1mphi\x1b[0m — General-purpose AI Agent CLI                 ║");
    println!("║                                                   ║");
    println!("║  Model: {:<42}║", if agent.config.model.is_empty() { "default" } else { &agent.config.model });
    println!("║  Session: {:<40}║", session_ctx.session_id);
    if session_ctx.is_new_session {
        println!("║  Status: New session                                ║");
    } else {
        println!("║  Status: Reusing session                            ║");
    }
    println!("║                                                   ║");
    println!("║  Commands: exit/quit | reset                       ║");
    println!("╚═══════════════════════════════════════════════════╝");
    println!();
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() > max_chars {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}...", truncated)
    } else {
        s.to_string()
    }
}

/// Initialize logging: write to file only, no console output.
async fn init_logging(session_ctx: &SessionContext, log_level: &str) -> Result<()> {
    use log_core::{LogCoreLayer, LogLevel};
    use tracing_subscriber::prelude::*;

    let session_log_path = session_ctx.log_path();

    let level = match log_level {
        "debug" => LogLevel::Debug,
        "info" => LogLevel::Info,
        "warn" => LogLevel::Warn,
        "error" => LogLevel::Error,
        _ => LogLevel::Info,
    };

    let layer = LogCoreLayer::file(session_log_path.to_str().unwrap_or("phi.log"), level).await?;

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
        )
        .with(layer)
        .init();

    tracing::info!(path = %session_log_path.display(), "logging initialized");

    Ok(())
}
