use crate::types::{AgentError, AgentResult, SessionId};

use super::plan_runner::RuntimeCore;

mod entry;
mod tools;
mod turn;
mod turn_end;

impl RuntimeCore {
    pub async fn validate_session(&self, session_id: &SessionId) -> AgentResult<()> {
        if self.session_manager.session(session_id).await.is_none() {
            return Err(AgentError::session_not_found(session_id.id));
        }
        Ok(())
    }

    pub async fn with_session_mut<F, R>(&self, session_id: &SessionId, f: F) -> AgentResult<R>
    where
        F: FnOnce(&mut crate::engine::AgentSession) -> R,
    {
        self.session_manager.with_session_mut(session_id, f).await
    }
}

#[cfg(test)]
mod tests;
