use tokio::sync::mpsc;

use agent_base::{AgentRuntime, RuntimeEvent, SessionId};

/// Agent session handle — unified input/output/cancel interface
///
/// `AgentHandle` wraps a command queue, a Worker task, an event stream, and cancellation.
/// All callers (CLI / UI / HTTP API) interact with agent-base through it.
///
/// # Example
///
/// ```rust,no_run
/// use agent_works::AgentHandle;
/// use agent_base::AgentRuntime;
///
/// # async fn example(runtime: AgentRuntime) {
/// let mut handle = AgentHandle::new(runtime);
///
/// // Send user input
/// handle.send_input("check disk space").await.unwrap();
///
/// // Receive events
/// while let Some(event) = handle.recv_event().await {
///     // Handle event...
///     if matches!(event, agent_base::RuntimeEvent::RunFinished { .. }
///         | agent_base::RuntimeEvent::RunCancelled { .. }) {
///         break;
///     }
/// }
///
/// // Cancel current execution
/// handle.cancel();
/// # }
/// ```
pub struct AgentHandle {
    cmd_tx: mpsc::Sender<AgentCommand>,
    event_rx: mpsc::UnboundedReceiver<RuntimeEvent>,
    runtime: AgentRuntime,
    default_session_id: Option<SessionId>,
}

enum AgentCommand {
    RunTurn {
        session_id: SessionId,
        input: String,
    },
}

#[derive(Debug)]
pub enum SendError {
    ChannelClosed,
}

impl AgentHandle {
    /// Create a new AgentHandle, spawning a background Worker
    pub fn new(runtime: AgentRuntime) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let rt = runtime.clone();

        // Worker task: processes user requests serially
        tokio::spawn(async move {
            let mut rx = cmd_rx;
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    AgentCommand::RunTurn { session_id, input } => {
                        let tx = event_tx.clone();
                        let sid = session_id.clone();
                        let result = rt
                            .run_turn(session_id, &input, move |event| {
                                let _ = tx.send(event);
                                Ok(())
                            })
                            .await;

                        match &result {
                            Ok(_) => {}
                            Err(e) if e.is_cancelled() => {}
                            Err(e) => {
                                tracing::error!(error = %e, "run_turn failed");
                                let _ =
                                    event_tx.send(RuntimeEvent::RunFinished { session_id: sid });
                            }
                        }
                    }
                }
            }
        });

        Self {
            cmd_tx,
            event_rx,
            runtime,
            default_session_id: None,
        }
    }

    /// Create an AgentHandle with a default session_id
    /// The session_id will be used for all send_input calls unless overridden
    pub fn with_session(runtime: AgentRuntime, session_id: SessionId) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let rt = runtime.clone();

        // Worker task: processes user requests serially
        tokio::spawn(async move {
            let mut rx = cmd_rx;
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    AgentCommand::RunTurn { session_id, input } => {
                        let tx = event_tx.clone();
                        let sid = session_id.clone();
                        let result = rt
                            .run_turn(session_id, &input, move |event| {
                                let _ = tx.send(event);
                                Ok(())
                            })
                            .await;

                        // Handle errors: ensure caller always gets a terminal event
                        match &result {
                            Ok(_) => {
                                // run_turn already emitted RunFinished or RunCancelled
                            }
                            Err(e) if e.is_cancelled() => {
                                // RunCancelled already emitted inside run_turn
                            }
                            Err(e) => {
                                // Non-cancellation error: emit RunFinished so caller isn't stuck
                                tracing::error!(error = %e, "run_turn failed");
                                let _ =
                                    event_tx.send(RuntimeEvent::RunFinished { session_id: sid });
                            }
                        }
                    }
                }
            }
        });

        Self {
            cmd_tx,
            event_rx,
            runtime,
            default_session_id: Some(session_id),
        }
    }

    /// Send user input (async, with error return)
    /// Uses the default session_id if set via with_session(), otherwise creates a new session
    pub async fn send_input(&self, input: &str) -> Result<(), SendError> {
        let session_id = match &self.default_session_id {
            Some(id) => id.clone(),
            None => self.runtime.create_session().await,
        };
        self.cmd_tx
            .send(AgentCommand::RunTurn {
                session_id,
                input: input.to_string(),
            })
            .await
            .map_err(|_| SendError::ChannelClosed)
    }

    /// Send user input with a specified session_id
    pub async fn send_input_with_session(
        &self,
        input: &str,
        session_id: SessionId,
    ) -> Result<(), SendError> {
        self.cmd_tx
            .send(AgentCommand::RunTurn {
                session_id,
                input: input.to_string(),
            })
            .await
            .map_err(|_| SendError::ChannelClosed)
    }

    /// Receive the next event (blocking)
    pub async fn recv_event(&mut self) -> Option<RuntimeEvent> {
        self.event_rx.recv().await
    }

    /// Try to receive an event (non-blocking)
    pub fn try_recv_event(&mut self) -> Option<RuntimeEvent> {
        self.event_rx.try_recv().ok()
    }

    /// Cancel current execution — delegates to runtime, always cancels the latest token
    pub fn cancel(&self) {
        self.runtime.cancel();
    }

    /// Get a reference to the underlying runtime
    pub fn runtime(&self) -> &AgentRuntime {
        &self.runtime
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_agent_handle_creation() {
        // This test requires a full AgentRuntime, skipped for now
        // Actual testing is done in integration tests
    }
}
