use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use tokio::sync::broadcast;

use crate::types::{AgentResult, RuntimeEvent};

/// Internal event bus broadcasting [`RuntimeEvent`]s within the runtime.
#[derive(Clone)]
pub(crate) struct EventBus {
    sender: broadcast::Sender<RuntimeEvent>,
    /// Count of PlanUpdated events emitted since last reset.
    plan_updates: Arc<AtomicU32>,
    /// Count of AwaitingApproval events emitted since last reset.
    approval_count: Arc<AtomicU32>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            plan_updates: Arc::new(AtomicU32::new(0)),
            approval_count: Arc::new(AtomicU32::new(0)),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.sender.subscribe()
    }

    pub fn emit(&self, event: RuntimeEvent) {
        match &event {
            RuntimeEvent::PlanUpdated { .. } => {
                self.plan_updates.fetch_add(1, Ordering::Relaxed);
            }
            RuntimeEvent::AwaitingApproval { .. } => {
                self.approval_count.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        let _ = self.sender.send(event);
    }

    /// Take and reset the plan-update counter for the current turn.
    pub fn take_plan_updates(&self) -> u32 {
        self.plan_updates.swap(0, Ordering::Relaxed)
    }

    /// Take and reset the approval counter for the current turn.
    pub fn take_approval_count(&self) -> u32 {
        self.approval_count.swap(0, Ordering::Relaxed)
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
                    tracing::warn!(skipped = n, "EventBus consumer lagged, events dropped");
                    continue;
                }
                Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }
        Ok(())
    }
}
