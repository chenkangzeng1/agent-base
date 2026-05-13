use anyhow::Result;
use async_trait::async_trait;

use crate::types::{ApprovalDecision, ApprovalRequest};

#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn approve(&self, request: ApprovalRequest) -> Result<ApprovalDecision>;
}

#[derive(Clone, Debug, Default)]
pub struct DenyAllApprovalHandler;

#[async_trait]
impl ApprovalHandler for DenyAllApprovalHandler {
    async fn approve(&self, _request: ApprovalRequest) -> Result<ApprovalDecision> {
        Ok(ApprovalDecision::Deny)
    }
}

#[derive(Clone, Debug, Default)]
pub struct AllowAllApprovalHandler;

#[async_trait]
impl ApprovalHandler for AllowAllApprovalHandler {
    async fn approve(&self, _request: ApprovalRequest) -> Result<ApprovalDecision> {
        Ok(ApprovalDecision::AllowAlways)
    }
}
