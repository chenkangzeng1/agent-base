//! phi-telemetry — Observability layer for phi-agent.
//!
//! Collects structured per-turn and per-session metrics via hooks registered
//! on the agent runtime. Metrics are persisted as `session_metrics.json`
//! alongside existing session data.
//!
//! ## Architecture
//!
//! ```text
//! agent-base (TurnContext + on_turn_end hook)
//!     ↑
//! phi-telemetry (collector + storage)  ← this crate
//!     ↑
//! phi-agent / phi-bard (consumers)
//! ```
//!
//! The observer runs in an independent tokio task, communicating with the
//! agent runtime through an mpsc channel. A panic in the observer task never
//! affects the agent.
//!
//! ## Quick start
//!
//! ```ignore
//! use phi_telemetry::{init_telemetry, save_metrics};
//!
//! let handle = init_telemetry(&runtime, session_id, node_id, model);
//! // ... agent runs, metrics accumulate automatically ...
//! handle.shutdown();
//! let session = handle.session.blocking_read();
//! save_metrics(&session, &session_dir)?;
//! ```

pub mod collector;
pub mod storage;
pub mod types;

pub use collector::{ObserverHandle, init_telemetry};
pub use storage::{list_all_metrics, load_metrics, save_metrics, try_load_metrics};
pub use types::{SessionMetrics, SessionOutcome, SessionSummary, TurnMetrics, TurnOutcome};
