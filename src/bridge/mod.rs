//! Bridge protocol — adapts phi-agent for SDK consumption.
//!
//! The bridge protocol uses NDJSON over stdio to expose an agent to
//! external language SDKs (Python, Node.js, etc.). The [`server::ProtocolServer`]
//! manages sessions, tool registration, and event forwarding.

pub mod messages;
pub mod server;
