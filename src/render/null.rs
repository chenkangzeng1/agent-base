use agent_base::{AgentResult, RuntimeEvent};

use crate::render::EventRenderer;

/// Null renderer: produces no output, only writes tracing logs.
/// Suitable for scenarios like web backends that don't need stdout.
pub struct NullRenderer;

impl EventRenderer for NullRenderer {
    fn render(&mut self, _event: RuntimeEvent) -> AgentResult<()> {
        Ok(())
    }

    fn finish_turn(&mut self) -> AgentResult<()> {
        Ok(())
    }
}
