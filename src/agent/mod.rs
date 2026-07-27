//! Agent construction and lifecycle management.
//!
//! This module provides:
//! - [`base_agent_builder`] — a pre-configured `AgentBuilder` factory
//! - [`PhiAgent`] — a thin wrapper around `AgentRuntime` with a simplified API
//! - [`PhiAgentConfig`] — tool-agnostic agent configuration

pub mod builder;
pub mod factory;

pub use builder::base_agent_builder;
pub use factory::{PhiAgent, PhiAgentConfig};
