use serde_json::Value;

use crate::types::{AgentResult, ApprovalRequest};
use super::{ToolContext, ToolOutput};

pub trait ToolPolicy: Send + Sync {
    fn evaluate_approval(
        &self,
        tool_name: &str,
        args: &Value,
    ) -> Option<ApprovalRequest>;

    fn before_call(&self, tool_name: &str, args: &Value, ctx: &ToolContext) -> AgentResult<()> {
        let _ = (tool_name, args, ctx);
        Ok(())
    }

    fn after_call(&self, tool_name: &str, args: &Value, result: &ToolOutput, ctx: &ToolContext) -> AgentResult<()> {
        let _ = (tool_name, args, result, ctx);
        Ok(())
    }
}
