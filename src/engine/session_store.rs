use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::types::{AgentResult, AgentError, SessionId};
use super::AgentSession;

/// Session 持久化适配层
///
/// `SessionStore` 是可选的持久化接口。在轻内核设计下:
/// - `AgentRuntime.sessions` 是运行期间的权威 live state
/// - `SessionStore` 是 persistence adapter，负责 save/load/list/delete
/// - 不介入每一步执行控制流
///
/// 上层可通过 build 时注入自定义实现（如文件、数据库等）。
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// 保存 session 快照到持久层
    async fn save(&self, session: &AgentSession) -> AgentResult<()>;

    /// 从持久层加载 session
    async fn load(&self, session_id: &SessionId) -> AgentResult<Option<AgentSession>>;

    /// 列出所有已保存的 session id
    async fn list(&self) -> AgentResult<Vec<SessionId>>;

    /// 删除指定 session
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
