//! Quickstart Demo — Corresponding to the QUICKSTART.md tutorial
//!
//! A complete server health check Agent demonstrating:
//!   - Tool definitions (disk check, memory check, service restart)
//!   - Approval flow (ToolPolicy + ApprovalHandler)
//!   - Middleware (anti-hallucination nudge)
//!   - Real-time event stream
//!   - Multi-turn REPL conversation
//!
//! How to run:
//!   cp .env.example .env
//!   # Edit .env and fill in your API Key
//!   cargo run --example quickstart_demo

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_base::{
    AgentBuilder, AgentError, AgentEvent, AgentResult, ApprovalDecision, ApprovalHandler,
    ApprovalRequest, Middleware, OpenAiClient, PostLlmCtx, RiskLevel, Tool,
    ToolContext, ToolControlFlow, ToolOutput, ToolPolicy,
};
use async_trait::async_trait;
use dotenvy::dotenv;
use serde_json::{json, Value};

// ============================================================================
// Tool definitions
// ============================================================================

/// Disk check tool
struct DiskCheckTool;

#[async_trait]
impl Tool for DiskCheckTool {
    fn name(&self) -> &'static str {
        "check_disk"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "check_disk",
                "description": "Check server disk usage. Returns used/total space and usage percentage.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Filesystem path to check (e.g. '/', '/home', '/var')"
                        }
                    },
                    "required": ["path"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let path = args["path"].as_str().unwrap_or("/");
        let output = format!(
            "Filesystem: {}\nTotal: 50G  Used: 32G  Available: 18G  Usage: 64%",
            path
        );
        Ok(ToolOutput {
            summary: output,
            raw: Some(json!({ "path": path, "used_gb": 32, "total_gb": 50, "percent": 64 })),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

/// Memory check tool
struct MemCheckTool;

#[async_trait]
impl Tool for MemCheckTool {
    fn name(&self) -> &'static str {
        "check_memory"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "check_memory",
                "description": "Check server memory usage. Returns used/total memory and usage percentage.",
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }
        })
    }

    async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        Ok(ToolOutput {
            summary: "Total: 16G  Used: 12G  Available: 4G  Usage: 75%\nSwap: Total 4G  Used 512M".into(),
            raw: Some(json!({ "total_gb": 16, "used_gb": 12, "percent": 75, "swap_total_gb": 4, "swap_used_gb": 0.5 })),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

/// Service restart tool (sensitive operation, requires approval)
struct RestartServiceTool;

#[async_trait]
impl Tool for RestartServiceTool {
    fn name(&self) -> &'static str {
        "restart_service"
    }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "restart_service",
                "description": "Restart a specified system service. This operation causes a brief service interruption and requires manual approval.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "service": {
                            "type": "string",
                            "description": "Service name (e.g. nginx, mysql, redis)"
                        }
                    },
                    "required": ["service"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let service = args["service"].as_str().unwrap_or("unknown");
        Ok(ToolOutput {
            summary: format!("Service '{}' has been successfully restarted. Status: active (running)", service),
            raw: Some(json!({ "service": service, "status": "restarted", "success": true })),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

// ============================================================================
// Approval: ToolPolicy + ApprovalHandler
// ============================================================================

/// Approval policy: restart_service requires manual approval, other tools are auto-approved
struct HealthCheckPolicy;

#[async_trait]
impl ToolPolicy for HealthCheckPolicy {
    async fn evaluate_approval(
        &self,
        tool_name: &str,
        args: &Value,
    ) -> Option<ApprovalRequest> {
        match tool_name {
            "restart_service" => {
                let service = args
                    .get("service")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                Some(ApprovalRequest {
                    title: "Restart Service".into(),
                    message: format!("Do you want to restart the service '{}'? This will cause a brief service interruption.", service),
                    risk_level: RiskLevel::Sensitive,
                    action_key: Some(format!("restart:{}", service)),
                    raw: None,
                })
            }
            _ => None,
        }
    }

    fn before_call(&self, _tool_name: &str, _args: &Value, _ctx: &ToolContext) -> AgentResult<()> {
        Ok(())
    }

    fn after_call(
        &self,
        _tool_name: &str,
        _args: &Value,
        _result: &ToolOutput,
        _ctx: &ToolContext,
    ) -> AgentResult<()> {
        Ok(())
    }
}

/// CLI approval interaction
struct CliApproval;

#[async_trait]
impl ApprovalHandler for CliApproval {
    async fn approve(&self, request: ApprovalRequest) -> AgentResult<ApprovalDecision> {
        println!();
        println!("⚠️  Approval Request: {}", request.title);
        println!("   Risk Level: {:?}", request.risk_level);
        println!("   Details: {}", request.message);

        loop {
            print!("   Select [y=allow once / a=always allow / n=deny]: ");
            io::stdout()
                .flush()
                .map_err(|e| AgentError::internal(format!("flush stdout failed: {e}")))?;

            let mut input = String::new();
            match io::stdin().read_line(&mut input) {
                Ok(0) => {
                    // stdin EOF (piped mode), default deny
                    println!("   [stdin EOF, default deny]");
                    return Ok(ApprovalDecision::Deny);
                }
                Ok(_) => {}
                Err(e) => {
                    return Err(AgentError::internal(format!("read stdin failed: {e}")));
                }
            }
            match input.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" => return Ok(ApprovalDecision::AllowOnce),
                "a" | "always" => return Ok(ApprovalDecision::AllowAlways),
                "n" | "no" => return Ok(ApprovalDecision::Deny),
                "" => {
                    // empty line (possibly end of pipe), default deny
                    return Ok(ApprovalDecision::Deny);
                }
                _ => println!("   Invalid input, please enter y / a / n"),
            }
        }
    }
}

// ============================================================================
// Middleware: anti-hallucination nudge
// ============================================================================

/// When the LLM has tools available but doesn't call them — only describes what it would do — force it to call the tool
struct ToolEnforcement {
    max_nudges: usize,
    nudge_count: AtomicUsize,
}

impl ToolEnforcement {
    fn new(max_nudges: usize) -> Self {
        Self {
            max_nudges,
            nudge_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Middleware for ToolEnforcement {
    async fn on_post_llm(&self, ctx: &mut PostLlmCtx) -> AgentResult<()> {
        if ctx.available_tools.is_empty()
            || ctx.is_tool_call
            || ctx.full_text.is_empty()
            || ctx.total_tool_calls > 0
        {
            return Ok(());
        }

        let count = self.nudge_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.max_nudges {
            return Ok(());
        }

        println!(
            "\n[Middleware] Detected LLM did not call a tool, nudging (attempt {})...",
            count + 1
        );

        ctx.skip_push = true;
        ctx.follow_up_message = Some(
            "You have tools available. Please call a tool directly to get data, don't just describe what you would do.".into(),
        );
        Ok(())
    }
}

// ============================================================================
// Event printing
// ============================================================================

struct CliEventPrinter {
    assistant_prefix_printed: bool,
}

impl CliEventPrinter {
    fn new() -> Self {
        Self {
            assistant_prefix_printed: false,
        }
    }

    fn handle(&mut self, event: AgentEvent) -> AgentResult<()> {
        match event {
            AgentEvent::TextDelta { text, .. } => {
                if !self.assistant_prefix_printed {
                    print!("Assistant > ");
                    self.assistant_prefix_printed = true;
                }
                print!("{}", text);
                io::stdout()
                    .flush()
                    .map_err(|e| AgentError::internal(format!("flush failed: {e}")))?;
            }
            AgentEvent::ThoughtDelta { text, .. } => {
                print!("\x1b[90m[Thought] {} \x1b[0m", text);
                io::stdout()
                    .flush()
                    .map_err(|e| AgentError::internal(format!("flush failed: {e}")))?;
            }
            AgentEvent::ToolCallStarted {
                tool_name,
                args_json,
                ..
            } => {
                self.finish();
                println!("[Tool Call] {} ({})", tool_name, args_json);
            }
            AgentEvent::ToolCallFinished {
                tool_name, summary, ..
            } => {
                self.finish();
                let display = if summary.len() > 300 {
                    format!("{}...", &summary[..300])
                } else {
                    summary.clone()
                };
                println!("[Tool Done] {}", tool_name);
                println!("  → {}", display);
            }
            AgentEvent::AwaitingApproval { .. } => {
                self.finish();
            }
            AgentEvent::RunFinished { .. } => {
                self.finish();
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(&mut self) {
        if self.assistant_prefix_printed {
            println!();
            self.assistant_prefix_printed = false;
        }
    }
}

// ============================================================================
// System Prompt
// ============================================================================

const SYSTEM_PROMPT: &str = r#"You are a server health check assistant.

You have the following tools:
- check_disk: Check disk usage for a specified path
- check_memory: Check memory usage
- restart_service: Restart a system service (requires manual approval)

When the user asks about server health, **you MUST call a tool** to get data — do not fabricate data.
Keep your answers concise and report results in bullet points.
If you find usage is too high, proactively alert the user and offer suggestions."#;

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> AgentResult<()> {
    dotenv().ok();

    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("DASHSCOPE_API_KEY"))
        .map_err(|_| {
            AgentError::internal(
                "Please set OPENAI_API_KEY or DASHSCOPE_API_KEY in your .env file",
            )
        })?;

    let model = std::env::var("OPENAI_MODEL")
        .or_else(|_| std::env::var("DASHSCOPE_MODEL"))
        .unwrap_or_else(|_| "gpt-4o-mini".to_string());

    let base_url = std::env::var("OPENAI_BASE_URL")
        .or_else(|_| std::env::var("DASHSCOPE_BASE_URL"))
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());

    let llm = Arc::new(OpenAiClient::new(api_key, model.clone(), Some(base_url)));

    let runtime = AgentBuilder::new(llm)
        .system_prompt(SYSTEM_PROMPT)
        .enable_thought(false)
        .enable_thinking(false)
        .register_tool(DiskCheckTool)
        .register_tool(MemCheckTool)
        .register_tool(RestartServiceTool)
        .tool_policy(Arc::new(HealthCheckPolicy))
        .approval_handler(Arc::new(CliApproval))
        .middleware(ToolEnforcement::new(3))
        .build()?;

    let mut session_id = runtime.create_session().await;

    println!("╔══════════════════════════════════════════════════════╗");
    println!("║       agent-base Quickstart Demo (Server Health)     ║");
    println!("╠══════════════════════════════════════════════════════╣");
    println!("║  Model: {:<47} ║", model);
    println!("║                                                      ║");
    println!("║  Available tools:                                    ║");
    println!("║    · check_disk      Check disk usage                ║");
    println!("║    · check_memory    Check memory usage              ║");
    println!("║    · restart_service Restart service (needs approval)║");
    println!("║                                                      ║");
    println!("║  Try saying:                                         ║");
    println!("║    \"Check the disk\"                                  ║");
    println!("║    \"How's the memory?\"                               ║");
    println!("║    \"Restart nginx\"                                   ║");
    println!("║    \"Full server health check\"                       ║");
    println!("║                                                      ║");
    println!("║  Commands: exit=quit  reset=reset session  session=history║");
    println!("╚══════════════════════════════════════════════════════╝");
    println!();

    loop {
        print!("User > ");
        io::stdout()
            .flush()
            .map_err(|e| AgentError::internal(format!("flush failed: {e}")))?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| AgentError::internal(format!("read stdin failed: {e}")))?;
        let input = input.trim().to_string();

        if input.is_empty() {
            continue;
        }
        if matches!(input.as_str(), "exit" | "quit") {
            println!("Goodbye!");
            break;
        }
        if input == "reset" {
            session_id = runtime.create_session().await;
            println!("Session reset\n");
            continue;
        }
        if input == "session" {
            if let Some(session) = runtime.session(&session_id).await {
                println!("\n--- Session History ---");
                for msg in session.chat_messages() {
                    match msg {
                        agent_base::ChatMessage::System { content, .. } => {
                            println!("[System] {}...", &content[..content.len().min(80)]);
                        }
                        agent_base::ChatMessage::User { content, .. } => {
                            println!("[User] {}", content);
                        }
                        agent_base::ChatMessage::Assistant {
                            content, tool_calls, ..
                        } => {
                            if let Some(tc) = tool_calls {
                                println!(
                                    "[Assistant] Tool calls: {:?}",
                                    tc.iter()
                                        .map(|t| format!("{}({})", t.name, t.arguments))
                                        .collect::<Vec<_>>()
                                );
                            } else if let Some(c) = content {
                                let display = if c.len() > 120 {
                                    format!("{}...", &c[..120])
                                } else {
                                    c.clone()
                                };
                                println!("[Assistant] {}", display);
                            }
                        }
                        agent_base::ChatMessage::Tool {
                            tool_call_id, content, ..
                        } => {
                            let display = if content.len() > 120 {
                                format!("{}...", &content[..120])
                            } else {
                                content.clone()
                            };
                            println!("[Tool:{}] {}", tool_call_id, display);
                        }
                    }
                }
                println!("---------------\n");
            }
            continue;
        }

        let mut printer = CliEventPrinter::new();
        match runtime
            .run_turn_with_handler(session_id.clone(), &input, |event| printer.handle(event))
            .await
        {
            Ok(_outcome) => {
                printer.finish();
            }
            Err(e) => {
                printer.finish();
                if e.is_cancelled() {
                    println!("[Cancelled]");
                } else {
                    eprintln!("[Error] {}", e);
                }
            }
        }
    }

    Ok(())
}