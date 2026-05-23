use tokio::sync::broadcast;

use crate::types::{AgentResult, AgentEvent};

#[derive(Clone)]
pub struct EventBus {
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

    pub fn sender(&self) -> broadcast::Sender<AgentEvent> {
        self.sender.clone()
    }

    pub fn drain_async_events<F>(
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<()>
    where
        F: FnMut(AgentEvent) -> AgentResult<()> + ?Sized,
    {
        loop {
            match event_rx.try_recv() {
                Ok(event) => on_event(event)?,
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }
        Ok(())
    }
}
