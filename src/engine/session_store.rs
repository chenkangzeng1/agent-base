use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::types::{AgentResult, AgentError, SessionId};
use super::AgentSession;

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn save(&self, session: &AgentSession) -> AgentResult<()>;

    async fn load(&self, session_id: &SessionId) -> AgentResult<Option<AgentSession>>;

    async fn list(&self) -> AgentResult<Vec<SessionId>>;

    async fn delete(&self, session_id: &SessionId) -> AgentResult<()>;
}

pub struct InMemorySessionStore {
    sessions: Mutex<HashMap<SessionId, AgentSession>>,
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn save(&self, session: &AgentSession) -> AgentResult<()> {
        let session_id = session
            .id()
            .ok_or_else(|| AgentError::internal("session has no id"))?;
        self.sessions
            .lock()
            .await
            .insert(session_id, session.clone());
        Ok(())
    }

    async fn load(&self, session_id: &SessionId) -> AgentResult<Option<AgentSession>> {
        Ok(self.sessions.lock().await.get(session_id).cloned())
    }

    async fn list(&self) -> AgentResult<Vec<SessionId>> {
        Ok(self.sessions.lock().await.keys().cloned().collect())
    }

    async fn delete(&self, session_id: &SessionId) -> AgentResult<()> {
        self.sessions.lock().await.remove(session_id);
        Ok(())
    }
}
