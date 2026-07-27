use agent_base::ReasoningEffort;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "phi",
    about = "phi — General-purpose AI Agent CLI tool",
    version,
    long_about = "phi — Drive local dev tasks via natural language.\n\n\
                  Supports interactive mode and one-shot mode."
)]
pub struct CliArgs {
    /// One-shot query (one-shot mode). If provided, runs the query and exits.
    /// If omitted, enters interactive REPL mode.
    #[arg(value_name = "QUERY")]
    pub query: Option<String>,

    // ── Output control ──
    /// Output format
    #[arg(long, value_enum, default_value = "terminal")]
    pub format: OutputFormatArg,

    /// Hide AI thinking process
    #[arg(long, default_value = "false")]
    pub no_thinking: bool,

    /// Thinking token budget
    #[arg(long)]
    pub thinking_budget: Option<u64>,

    /// Thinking effort (low/medium/high/xhigh)
    #[arg(long, value_enum, default_value = "medium")]
    pub thinking_effort: ReasoningEffortArg,

    /// Hide tool argument details
    #[arg(long, default_value = "false")]
    pub no_tool_args: bool,

    /// Disable terminal colors
    #[arg(long, default_value = "false")]
    pub no_color: bool,

    // ── Approval control ──
    /// Auto-approve all operations (skip confirmation)
    #[arg(long, short = 'y', default_value = "false")]
    pub auto_approve: bool,

    // ── Session control ──
    /// Session ID (for session persistence)
    #[arg(long, env = "PHI_SESSION_ID")]
    pub session_id: Option<String>,

    // ── Model config ──
    /// LLM model name
    #[arg(long)]
    pub model: Option<String>,

    /// LLM API base URL
    #[arg(long)]
    pub base_url: Option<String>,

    // ── Logging control ──
    /// Log directory
    #[arg(long, default_value = "~/.phi-agent")]
    pub log_dir: String,

    /// Log level
    #[arg(long, default_value = "info")]
    pub log_level: String,

    /// Disable file logging
    #[arg(long, default_value = "false")]
    pub no_log: bool,

    // ── Safety limits ──
    /// Max tool calls per turn
    #[arg(long)]
    pub max_tool_calls: Option<usize>,

    /// Max consecutive failures for the same tool
    #[arg(long)]
    pub max_failures: Option<usize>,

    // ── Tool config ──
    /// Shell command timeout (milliseconds)
    #[arg(long, default_value = "30000")]
    pub shell_timeout_ms: u64,
}

#[derive(Clone, Debug, clap::ValueEnum)]
pub enum OutputFormatArg {
    /// Rich terminal output
    Terminal,
    /// One JSON object per line
    Json,
    /// No output
    Quiet,
}

#[derive(Clone, Debug, clap::ValueEnum)]
pub enum ReasoningEffortArg {
    Low,
    Medium,
    High,
    Xhigh,
}

impl From<ReasoningEffortArg> for ReasoningEffort {
    fn from(arg: ReasoningEffortArg) -> Self {
        match arg {
            ReasoningEffortArg::Low => ReasoningEffort::Low,
            ReasoningEffortArg::Medium => ReasoningEffort::Medium,
            ReasoningEffortArg::High => ReasoningEffort::High,
            ReasoningEffortArg::Xhigh => ReasoningEffort::XHigh,
        }
    }
}
