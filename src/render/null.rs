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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_base::SessionId;

    #[test]
    fn test_null_renderer_always_ok() {
        let mut r = NullRenderer;
        assert!(r.render(RuntimeEvent::RunFinished { session_id: SessionId { id: 1, external_id: None } }).is_ok());
        assert!(r.render(RuntimeEvent::RunCancelled { session_id: SessionId { id: 1, external_id: None } }).is_ok());
        assert!(r.finish_turn().is_ok());
        assert!(r.finish_session().is_ok());
    }
}
