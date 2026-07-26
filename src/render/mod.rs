pub mod json_stream;
pub mod null;
pub mod terminal;

pub use json_stream::JsonStreamRenderer;
pub use null::NullRenderer;
pub use terminal::TerminalRenderer;

use std::io::{self, Write};

use agent_base::{AgentResult, RuntimeEvent};

/// Event renderer: converts RuntimeEvents into a specific output format.
///
/// Each renderer is a pure consumer — it only reads events and produces
/// output, without modifying Agent state.
pub trait EventRenderer: Send {
    /// Process one runtime event.
    fn render(&mut self, event: RuntimeEvent) -> AgentResult<()>;

    /// End of current turn — renderer may flush / output summary.
    fn finish_turn(&mut self) -> AgentResult<()>;

    /// End of entire session.
    fn finish_session(&mut self) -> AgentResult<()> {
        Ok(())
    }
}

/// Output format
#[derive(Clone, Debug)]
pub enum OutputFormat {
    /// Rich terminal output (with colors and emoji)
    Terminal {
        show_thinking: bool,
        show_tool_args: bool,
        color: bool,
    },
    /// One JSON object per line
    Json,
    /// No output
    Quiet,
}

/// Create the corresponding renderer for a given output format.
///
/// `writer` defaults to stdout (CLI scenario). Web consumers can pass a
/// custom writer (e.g. a WebSocket sink).
pub fn create_renderer(format: &OutputFormat, writer: Option<Box<dyn Write + Send>>) -> Box<dyn EventRenderer> {
    match format {
        OutputFormat::Terminal { show_thinking, show_tool_args, color } => {
            let w = writer.unwrap_or_else(|| Box::new(io::stdout()));
            Box::new(TerminalRenderer::new(*show_thinking, *show_tool_args, *color, w))
        }
        OutputFormat::Json => {
            let w = writer.unwrap_or_else(|| Box::new(io::stdout()));
            Box::new(JsonStreamRenderer::new(w))
        }
        OutputFormat::Quiet => Box::new(NullRenderer),
    }
}

/// Create a renderer using stdout (backward-compatible).
pub fn create_stdout_renderer(format: &OutputFormat) -> Box<dyn EventRenderer> {
    create_renderer(format, None)
}
