use async_trait::async_trait;
use serde_json::Value;

use crate::types::ApprovalRequest;
use super::{ToolContext, ToolOutput};

#[async_trait]
pub trait ToolPolicy: Send + Sync {
    fn evaluate_approval(
        &self,
        tool_name: &str,
        args: &Value,
        args_json: &str,
    ) -> Option<ApprovalRequest>;

    fn on_pre_call(&self, tool_name: &str, args: &Value, ctx: &ToolContext);

    fn on_post_call(&self, tool_name: &str, args: &Value, result: &ToolOutput, ctx: &ToolContext);
}
