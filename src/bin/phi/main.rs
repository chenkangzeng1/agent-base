mod approval;
mod args;
mod init;
mod tools;

use std::sync::Arc;

use anyhow::Result;
use args::{CliArgs, MetricsCmd, OutputFormatArg, SubCommand};
use clap::Parser;
use phi_agent::config::resolve_llm_config;
use phi_agent::render::{OutputFormat, create_stdout_renderer};
use phi_agent::{ApprovalMode, AutoApprovalHandler};
use phi_agent::{
    OpenAiClient, PhiAgent, RunOutcome, SafetyConfig, SessionContext, TurnFactMiddleware, TurnToolLimitMiddleware,
    base_agent_builder, build_system_prompt, save_turn_log,
};
use phi_telemetry::{self, SessionOutcome, list_all_metrics, load_metrics, save_metrics};

use approval::CliApprovalHandler;
use tools::LocalShellTool;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let args = CliArgs::parse();

    // Handle subcommands (no agent needed)
    if let Some(cmd) = &args.command {
        match cmd {
            SubCommand::Init { name, lib } => return init::run(name, *lib),
            SubCommand::Metrics { cmd } => return handle_metrics(cmd, &args),
        }
    }

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
        max_turns: args.max_turns,
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
        .apply_if(args.thinking_budget, |b, budget| b.thinking_budget(budget))
        .apply_if(args.max_turns, |b, n| b.execution_max_turns(n));

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
        // Set up telemetry
        let node_id = std::env::var("PHI_NODE_ID").unwrap_or_else(|_| default_node_id());
        let metrics_enabled = std::env::var("PHI_METRICS_ENABLED")
            .map(|v| {
                let v = v.to_lowercase();
                !matches!(v.as_str(), "false" | "0" | "no" | "off" | "")
            })
            .unwrap_or(true);

        let mut telemetry = if metrics_enabled {
            Some(phi_telemetry::init_telemetry(
                agent.runtime(),
                session_id_str.clone(),
                node_id,
                llm_config.model.clone(),
            ))
        } else {
            None
        };

        let (result, run_outcome) = run_one_shot(&agent, &agent_session_id, &session_ctx, &query, &output_format).await;

        // Finalize and save metrics
        if let Some(handle) = &mut telemetry {
            handle.shutdown().await;
            let session = handle.session.read().await;
            let mut session = session.clone();
            session.finalize(phi_telemetry::types::run_outcome_to_session_outcome(&run_outcome));
            let _ = save_metrics(&session, &session_ctx.session_dir);
        }

        result?;
        Ok(())
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
) -> (Result<()>, RunOutcome) {
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

    let run_outcome = match &result {
        Ok(outcome) => outcome.clone(),
        Err(_) => RunOutcome::Failed { error: "agent error".to_string() },
    };

    cancel_handle.abort();
    let _ = renderer.finish_turn();

    let _ = save_turn_log(session_ctx, 1, &turn_events, query);

    if matches!(format, OutputFormat::Json) {
        let session_info = serde_json::json!({
            "type": "session_info",
            "session_id": session_ctx.session_id,
            "is_new_session": session_ctx.is_new_session,
        });
        if let Ok(json) = serde_json::to_string(&session_info) {
            println!("{}", json);
        }
    }

    match &result {
        Ok(_) => {
            tracing::info!(duration_ms = turn_start.elapsed().as_millis() as u64, "one-shot completed");
            (Ok(()), run_outcome)
        },
        Err(err) => {
            tracing::error!(error = %err, "one-shot failed");
            if matches!(format, OutputFormat::Terminal { .. }) {
                eprintln!("\n❌ Error: {}", err);
            }
            (Err(anyhow::anyhow!("{}", err)), run_outcome)
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

    let node_id = std::env::var("PHI_NODE_ID").unwrap_or_else(|_| default_node_id());
    let metrics_enabled = std::env::var("PHI_METRICS_ENABLED")
        .map(|v| {
            let v = v.to_lowercase();
            !matches!(v.as_str(), "false" | "0" | "no" | "off" | "")
        })
        .unwrap_or(true);

    let mut telemetry = if metrics_enabled {
        Some(phi_telemetry::init_telemetry(
            agent.runtime(),
            session_ctx.session_id.clone(),
            node_id,
            agent.config.model.clone(),
        ))
    } else {
        None
    };

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
        if input == "tools" {
            let tools = agent.list_tools().await;
            if matches!(format, OutputFormat::Terminal { .. }) {
                println!();
                if tools.is_empty() {
                    println!("  (no tools registered)");
                } else {
                    println!("  Registered tools ({}):\n", tools.len());
                    for (name, desc) in &tools {
                        println!("  \x1b[1m{}\x1b[0m — {}", name, desc);
                    }
                }
                println!();
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

                // Save metrics incrementally
                if let Some(ref handle) = telemetry {
                    let session = handle.session.read().await;
                    let _ = save_metrics(&session, &session_ctx.session_dir);
                }

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

                // Save metrics on error too
                if let Some(ref handle) = telemetry {
                    let session = handle.session.read().await;
                    let _ = save_metrics(&session, &session_ctx.session_dir);
                }

                tracing::error!(error = %err, turn = turn_number, "agent turn failed");
                if matches!(format, OutputFormat::Terminal { .. }) {
                    eprintln!("\n❌ Error: {}", err);
                }
            },
        }
    }

    // Finalize metrics on session end
    if let Some(handle) = &mut telemetry {
        handle.shutdown().await;
        let session = handle.session.read().await;
        let mut session = session.clone();
        session.finalize(SessionOutcome::Completed);
        let _ = save_metrics(&session, &session_ctx.session_dir);
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
    println!("║  Commands: exit/quit | reset | tools              ║");
    println!("╚═══════════════════════════════════════════════════╝");
    println!();
}

/// Default node_id: phi-{current_dir_name}, or phi-unknown.
fn default_node_id() -> String {
    let dir = std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "unknown".to_string());
    format!("phi-{}", dir)
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() > max_chars {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}...", truncated)
    } else {
        s.to_string()
    }
}

// ── Metrics commands ──

fn handle_metrics(cmd: &MetricsCmd, args: &CliArgs) -> Result<()> {
    let log_dir = args.log_dir.replace("~", &std::env::var("HOME").unwrap_or_default());
    let log_dir_path = std::path::PathBuf::from(&log_dir);

    match cmd {
        MetricsCmd::List => {
            let summaries = list_all_metrics(&log_dir_path)?;
            if summaries.is_empty() {
                println!("No sessions found.");
                return Ok(());
            }

            println!("  {:<30} {:<22} {:>6} {:>10}  Outcome", "Session", "Node", "Turns", "Chars");
            println!("  {}", "-".repeat(80));

            for s in &summaries {
                let label = if let Some(ref product) = s.product {
                    format!("{} ({})", s.session_id, product)
                } else {
                    s.session_id.clone()
                };

                let outcome_icon = match s.outcome {
                    SessionOutcome::Completed => "✅ completed",
                    SessionOutcome::Failed => "❌ failed",
                    SessionOutcome::Cancelled => "⏹️ cancelled",
                    SessionOutcome::MaxTurns => "⚠️ max_turns",
                };

                println!(
                    "  {:<30} {:<22} {:>6} {:>10}  {}",
                    truncate_str(&label, 29),
                    if s.node_id.is_empty() { "-" } else { &s.node_id },
                    s.total_turns,
                    format_number(s.total_chars),
                    outcome_icon,
                );
            }
            println!("\n  {} session(s)", summaries.len());
        },

        MetricsCmd::Show { session_id } => {
            let session_dir = log_dir_path.join("sessions").join(session_id);
            if !session_dir.exists() {
                eprintln!("Session '{}' not found.", session_id);
                return Ok(());
            }

            let metrics = load_metrics(&session_dir)?;
            print_session_detail(&metrics, session_id);
        },

        MetricsCmd::Last => {
            let summaries = list_all_metrics(&log_dir_path)?;
            match summaries.first() {
                Some(summary) => {
                    let session_dir = log_dir_path.join("sessions").join(&summary.session_id);
                    let metrics = load_metrics(&session_dir)?;
                    print_session_detail(&metrics, &summary.session_id);
                },
                None => {
                    println!("No sessions found.");
                },
            }
        },
    }

    Ok(())
}

fn print_session_detail(metrics: &phi_agent::SessionMetrics, session_id: &str) {
    println!();
    println!("  Session:    {}", session_id);
    // Node: always show (default_node_id ensures it's never empty)
    println!("  Node:       {}", metrics.node_id);
    println!("  Model:      {}", metrics.model);

    // Product info from custom
    if let Some(product) = metrics.custom.get("product").and_then(|v| v.as_str()) {
        let role = metrics.custom.get("role").and_then(|v| v.as_str()).map(|r| format!(" ({})", r)).unwrap_or_default();
        println!("  Product:    {}{}", product, role);
    }

    println!("  Turns:      {}", metrics.total_turns);
    println!(
        "  Duration:   {}s (avg {}s/turn, P50 {}s, P95 {}s, P99 {}s)",
        metrics.total_duration_ms / 1000,
        metrics.avg_turn_ms / 1000,
        metrics.p50_turn_ms / 1000,
        metrics.p95_turn_ms / 1000,
        metrics.p99_turn_ms / 1000,
    );
    println!("  ─────────────────────────────────────────");
    println!("  Chars:      {}", format_number(metrics.total_chars));
    println!("  ─────────────────────────────────────────");
    println!(
        "  LLM:        {}s ({}%)",
        metrics.total_llm_ms / 1000,
        (metrics.total_llm_ms * 100).checked_div(metrics.total_duration_ms).unwrap_or(0)
    );
    println!(
        "  Tool:       {}s ({}%)",
        metrics.total_tool_ms / 1000,
        (metrics.total_tool_ms * 100).checked_div(metrics.total_duration_ms).unwrap_or(0)
    );
    if !metrics.tool_breakdown.is_empty() {
        let tools: Vec<String> =
            metrics.tool_breakdown.iter().map(|(name, count)| format!("{}({})", name, count)).collect();
        println!("  Tools:      {}", tools.join(", "));
    }
    println!("  ─────────────────────────────────────────");
    let outcome_icon = match metrics.outcome {
        SessionOutcome::Completed => "✅ completed",
        SessionOutcome::Failed => "❌ failed",
        SessionOutcome::Cancelled => "⏹️ cancelled",
        SessionOutcome::MaxTurns => "⚠️ max_turns",
    };
    println!("  Outcome:    {}", outcome_icon);
    println!("  Errors:     {}", metrics.error_count);
    if metrics.total_plan_updates > 0 || metrics.total_approvals > 0 {
        println!("  Plans:      {} update(s), {} approval(s)", metrics.total_plan_updates, metrics.total_approvals);
    }

    if !metrics.turns.is_empty() {
        println!();
        println!("  Turn breakdown:");
        for turn in &metrics.turns {
            let tools_str = if turn.tools_used.is_empty() {
                "text-only".to_string()
            } else {
                format!("tools[{}]", turn.tools_used.join(", "))
            };
            let outcome_icon = match turn.outcome {
                phi_telemetry::TurnOutcome::Completed => "✅",
                phi_telemetry::TurnOutcome::ToolCalls => "🔧",
                phi_telemetry::TurnOutcome::Error => "❌",
                phi_telemetry::TurnOutcome::Cancelled => "⏹️",
                phi_telemetry::TurnOutcome::MaxTurns => "⚠️",
            };
            println!(
                "  #{:<3} {:>4}s  TTFT {:>4}ms  {:<30} {}",
                turn.turn_number,
                turn.duration_ms / 1000,
                turn.time_to_first_token_ms,
                truncate_str(&tools_str, 29),
                outcome_icon,
            );
        }
    }
    println!();
}

fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
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
