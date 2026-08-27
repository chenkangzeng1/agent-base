//! Pure type definitions for the agent-base runtime.
//!
//! This crate contains data structures and enums with **zero runtime
//! dependencies** — no async, no I/O, no trait objects. It exists so that
//! downstream crates (SDKs, protocol layers, tool definitions) can depend on
//! just the types without pulling in the full agent-base runtime.
//!
//! agent-base re-exports everything from this crate, so existing
//! `use agent_base::Content` continues to work.

mod session;
mod tool;

pub use session::SessionId;
pub use tool::{ActivationContext, Content, ToolExposure, ToolMetadata, content_text};
