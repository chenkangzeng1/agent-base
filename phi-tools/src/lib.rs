//! phi-tools: General-purpose Agent toolset — local shell and file operations.
//!
//! Each tool independently implements the `agent_base::Tool` trait.
//! Consumers register tools with `AgentBuilder` on demand.
//!
//! For SSH/PTY tools, see the `ops-tools` crate.
//!
//! # Example
//!
//! ```ignore
//! use phi_tools::LocalShellTool;
//!
//! builder.register_tool(LocalShellTool::new(30_000));
//! ```

pub mod local_shell;
pub use local_shell::LocalShellTool;
