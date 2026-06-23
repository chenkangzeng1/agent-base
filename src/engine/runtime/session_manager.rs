use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, RwLock};

use crate::engine::context::ContextWindowManager;
use crate::types::{AgentError, AgentResult, MessageRole, SessionId, SessionIdGenerator, SessionConfig};
use crate::engine::session_store::SessionStore;
use crate::engine::AgentSession;

#[derive(Clone)]
pub struct SessionManager {
    session_id_generator: Arc<dyn SessionIdGenerator>,
    sessions: Arc<RwLock<HashMap<SessionId, AgentSession>>>,
    /// Separate LRU timestamp map — protected by a lightweight Mutex,
    /// so session reads (RwLock::read) don't need exclusive access just to
    /// update the last-active time.
    lru_times: Arc<Mutex<HashMap<SessionId, Instant>>>,
    session_store: Arc<dyn SessionStore>,
    config: SessionConfig,
}

impl SessionManager {
    pub fn new(
        session_id_generator: Arc<dyn SessionIdGenerator>,
        session_store: Arc<dyn SessionStore>,
        config: SessionConfig,
    ) -> Self {
        Self {
            session_id_generator,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            lru_times: Arc::new(Mutex::new(HashMap::new())),
            session_store,
            config,
        }
    }

    pub async fn create_session(&self, system_prompt: Option<&str>) -> SessionId {
        // Eviction is best-effort — if it fails, we still create the session
        if let Err(e) = self.evict_if_needed().await {
            tracing::warn!(error = %e, "session eviction failed, proceeding with creation");
        }

        let id = self.session_id_generator.generate();
        let mut session = AgentSession::new(id.clone());
        if let Some(prompt) = system_prompt {
            session.push_message(MessageRole::System, prompt);
        }
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(id.clone(), session);
        }
        {
            let mut lru = self.lru_times.lock().await;
            lru.insert(id.clone(), Instant::now());
        }
        tracing::debug!(session_id = id.id, "session created");
        id
    }

    pub async fn restore_session(&self, session_id: &SessionId) -> Option<AgentSession> {
        {
            let sessions = self.sessions.read().await;
            if sessions.contains_key(session_id) {
                let mut lru = self.lru_times.lock().await;
                lru.insert(session_id.clone(), Instant::now());
                tracing::debug!(session_id = session_id.id, "session restore cache hit");
                return sessions.get(session_id).cloned();
            }
        }
        match self.session_store.load(session_id).await {
            Ok(Some(session)) => {
                let msg_count = session.chat_messages().len();
                // Evict before inserting to make room if needed
                self.evict_if_needed().await.ok();
                {
                    let mut sessions = self.sessions.write().await;
                    sessions.insert(session_id.clone(), session.clone());
                }
                {
                    let mut lru = self.lru_times.lock().await;
                    lru.insert(session_id.clone(), Instant::now());
                }
                tracing::debug!(session_id = session_id.id, msg_count, "session restored from store");
                Some(session)
            }
            Ok(None) => {
                tracing::debug!(session_id = session_id.id, "session not found in store");
                None
            }
            Err(e) => {
                tracing::warn!(session_id = session_id.id, error = %e, "session restore failed");
                None
            }
        }
    }

    pub async fn session(&self, session_id: &SessionId) -> Option<AgentSession> {
        let sessions = self.sessions.read().await;
        let result = sessions.get(session_id).cloned();
        if result.is_some() {
            let mut lru = self.lru_times.lock().await;
            lru.insert(session_id.clone(), Instant::now());
        }
        result
    }

    pub async fn session_or_err(&self, session_id: &SessionId) -> AgentResult<AgentSession> {
        let sessions = self.sessions.read().await;
        let result = sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| AgentError::session_not_found(session_id.id));
        if result.is_ok() {
            let mut lru = self.lru_times.lock().await;
            lru.insert(session_id.clone(), Instant::now());
        }
        result
    }

    pub async fn with_session_mut<F, R>(&self, session_id: &SessionId, f: F) -> AgentResult<R>
    where
        F: FnOnce(&mut AgentSession) -> R,
    {
        // The write lock on `sessions` is held only for the closure execution.
        // It MUST be dropped before `enforce_session_limits` — that method may
        // call `save_session` → `session_or_err` which re-acquires a (read) lock
        // on the same `sessions` map. Holding the write lock across that call
        // would deadlock.
        let result = {
            let mut sessions = self.sessions.write().await;
            let session = sessions
                .get_mut(session_id)
                .ok_or_else(|| AgentError::session_not_found(session_id.id))?;
            f(session)
        };
        // Update LRU timestamp (lightweight Mutex, not RwLock)
        {
            let mut lru = self.lru_times.lock().await;
            lru.insert(session_id.clone(), Instant::now());
        }
        self.enforce_session_limits(session_id).await;
        Ok(result)
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
        let msg_count = session.chat_messages().len();
        tracing::debug!(session_id = session_id.id, msg_count, "saving session");
        self.session_store.save(&session).await.map_err(|e| AgentError::internal(format!("Session persistence failed: {e}")))
    }

    pub fn session_store(&self) -> &Arc<dyn SessionStore> {
        &self.session_store
    }

    /// Evict the least recently used session if at capacity.
    ///
    /// Note: there is a benign TOCTOU window between the read-lock capacity
    /// check and the write-lock removal — another task may concurrently create
    /// a session and also trigger eviction. The worst case is one extra eviction,
    /// which is harmless since `restore_session` transparently reloads from store.
    async fn evict_if_needed(&self) -> AgentResult<()> {
        let max = match self.config.max_sessions {
            Some(m) => m,
            None => return Ok(()),
        };

        // Find the LRU victim (read lock on sessions, Mutex on lru_times)
        let victim = {
            let sessions = self.sessions.read().await;
            if sessions.len() < max {
                return Ok(());
            }
            let lru = self.lru_times.lock().await;
            sessions
                .keys()
                .min_by_key(|id| lru.get(*id).copied().unwrap_or(Instant::now()))
                .cloned()
        };

        let Some(victim_id) = victim else {
            return Ok(());
        };

        // Persist before evicting
        if let Err(e) = self.save_session(&victim_id).await {
            tracing::warn!(session_id = victim_id.id, error = %e, "failed to persist session before eviction");
        }

        // Remove from both maps
        {
            let mut sessions = self.sessions.write().await;
            sessions.remove(&victim_id);
        }
        {
            let mut lru = self.lru_times.lock().await;
            lru.remove(&victim_id);
        }
        tracing::info!(session_id = victim_id.id, "session evicted (LRU)");

        Ok(())
    }

    /// Enforce turn count and message size limits after a session mutation.
    async fn enforce_session_limits(&self, session_id: &SessionId) {
        // Layer 2: Turn count trimming
        if let Some(max_turns) = self.config.max_turns_per_session {
            let needs_trim = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(session_id)
                    .is_some_and(|session| session.turn_count() > max_turns)
            };

            if needs_trim {
                // Persist full history before trimming
                if let Err(e) = self.save_session(session_id).await {
                    tracing::warn!(session_id = session_id.id, error = %e, "failed to persist before turn trim");
                }
                // Trim under write lock
                let mut sessions = self.sessions.write().await;
                if let Some(session) = sessions.get_mut(session_id) {
                    let before = session.turn_count();
                    session.trim_oldest_turns(max_turns);
                    tracing::info!(
                        session_id = session_id.id,
                        before,
                        after = session.turn_count(),
                        max_turns,
                        "session turns trimmed"
                    );
                }
            }
        }

        // Layer 3: Oversized message safety valve
        if let Some(max_tokens) = self.config.max_message_tokens {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(session_id) {
                if let Some(last) = session.chat_messages().last() {
                    let tokens = ContextWindowManager::message_tokens(last);
                    if tokens > max_tokens {
                        session.pop_last_message();
                        tracing::warn!(
                            session_id = session_id.id,
                            tokens,
                            max_tokens,
                            "oversized message removed from session (safety valve)"
                        );
                    }
                }
            }
        }
    }
}
