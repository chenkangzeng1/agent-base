use tokio::sync::broadcast;

use crate::types::{AgentResult, RuntimeEvent};

/// Internal event bus broadcasting [`RuntimeEvent`]s within the runtime.
#[derive(Clone)]
pub(crate) struct EventBus {
    sender: broadcast::Sender<RuntimeEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.sender.subscribe()
    }

    pub fn emit(&self, event: RuntimeEvent) {
        let _ = self.sender.send(event);
    }

    /// Drain pending events from the broadcast receiver, forwarding each
    /// [`RuntimeEvent`] to the callback.
    pub fn drain_async_events<F>(
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: &mut F,
    ) -> AgentResult<()>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + ?Sized,
    {
        loop {
            match event_rx.try_recv() {
                Ok(event) => on_event(event)?,
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::warn!(
                        skipped = n,
                        "EventBus consumer lagged, events dropped"
                    );
                    continue;
                }
                Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }
        Ok(())
    }
}
