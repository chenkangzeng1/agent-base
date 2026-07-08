use std::collections::HashMap;
use std::sync::Mutex;

use crate::types::{AgentError, AgentResult, SessionId};

/// Action taken by the runtime after a tool execution failure
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolErrorAction {
    /// Stop the current run with a failed outcome
    Stop,
    /// Feed the error back to the LLM and continue reasoning
    Retry,
}

/// Recovery strategy after a tool execution failure
///
/// Defaults to [`StopOnError`], following the lightweight kernel design of
/// conservative defaults and strategy injection.
/// Upper-layer agents can inject custom strategies such as [`RetryOnError`].
pub trait ToolErrorRecovery: Send + Sync {
    fn on_error(
        &self,
        _session_id: &SessionId,
        _tool_names: &[String],
        _error: &AgentError,
    ) -> AgentResult<ToolErrorAction>;
}

/// Default strategy: stop on tool failure.
///
/// This is the most conservative strategy. The kernel only reports the fact
/// without making business recovery decisions for the upper layer.
pub struct StopOnError;

impl ToolErrorRecovery for StopOnError {
    fn on_error(
        &self,
        _session_id: &SessionId,
        _tool_names: &[String],
        _error: &AgentError,
    ) -> AgentResult<ToolErrorAction> {
        Ok(ToolErrorAction::Stop)
    }
}

/// Continue on tool failure, feeding the error back to the model
///
/// Suitable for scenarios where model self-healing is desired (e.g. code-agent, browser-agent).
pub struct RetryOnError;

impl ToolErrorRecovery for RetryOnError {
    fn on_error(
        &self,
        _session_id: &SessionId,
        _tool_names: &[String],
        _error: &AgentError,
    ) -> AgentResult<ToolErrorAction> {
        Ok(ToolErrorAction::Retry)
    }
}

/// Recovery strategy that tracks consecutive failures per tool and stops after a limit.
///
/// This prevents runaway retry loops where the model keeps calling the same failing
/// tool (e.g., `execute_ssh_command` failing due to auth, but the model retries
/// indefinitely). After `max_consecutive_failures` for the same tool name, the
/// strategy switches from `Retry` to `Stop` with a summary message.
///
/// Failure counters are tracked per session and reset when a different tool
/// succeeds or a new session starts.
pub struct ConsecutiveFailureRecovery {
    max_consecutive_failures: usize,
    /// session_id -> (tool_name -> consecutive_failures)
    failure_counts: Mutex<HashMap<u64, HashMap<String, usize>>>,
}

impl ConsecutiveFailureRecovery {
    pub fn new(max_consecutive_failures: usize) -> Self {
        Self {
            max_consecutive_failures,
            failure_counts: Mutex::new(HashMap::new()),
        }
    }

    /// Reset failure count for a specific tool in a session.
    /// Call this when a tool succeeds to avoid false positives.
    pub fn reset_failures(&self, session_id: &SessionId, tool_name: &str) {
        if let Ok(mut counts) = self.failure_counts.lock() {
            if let Some(session_counts) = counts.get_mut(&session_id.id) {
                session_counts.remove(tool_name);
            }
        }
    }

    /// Reset all failure counts for a session.
    /// Call this when a new turn starts.
    pub fn reset_session(&self, session_id: &SessionId) {
        if let Ok(mut counts) = self.failure_counts.lock() {
            counts.remove(&session_id.id);
        }
    }
}

impl ToolErrorRecovery for ConsecutiveFailureRecovery {
    fn on_error(
        &self,
        session_id: &SessionId,
        tool_names: &[String],
        _error: &AgentError,
    ) -> AgentResult<ToolErrorAction> {
        let mut counts = self.failure_counts.lock().map_err(|e| {
            AgentError::internal(format!("Failed to lock failure counts: {}", e))
        })?;

        let session_counts = counts.entry(session_id.id).or_insert_with(HashMap::new);

        // Increment failure count for each failing tool
        let mut max_failures = 0;
        for name in tool_names {
            let count = session_counts.entry(name.clone()).or_insert(0);
            *count += 1;
            if *count > max_failures {
                max_failures = *count;
            }
        }

        if max_failures >= self.max_consecutive_failures {
            let failing_tools: Vec<String> = tool_names
                .iter()
                .filter(|name| {
                    session_counts
                        .get(*name)
                        .map_or(false, |&c| c >= self.max_consecutive_failures)
                })
                .cloned()
                .collect();

            tracing::warn!(
                session_id = session_id.id,
                failing_tools = ?failing_tools,
                max_consecutive_failures = self.max_consecutive_failures,
                "ConsecutiveFailureRecovery: stopping after repeated failures"
            );

            // Clear counts for this session since we're stopping
            counts.remove(&session_id.id);

            return Ok(ToolErrorAction::Stop);
        }

        Ok(ToolErrorAction::Retry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_on_error_always_stops() {
        let recovery = StopOnError;
        let session_id = SessionId::new(1);
        let names = vec!["tool_a".to_string()];
        let error = AgentError::internal("test error");
        assert_eq!(
            recovery.on_error(&session_id, &names, &error).unwrap(),
            ToolErrorAction::Stop
        );
    }

    #[test]
    fn retry_on_error_always_retries() {
        let recovery = RetryOnError;
        let session_id = SessionId::new(1);
        let names = vec!["tool_a".to_string()];
        let error = AgentError::internal("test error");
        assert_eq!(
            recovery.on_error(&session_id, &names, &error).unwrap(),
            ToolErrorAction::Retry
        );
    }

    #[test]
    fn consecutive_failure_retries_then_stops() {
        let recovery = ConsecutiveFailureRecovery::new(3);
        let session_id = SessionId::new(1);
        let names = vec!["tool_a".to_string()];
        let error = AgentError::internal("test error");

        // First two failures: retry
        assert_eq!(
            recovery.on_error(&session_id, &names, &error).unwrap(),
            ToolErrorAction::Retry
        );
        assert_eq!(
            recovery.on_error(&session_id, &names, &error).unwrap(),
            ToolErrorAction::Retry
        );

        // Third failure: stop
        assert_eq!(
            recovery.on_error(&session_id, &names, &error).unwrap(),
            ToolErrorAction::Stop
        );
    }

    #[test]
    fn consecutive_failure_resets_on_success() {
        let recovery = ConsecutiveFailureRecovery::new(3);
        let session_id = SessionId::new(1);
        let names = vec!["tool_a".to_string()];
        let error = AgentError::internal("test error");

        // Two failures
        recovery.on_error(&session_id, &names, &error).unwrap();
        recovery.on_error(&session_id, &names, &error).unwrap();

        // Reset on success
        recovery.reset_failures(&session_id, "tool_a");

        // Should retry again (counter reset)
        assert_eq!(
            recovery.on_error(&session_id, &names, &error).unwrap(),
            ToolErrorAction::Retry
        );
    }

    #[test]
    fn consecutive_failure_per_session_isolation() {
        let recovery = ConsecutiveFailureRecovery::new(2);
        let session1 = SessionId::new(1);
        let session2 = SessionId::new(2);
        let names = vec!["tool_a".to_string()];
        let error = AgentError::internal("test error");

        // Session 1: two failures -> stop
        recovery.on_error(&session1, &names, &error).unwrap();
        assert_eq!(
            recovery.on_error(&session1, &names, &error).unwrap(),
            ToolErrorAction::Stop
        );

        // Session 2: should still retry (isolated)
        assert_eq!(
            recovery.on_error(&session2, &names, &error).unwrap(),
            ToolErrorAction::Retry
        );
    }

    #[test]
    fn consecutive_failure_resets_session() {
        let recovery = ConsecutiveFailureRecovery::new(2);
        let session_id = SessionId::new(1);
        let names = vec!["tool_a".to_string()];
        let error = AgentError::internal("test error");

        // Two failures -> stop
        recovery.on_error(&session_id, &names, &error).unwrap();
        assert_eq!(
            recovery.on_error(&session_id, &names, &error).unwrap(),
            ToolErrorAction::Stop
        );

        // Reset session
        recovery.reset_session(&session_id);

        // Should retry again
        assert_eq!(
            recovery.on_error(&session_id, &names, &error).unwrap(),
            ToolErrorAction::Retry
        );
    }
}
