mod approval;
mod builder;
mod circuit_breaker;
mod context;
mod middleware;
mod pipeline;

mod recovery;
mod runtime;
mod safety;
mod session;
mod session_store;
mod tool_enforcement;
mod turn_facts;

pub use approval::{AllowAllApprovalHandler, ApprovalHandler, DenyAllApprovalHandler};
pub use builder::AgentBuilder;
pub use circuit_breaker::{CircuitBreaker, CircuitState};
pub use context::ContextWindowManager;
pub use middleware::{Middleware, PostLlmCtx, PreLlmCtx, UserMessageCtx};
pub use pipeline::{DefaultPipeline, ToolExecutionPipeline};
pub(crate) use runtime::EventBus;

pub use crate::types::{AgentResult, ApprovalDecision, ApprovalRequest, RiskLevel, SessionId};
pub use recovery::{
    ConsecutiveFailureRecovery, RetryOnError, StopOnError, ToolErrorAction, ToolErrorRecovery,
};
pub use runtime::AgentRuntime;
pub use runtime::QueueMode;
pub use runtime::SessionManager;
pub use safety::TurnToolLimitMiddleware;
pub use session::AgentSession;
pub use session_store::{InMemorySessionStore, SessionStore};

#[cfg(feature = "sqlite-session")]
pub use session_store::SqliteSessionStore;
pub use tool_enforcement::{ToolEnforcementConfig, ToolEnforcementMiddleware};
pub use turn_facts::TurnFactMiddleware;
