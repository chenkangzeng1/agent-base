use async_trait::async_trait;
use serde_json::Value;

use super::{ToolContext, ToolOutput};
use crate::types::{AgentResult, ApprovalRequest};

/// Policy-based control over tool execution.
///
/// Implement this trait to customise how tools are approved, monitored, and
/// validated during an agent run. The pipeline calls each hook at a specific
/// point in the tool lifecycle:
///
/// ```text
/// evaluate_approval  →  before_call  →  (tool executes)  →  after_call
/// ```
///
/// # Example: auto-approve read-only tools
///
/// ```ignore
/// struct ReadOnlyPolicy;
///
/// #[async_trait]
/// impl ToolPolicy for ReadOnlyPolicy {
///     async fn evaluate_approval(&self, tool_name: &str, _args: &Value) -> Option<ApprovalRequest> {
///         if tool_name == "read_file" || tool_name == "search" {
///             None  // auto-approve — no prompt for user
///         } else {
///             Some(ApprovalRequest { message: format!("Allow {}?", tool_name) })
///         }
///     }
/// }
/// ```
///
/// All hooks have default no-op implementations, so you only need to override
/// the ones you care about.
#[async_trait]
pub trait ToolPolicy: Send + Sync {
    /// Called **before** every tool call.
    ///
    /// Return `None` to auto-approve (skip the approval handler entirely).
    /// Return `Some(ApprovalRequest)` to defer to the configured
    /// [`ApprovalHandler`](crate::ApprovalHandler).
    ///
    /// This is the primary hook for implementing permission guards — for
    /// example, auto-approving read-only operations while prompting for
    /// destructive ones.
    async fn evaluate_approval(&self, tool_name: &str, args: &Value) -> Option<ApprovalRequest>;

    /// Called immediately **before** a tool executes, after approval has been
    /// granted (or auto-approved).
    ///
    /// Use this for:
    /// - Input validation (reject malformed arguments before the tool runs).
    /// - Auditing / logging the raw call.
    /// - Rate-limiting or quota enforcement.
    ///
    /// Return an `Err` to **cancel** the tool call before execution. The error
    /// message is surfaced to the LLM so it can correct its approach.
    fn before_call(&self, tool_name: &str, args: &Value, ctx: &ToolContext) -> AgentResult<()> {
        let _ = (tool_name, args, ctx);
        Ok(())
    }

    /// Called immediately **after** a tool executes, before the result is
    /// returned to the LLM.
    ///
    /// Use this for:
    /// - Output scrubbing / redaction (strip secrets from tool results).
    /// - Truncation or formatting of large outputs.
    /// - Recording metrics or audit trails.
    ///
    /// The `result` is the raw [`ToolOutput`] produced by the tool. You can
    /// inspect it but not modify it through this hook — if you need to
    /// transform the output, use a middleware instead.
    ///
    /// Return an `Err` to **reject** the result. The error message is surfaced
    /// to the LLM.
    fn after_call(
        &self,
        tool_name: &str,
        args: &Value,
        result: &ToolOutput,
        ctx: &ToolContext,
    ) -> AgentResult<()> {
        let _ = (tool_name, args, result, ctx);
        Ok(())
    }
}
