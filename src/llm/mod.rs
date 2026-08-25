//! LLM provider integration.
//!
//! This module re-exports types from `llm-trait` and provides agent-base
//! specific type conversions.

// ── Re-export sub-modules from llm-trait ──

/// Provider backend type and protocol type definitions.
pub mod backend;
/// Provider capabilities and info types.
pub mod capabilities;
/// Unified LLM provider configuration.
pub mod config;
/// The unified LLM provider trait.
pub mod provider;
/// Chat request types.
pub mod request;
/// Reasoning configuration (re-export of existing types).
pub mod reasoning;
/// Chat response, chat stream, and stream chunk types.
pub mod response;
/// Raw adapter trait for low-level provider implementations.
pub mod raw_adapter;
/// Usage info re-export.
pub mod types;

// ── Type re-exports from llm-trait ──

pub use llm_trait::response::StreamChunk;
pub use llm_trait::types::UsageInfo;
pub use llm_trait::reasoning::{ReasoningConfig, ReasoningEffort};
