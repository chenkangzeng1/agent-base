use std::io::{self, Write};

use agent_base::{AgentResult, RuntimeEvent};

pub struct CliEventPrinter {
    pub assistant_prefix_printed: bool,
    pub custom_handlers: Vec<Box<dyn Fn(&RuntimeEvent) -> Option<String> + Send>>,
}

impl Default for CliEventPrinter {
    fn default() -> Self {
        Self {
            assistant_prefix_printed: false,
            custom_handlers: Vec::new(),
        }
    }
}

impl CliEventPrinter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle(&mut self, event: RuntimeEvent) -> AgentResult<()> {
        for handler in &self.custom_handlers {
            if let Some(output) = handler(&event) {
                self.finish();
                print!("{output}");
                io::stdout().flush().map_err(|e| {
                    agent_base::AgentError::internal(format!("flush stdout failed: {e}"))
                })?;
                return Ok(());
            }
        }

        match event {
            RuntimeEvent::TextDelta { text, .. } => {
                if !self.assistant_prefix_printed {
                    print!("Assistant > ");
                    self.assistant_prefix_printed = true;
                }
                print!("{text}");
                io::stdout().flush().map_err(|e| {
                    agent_base::AgentError::internal(format!("flush stdout failed: {e}"))
                })?;
            }
            RuntimeEvent::ThoughtDelta { text, .. } => {
                print!("\x1b[90m{text}\x1b[0m");
                io::stdout().flush().map_err(|e| {
                    agent_base::AgentError::internal(format!("flush stdout failed: {e}"))
                })?;
            }
            RuntimeEvent::ToolCallStarted {
                tool_name,
                args_json,
                ..
            } => {
                self.finish();
                println!("[Tool Start] {tool_name} {args_json}");
            }
            RuntimeEvent::ToolCallFinished {
                tool_name, summary, ..
            } => {
                self.finish();
                println!("[Tool Done] {tool_name}");
                for line in summary.lines() {
                    println!("  {line}");
                }
            }
            RuntimeEvent::AwaitingApproval { .. } => {
                self.finish();
            }
            RuntimeEvent::RunFinished { .. } => {
                self.finish();
            }
            RuntimeEvent::PlanUpdated {
                objective, plan, ..
            } => {
                self.finish();
                println!("[Plan] {objective}");
                for (i, item) in plan.iter().enumerate() {
                    let status_icon = match item.status {
                        agent_base::PlanStepStatus::Completed => "✅",
                        agent_base::PlanStepStatus::InProgress => "🔄",
                        agent_base::PlanStepStatus::Pending => "⏳",
                    };
                    println!("  {status_icon} {}. {}", i + 1, item.step);
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn finish(&mut self) {
        if self.assistant_prefix_printed {
            println!();
            self.assistant_prefix_printed = false;
        }
    }
}
