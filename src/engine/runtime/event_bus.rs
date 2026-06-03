use tokio::sync::broadcast;

use crate::types::{AgentEvent, AgentResult, RuntimeEvent};

/// Internal event bus broadcasting [`AgentEvent`]s within the runtime.
///
/// This is **not** exposed to external consumers or user tools. External
/// consumers receive [`RuntimeEvent`](crate::types::RuntimeEvent) through the
/// unified event callback.
#[derive(Clone)]
pub(crate) struct EventBus {
    sender: broadcast::Sender<AgentEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.sender.subscribe()
    }

    pub fn emit(&self, event: AgentEvent) {
        let _ = self.sender.send(event);
    }

    /// Drain pending events from a broadcast receiver, converting each
    /// [`AgentEvent`] to [`RuntimeEvent`] before forwarding to the callback.
    pub fn drain_async_events<F>(
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<()>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + ?Sized,
    {
        loop {
            match event_rx.try_recv() {
                Ok(event) => on_event(RuntimeEvent::from(event))?,
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }
        Ok(())
    }
}
