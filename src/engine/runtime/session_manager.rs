use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::types::{AgentError, AgentResult, MessageRole, SessionId, SessionIdGenerator};
use crate::engine::session_store::SessionStore;
use crate::engine::AgentSession;

pub struct SessionManager {
    session_id_generator: Arc<dyn SessionIdGenerator>,
    sessions: Arc<RwLock<HashMap<SessionId, AgentSession>>>,
    session_store: Arc<dyn SessionStore>,
}

impl SessionManager {
    pub fn new(
        session_id_generator: Arc<dyn SessionIdGenerator>,
        session_store: Arc<dyn SessionStore>,
    ) -> Self {
        Self {
            session_id_generator,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            session_store,
        }
    }

    pub async fn create_session(&self, system_prompt: Option<&str>) -> SessionId {
        let id = self.session_id_generator.generate();
        let mut session = AgentSession::new(id.clone());
        if let Some(prompt) = system_prompt {
            session.push_message(MessageRole::System, prompt);
        }
        let mut sessions = self.sessions.write().await;
        sessions.insert(id.clone(), session);
        id
    }

    pub async fn restore_session(&self, session_id: &SessionId) -> Option<AgentSession> {
        {
            let sessions = self.sessions.read().await;
            if sessions.contains_key(session_id) {
                return sessions.get(session_id).cloned();
            }
        }
        match self.session_store.load(session_id).await {
            Ok(Some(session)) => {
                let mut sessions = self.sessions.write().await;
                sessions.insert(session_id.clone(), session.clone());
                Some(session)
            }
            _ => None,
        }
    }

    pub async fn session(&self, session_id: &SessionId) -> Option<AgentSession> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned()
    }

    pub async fn session_or_err(&self, session_id: &SessionId) -> AgentResult<AgentSession> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| AgentError::session_not_found(session_id.id))
    }

    pub async fn with_session_mut<F, R>(&self, session_id: &SessionId, f: F) -> AgentResult<R>
    where
        F: FnOnce(&mut AgentSession) -> R,
    {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| AgentError::session_not_found(session_id.id))?;
        Ok(f(session))
    }

    pub async fn cached_approval(&self, session_id: &SessionId, action_key: &str) -> bool {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .is_some_and(|session| session.is_action_allowed(action_key))
    }

    pub async fn cache_approval(&self, session_id: &SessionId, action_key: String) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.allow_action(action_key);
        }
    }

    pub async fn save_session(&self, session_id: &SessionId) -> AgentResult<()> {
        let session = self.session_or_err(session_id).await?;
        self.session_store.save(&session).await.map_err(|e| AgentError::internal(format!("Session persistence failed: {e}")))
    }

    pub fn session_store(&self) -> &Arc<dyn SessionStore> {
        &self.session_store
    }
}
