use std::io::{self, Write};

use agent_base::{AgentResult, PlanStepStatus, RuntimeEvent};

use crate::render::EventRenderer;

/// Rich terminal renderer — colors, emoji, formatted output.
///
/// Streams AI responses in real-time, displays tool calls with icons, and
/// shows turn summaries including duration and tool call count.
pub struct TerminalRenderer {
    show_thinking: bool,
    show_tool_args: bool,
    color: bool,
    writer: Box<dyn Write + Send>,
    tool_call_count: u32,
    turn_start: Option<std::time::Instant>,
    last_assistant_text: String,
    last_was_thought: bool,
}

impl TerminalRenderer {
    /// Create a new terminal renderer.
    ///
    /// - `show_thinking` — display the LLM's chain-of-thought
    /// - `show_tool_args` — display tool call arguments inline
    /// - `color` — enable ANSI color codes
    /// - `writer` — output destination (usually stdout, can be a WebSocket, etc.)
    pub fn new(show_thinking: bool, show_tool_args: bool, color: bool, writer: Box<dyn Write + Send>) -> Self {
        Self {
            show_thinking,
            show_tool_args,
            color,
            writer,
            tool_call_count: 0,
            turn_start: None,
            last_assistant_text: String::new(),
            last_was_thought: false,
        }
    }

    pub fn stdout(show_thinking: bool, show_tool_args: bool, color: bool) -> Self {
        Self::new(show_thinking, show_tool_args, color, Box::new(io::stdout()))
    }

    fn green(&self, s: &str) -> String {
        if self.color { format!("\x1b[32m{}\x1b[0m", s) } else { s.to_string() }
    }

    fn dim(&self, s: &str) -> String {
        if self.color { format!("\x1b[2m{}\x1b[0m", s) } else { s.to_string() }
    }

    fn bold(&self, s: &str) -> String {
        if self.color { format!("\x1b[1m{}\x1b[0m", s) } else { s.to_string() }
    }

    fn yellow(&self, s: &str) -> String {
        if self.color { format!("\x1b[33m{}\x1b[0m", s) } else { s.to_string() }
    }

    fn subtle(&self, s: &str) -> String {
        if self.color { format!("\x1b[90m{}\x1b[0m", s) } else { s.to_string() }
    }

    fn write_line(&mut self, s: &str) -> AgentResult<()> {
        writeln!(self.writer, "{}", s).map_err(|e| agent_base::AgentError::internal(format!("write error: {e}")))?;
        self.writer.flush().map_err(|e| agent_base::AgentError::internal(format!("flush error: {e}")))?;
        Ok(())
    }

    /// Write without newline — for streaming text fragments
    fn write_text(&mut self, s: &str) -> AgentResult<()> {
        write!(self.writer, "{}", s).map_err(|e| agent_base::AgentError::internal(format!("write error: {e}")))?;
        self.writer.flush().map_err(|e| agent_base::AgentError::internal(format!("flush error: {e}")))?;
        Ok(())
    }
}

impl EventRenderer for TerminalRenderer {
    fn render(&mut self, event: RuntimeEvent) -> AgentResult<()> {
        if self.turn_start.is_none() {
            self.turn_start = Some(std::time::Instant::now());
        }

        match &event {
            RuntimeEvent::ThoughtDelta { text, .. } => {
                if self.show_thinking {
                    self.write_text(&self.dim(text))?;
                }
                self.last_was_thought = true;
            },
            RuntimeEvent::TextDelta { text, .. } => {
                if self.last_was_thought {
                    let _ = writeln!(self.writer);
                    self.last_was_thought = false;
                }
                self.last_assistant_text.push_str(text);
                self.write_text(text)?;
            },
            RuntimeEvent::ToolCallStarted { tool_name, args_json, .. } => {
                self.last_was_thought = false;
                self.tool_call_count += 1;
                if self.show_tool_args {
                    self.write_line(&format!(
                        "\n{} {} {}",
                        self.bold("\u{1F527}"),
                        self.green(tool_name),
                        self.dim(args_json),
                    ))?;
                } else {
                    self.write_line(&format!("\n{} {}", self.bold("\u{1F527}"), self.green(tool_name),))?;
                }
            },
            RuntimeEvent::ToolCallFinished { tool_name: _, summary, .. } => {
                let summary_short: String = if summary.chars().count() > 500 {
                    let truncated: String = summary.chars().take(500).collect();
                    format!("{}...", truncated)
                } else {
                    summary.clone()
                };
                self.write_line(&format!("   {} {}", self.dim("→"), self.dim(&summary_short)))?;
                // Add a blank line after tool completion for readability
                let _ = writeln!(self.writer);
            },
            RuntimeEvent::AwaitingApproval { request, .. } => {
                self.write_line(&format!("\n⚠️  {} [{:?}] — {}", request.title, request.risk_level, request.message,))?;
            },
            RuntimeEvent::PlanUpdated { explanation, plan, .. } => {
                self.write_line(&format!("\n\u{1F4CB} {}", self.bold("Plan Update")))?;
                self.write_line(&format!("   {}", self.dim(explanation.as_deref().unwrap_or(""))))?;
                for item in plan {
                    let icon = match item.status {
                        PlanStepStatus::Completed => "✅",
                        PlanStepStatus::InProgress => "\u{1F504}",
                        PlanStepStatus::Pending => "⏳",
                    };
                    self.write_line(&format!("   {} {}", icon, item.step))?;
                }
                let _ = writeln!(self.writer);
            },
            RuntimeEvent::RunCancelled { .. } => {
                self.write_line(&format!("\n{} Cancelled", self.yellow("⚠")))?;
            },
            RuntimeEvent::RunFinished { .. } => {},
            RuntimeEvent::UserEvent { .. } => {},
            RuntimeEvent::Checkpoint { .. } => {},
        }

        Ok(())
    }

    fn finish_turn(&mut self) -> AgentResult<()> {
        let duration_ms = self.turn_start.map(|s| s.elapsed().as_millis() as u64).unwrap_or(0);

        let duration_str = if duration_ms >= 1000 {
            format!("{:.1}s", duration_ms as f64 / 1000.0)
        } else {
            format!("{}ms", duration_ms)
        };

        writeln!(
            self.writer,
            "\n{}",
            self.subtle(&format!("· {} elapsed · {} tool call(s)", duration_str, self.tool_call_count)),
        )
        .map_err(|e| agent_base::AgentError::internal(format!("write error: {e}")))?;

        self.tool_call_count = 0;
        self.turn_start = None;
        self.last_assistant_text.clear();
        self.last_was_thought = false;

        Ok(())
    }
}
