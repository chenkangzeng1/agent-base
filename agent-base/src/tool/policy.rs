use async_trait::async_trait;
use serde_json::Value;

use super::{ToolContext, ToolOutput};
use crate::types::{AgentResult, ApprovalRequest};

#[async_trait]
pub trait ToolPolicy: Send + Sync {
    async fn evaluate_approval(&self, tool_name: &str, args: &Value) -> Option<ApprovalRequest>;

    fn before_call(&self, tool_name: &str, args: &Value, ctx: &ToolContext) -> AgentResult<()> {
        let _ = (tool_name, args, ctx);
        Ok(())
    }

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
