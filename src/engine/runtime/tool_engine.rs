use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::{RwLock, broadcast, mpsc};

use crate::engine::approval::ApprovalHandler;
use crate::engine::pipeline::{DefaultPipeline, ToolExecutionPipeline};
use crate::engine::recovery::ToolErrorRecovery;
use crate::engine::runtime::event_bus::EventBus;
use crate::engine::runtime::session_manager::SessionManager;
use crate::tool::{Content, ToolContext, ToolPolicy, ToolRegistry, content_text};
use crate::types::{AgentError, AgentResult, Language, RuntimeEvent, SessionId, UserEvent};

pub(crate) struct ToolEngine {
    tools: Arc<RwLock<ToolRegistry>>,
    approval_handler: Option<Arc<dyn ApprovalHandler>>,
    tool_policy: Option<Arc<dyn ToolPolicy>>,
    error_recovery: Arc<dyn ToolErrorRecovery>,
    event_bus: EventBus,
    pipeline: DefaultPipeline,
}

impl ToolEngine {
    pub fn new(
        tools: ToolRegistry,
        approval_handler: Option<Arc<dyn ApprovalHandler>>,
        tool_policy: Option<Arc<dyn ToolPolicy>>,
        error_recovery: Arc<dyn ToolErrorRecovery>,
        event_bus: EventBus,
    ) -> Self {
        let pipeline = DefaultPipeline::new(tool_policy.clone(), None, None);
        Self {
            tools: Arc::new(RwLock::new(tools)),
            approval_handler,
            tool_policy,
            error_recovery,
            event_bus,
            pipeline,
        }
    }

    pub async fn definitions(&self) -> Vec<Value> {
        self.tools.read().await.definitions()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn execute_tool<F>(
        &self,
        session_id: &SessionId,
        id: &str,
        name: &str,
        args: &Value,
        tool_args_json: &str,
        ctx: &ExecutionContext,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: Arc<Mutex<F>>,
    ) -> AgentResult<ToolExecutionResult>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        tracing::debug!(
            session_id = session_id.id,
            tool = name,
            args_len = tool_args_json.len(),
            "execute tool start"
        );

        // Emit ToolCallStarted via internal EventBus
        self.event_bus.emit(RuntimeEvent::ToolCallStarted {
            session_id: session_id.clone(),
            tool_name: name.to_string(),
            args_json: tool_args_json.to_string(),
            agent_id: None,
            trace_id: None,
        });
        {
            let mut cb = on_event.lock().unwrap();
            EventBus::drain_async_events(event_rx, &mut *cb)?;
        }

        // Build ToolContext with UserEvent channel for tool-produced events
        let (user_event_tx, mut user_event_rx) = mpsc::unbounded_channel::<UserEvent>();
        let tool_context = ToolContext {
            session_id: session_id.clone(),
            user_event_tx,
            llm_client: ctx.llm_client.clone(),
            session_store: Some(ctx.session_manager.session_store().clone()),
            language: ctx.language.clone(),
            cancel_token: ctx.cancel_token.clone(),
            max_output_chars: ctx.max_output_chars,
            event_bus: self.event_bus.clone(),
        };

        // Lookup tool and execute via pipeline.
        // The pipeline handles: before_call hook → timeout → truncation → after_call hook.
        // ToolEngine handles: event emission and UserEvent forwarding.
        tracing::debug!(
            session_id = session_id.id,
            tool = name,
            "looking up tool in registry"
        );
        let tools_guard = self.tools.read().await;
        let tool_result = match tools_guard.get(name) {
            Some(tool) => {
                tracing::debug!(
                    session_id = session_id.id,
                    tool = name,
                    "tool found, executing via pipeline"
                );

                // Per-call pipeline: inherits policy from self.pipeline, adds caller's timeout/truncation.
                let pipeline = DefaultPipeline::new(
                    self.pipeline.policy(),
                    ctx.tool_timeout_ms,
                    ctx.max_output_chars,
                );

                let future = pipeline.execute(tool.as_ref(), args, &tool_context);
                tokio::pin!(future);

                // Execute with UserEvent forwarding interleaved via tokio::select!
                let output = loop {
                    tokio::select! {
                        result = &mut future => break result,
                        Some(user_event) = user_event_rx.recv() => {
                            if let Ok(mut cb) = on_event.lock() {
                                cb(RuntimeEvent::UserEvent {
                                    session_id: session_id.clone(),
                                    event: user_event,
                                    agent_id: None,
                                    trace_id: None,
                                })?;
                            }
                        }
                        _ = ctx.cancel_token.cancelled() => {
                            tracing::info!(session_id = session_id.id, tool = name, "tool execution cancelled");
                            return Err(crate::types::AgentError::Cancelled);
                        }
                    }
                };

                // Drain remaining UserEvents after tool completes
                while let Ok(user_event) = user_event_rx.try_recv() {
                    if let Ok(mut cb) = on_event.lock() {
                        cb(RuntimeEvent::UserEvent {
                            session_id: session_id.clone(),
                            event: user_event,
                            agent_id: None,
                            trace_id: None,
                        })?;
                    }
                }

                match output {
                    Ok(output) => output,
                    Err(e) => {
                        tracing::error!(session_id = session_id.id, tool_name = name, error = %e, "Tool execution failed");
                        // Emit ToolCallFinished with error summary before returning error
                        let error_summary = if ctx.language == Language::Zh {
                            format!("❌ 执行失败: {}", e)
                        } else {
                            format!("❌ Tool execution failed: {}", e)
                        };
                        self.event_bus.emit(RuntimeEvent::ToolCallFinished {
                            session_id: session_id.clone(),
                            tool_name: name.to_string(),
                            summary: error_summary,
                            agent_id: None,
                            trace_id: None,
                            denied: false,
                        });
                        // Use `let _ =` to avoid masking the original tool error
                        // if the event callback fails
                        let _ = {
                            let mut cb = on_event.lock().unwrap();
                            EventBus::drain_async_events(event_rx, &mut *cb)
                        };
                        return Err(AgentError::ToolExecution {
                            name: name.to_string(),
                            source: Box::new(e),
                        });
                    }
                }
            }
            None => {
                tracing::warn!(
                    session_id = session_id.id,
                    tool = name,
                    "tool not found in registry"
                );
                vec![Content::text(if ctx.language == Language::Zh {
                    format!("工具 {} 未找到", name)
                } else {
                    format!("Tool {} not found", name)
                })]
            }
        };

        // Emit ToolCallFinished via internal EventBus
        self.event_bus.emit(RuntimeEvent::ToolCallFinished {
            session_id: session_id.clone(),
            tool_name: name.to_string(),
            summary: content_text(&tool_result),
            agent_id: None,
            trace_id: None,
            denied: false,
        });
        {
            let mut cb = on_event.lock().unwrap();
            EventBus::drain_async_events(event_rx, &mut *cb)?;
        }

        // A successful execution breaks any consecutive-failure streak for this tool.
        self.error_recovery.on_success(session_id, name);

        Ok(ToolExecutionResult {
            id: id.to_string(),
            name: name.to_string(),
            output: tool_result,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn process_approval<F>(
        &self,
        session_id: &SessionId,
        tool_name: &str,
        args: &Value,
        _tool_args_json: &str,
        ctx: &ExecutionContext,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: Arc<Mutex<F>>,
    ) -> AgentResult<()>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let approval_request = match self.tool_policy.as_ref() {
            Some(policy) => policy.evaluate_approval(tool_name, args).await,
            None => None,
        };

        let Some(request) = approval_request else {
            return Ok(());
        };

        let approved = if let Some(key) = request.action_key.as_deref() {
            ctx.session_manager.cached_approval(session_id, key).await
        } else {
            false
        };

        if approved {
            tracing::debug!(
                session_id = session_id.id,
                tool = tool_name,
                "approval cached, skipping"
            );
            return Ok(());
        }

        tracing::debug!(session_id = session_id.id, tool = tool_name, risk = ?request.risk_level, "requesting approval");

        self.event_bus.emit(RuntimeEvent::AwaitingApproval {
            session_id: session_id.clone(),
            request: request.clone(),
            agent_id: None,
            trace_id: None,
        });
        {
            let mut cb = on_event.lock().unwrap();
            EventBus::drain_async_events(event_rx, &mut *cb)?;
        }

        let decision = match self.approval_handler.as_ref() {
            Some(handler) => {
                let timeout = std::time::Duration::from_secs(
                    std::env::var("APPROVAL_TIMEOUT_SECS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(300),
                );
                let result = tokio::time::timeout(
                    timeout,
                    handler.approve(request.clone(), ctx.cancel_token.clone()),
                )
                .await;
                match result {
                    Ok(result) => result.map_err(|e| {
                        AgentError::internal(format!("Approval handler failed: {e}"))
                    })?,
                    Err(_) => {
                        tracing::warn!(
                            session_id = session_id.id,
                            ?timeout,
                            "Approval timed out, defaulting to Deny"
                        );
                        crate::types::ApprovalDecision::Deny
                    }
                }
            }
            None => crate::types::ApprovalDecision::Deny,
        };

        match decision {
            crate::types::ApprovalDecision::AllowOnce => {
                tracing::info!(
                    session_id = session_id.id,
                    tool = tool_name,
                    decision = "AllowOnce",
                    "approval granted"
                );
            }
            crate::types::ApprovalDecision::AllowAlways => {
                tracing::info!(
                    session_id = session_id.id,
                    tool = tool_name,
                    decision = "AllowAlways",
                    "approval granted (cached)"
                );
                if let Some(action_key) = request.action_key.clone() {
                    ctx.session_manager
                        .cache_approval(session_id, action_key)
                        .await;
                }
            }
            crate::types::ApprovalDecision::Deny => {
                tracing::warn!(
                    session_id = session_id.id,
                    tool = tool_name,
                    decision = "Deny",
                    "approval denied"
                );
                let denial_summary =
                    format!("[Action Denied]: tool {} rejected by approval", tool_name);
                // 不记录到 session 历史 — 用户拒绝是 UI 层交互，不需要 LLM 看到
                self.event_bus.emit(RuntimeEvent::ToolCallFinished {
                    session_id: session_id.clone(),
                    tool_name: tool_name.to_string(),
                    summary: denial_summary,
                    agent_id: None,
                    trace_id: None,
                    denied: true,
                });
                let _ = {
                    let mut cb = on_event.lock().unwrap();
                    EventBus::drain_async_events(event_rx, &mut *cb)
                };
                return Err(AgentError::ApprovalDenied {
                    tool_name: tool_name.to_string(),
                });
            }
        }

        Ok(())
    }

    /// Orchestrate a batch of tool calls: parse args, check approval, execute.
    ///
    /// A single bad tool call (invalid args or a tool execution failure) does NOT
    /// abort the whole batch — it is recorded in [`OrchestrateOutcome::failures`] and
    /// the remaining calls keep executing. Approval denial and cancellation still
    /// hard-abort (return `Err`) since they represent a user/interrupt decision, not a
    /// per-call failure that the model can recover from mid-batch.
    pub async fn orchestrate<F>(
        &self,
        session_id: &SessionId,
        tool_calls: &[(String, String, String)],
        ctx: &ExecutionContext,
        event_rx: &mut broadcast::Receiver<RuntimeEvent>,
        on_event: Arc<Mutex<F>>,
    ) -> AgentResult<OrchestrateOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let mut results = Vec::with_capacity(tool_calls.len());
        let mut failures = Vec::new();

        for (id, name, args_str) in tool_calls {
            let args: Value = match serde_json::from_str(args_str) {
                Ok(args) => args,
                Err(_) => {
                    failures.push(ToolFailure {
                        id: id.clone(),
                        error: AgentError::ToolArgsInvalid {
                            name: name.clone(),
                            raw: args_str.clone(),
                        },
                    });
                    continue;
                }
            };

            self.process_approval(
                session_id,
                name,
                &args,
                args_str,
                ctx,
                event_rx,
                on_event.clone(),
            )
            .await?;

            match self
                .execute_tool(
                    session_id,
                    id,
                    name,
                    &args,
                    args_str,
                    ctx,
                    event_rx,
                    on_event.clone(),
                )
                .await
            {
                Ok(result) => results.push(result),
                Err(e) if e.is_cancelled() => return Err(e),
                Err(e) => failures.push(ToolFailure {
                    id: id.clone(),
                    error: e,
                }),
            }
        }

        Ok(OrchestrateOutcome { results, failures })
    }

    pub fn error_recovery(&self) -> &Arc<dyn ToolErrorRecovery> {
        &self.error_recovery
    }

    pub fn approval_handler(&self) -> Option<&Arc<dyn ApprovalHandler>> {
        self.approval_handler.as_ref()
    }

    pub fn tool_policy(&self) -> Option<&Arc<dyn ToolPolicy>> {
        self.tool_policy.as_ref()
    }

    pub fn tools_arc(&self) -> Arc<RwLock<ToolRegistry>> {
        self.tools.clone()
    }
}

#[derive(Debug)]
pub struct ToolExecutionResult {
    pub id: String,
    #[allow(dead_code)]
    pub name: String,
    pub output: Vec<Content>,
}

/// One failed tool call within an orchestrated batch.
#[derive(Debug)]
pub struct ToolFailure {
    pub id: String,
    pub error: AgentError,
}

/// Result of [`ToolEngine::orchestrate`]: successful tool calls plus any per-call
/// failures collected without aborting the rest of the batch.
#[derive(Debug)]
pub struct OrchestrateOutcome {
    pub results: Vec<ToolExecutionResult>,
    pub failures: Vec<ToolFailure>,
}

/// Grouped context passed through the tool execution call chain.
/// Reduces parameter count on `execute_tool` and `process_approval`.
pub(crate) struct ExecutionContext {
    pub session_manager: SessionManager,
    pub llm_client: Option<Arc<dyn crate::llm::StreamClient>>,
    pub language: Language,
    pub tool_timeout_ms: Option<u64>,
    pub max_output_chars: Option<usize>,
    pub cancel_token: tokio_util::sync::CancellationToken,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{
        AllowAllApprovalHandler, DenyAllApprovalHandler, InMemorySessionStore, StopOnError,
    };
    use crate::tool::Tool;
    use crate::types::{ApprovalRequest, AtomicU64SessionIdGenerator, RiskLevel, SessionConfig};
    use async_trait::async_trait;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn execute_tool_returns_not_found_for_unknown_tool() {
        // Empty registry + minimal runtime plumbing so the tool-not-found
        // branch is the only code path exercised.
        let event_bus = EventBus::new(16);
        let mut event_rx = event_bus.subscribe();
        let engine = ToolEngine::new(
            ToolRegistry::default(),
            None,
            None,
            Arc::new(StopOnError),
            event_bus,
        );

        let session_manager = SessionManager::new(
            Arc::new(AtomicU64SessionIdGenerator::default()),
            Arc::new(InMemorySessionStore::new()),
            SessionConfig::default(),
        );

        for (language, expected) in [
            (Language::En, "Tool no_such_tool not found"),
            (Language::Zh, "工具 no_such_tool 未找到"),
        ] {
            let ctx = ExecutionContext {
                session_manager: session_manager.clone(),
                llm_client: None,
                language,
                tool_timeout_ms: None,
                max_output_chars: None,
                cancel_token: CancellationToken::new(),
            };

            let session_id = SessionId::new(1);
            let result = engine
                .execute_tool(
                    &session_id,
                    "call_1",
                    "no_such_tool",
                    &Value::Null,
                    "{}",
                    &ctx,
                    &mut event_rx,
                    std::sync::Arc::new(std::sync::Mutex::new(|_| -> AgentResult<()> { Ok(()) })),
                )
                .await
                .expect("unknown tool should not error");

            assert_eq!(result.id, "call_1");
            assert_eq!(result.name, "no_such_tool");
            assert_eq!(content_text(&result.output), expected);
        }
    }

    /// Minimal tool that echoes its `text` argument back as `echo: <text>`.
    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }

        fn description(&self) -> &'static str {
            "Echo back the provided text"
        }

        fn schema(&self) -> Value {
            serde_json::json!({
                "type": "object",
                "properties": { "text": { "type": "string" } }
            })
        }

        async fn call(&self, args: &Value, _ctx: &ToolContext) -> AgentResult<Vec<Content>> {
            let text = args.get("text").and_then(Value::as_str).unwrap_or("");
            Ok(vec![Content::text(format!("echo: {text}"))])
        }
    }

    /// Policy that always requests approval, so `process_approval` reaches the handler.
    struct RequireApproval;

    #[async_trait]
    impl ToolPolicy for RequireApproval {
        async fn evaluate_approval(
            &self,
            tool_name: &str,
            _args: &Value,
        ) -> Option<ApprovalRequest> {
            Some(ApprovalRequest {
                title: format!("Approve {tool_name}"),
                message: "Approve this tool call?".to_string(),
                action_key: Some(format!("approve:{tool_name}")),
                risk_level: RiskLevel::Sensitive,
                raw: None,
            })
        }
    }

    fn session_manager() -> SessionManager {
        SessionManager::new(
            Arc::new(AtomicU64SessionIdGenerator::default()),
            Arc::new(InMemorySessionStore::new()),
            SessionConfig::default(),
        )
    }

    fn ctx(session_manager: &SessionManager) -> ExecutionContext {
        ExecutionContext {
            session_manager: session_manager.clone(),
            llm_client: None,
            language: Language::En,
            tool_timeout_ms: None,
            max_output_chars: None,
            cancel_token: CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn execute_tool_found_runs_and_emits_events() {
        let mut registry = ToolRegistry::default();
        registry.register(EchoTool);

        let event_bus = EventBus::new(16);
        let mut event_rx = event_bus.subscribe();
        let engine = ToolEngine::new(registry, None, None, Arc::new(StopOnError), event_bus);

        let sm = session_manager();
        let c = ctx(&sm);

        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        let result = engine
            .execute_tool(
                &SessionId::new(1),
                "call_1",
                "echo",
                &serde_json::json!({"text": "hi"}),
                r#"{"text":"hi"}"#,
                &c,
                &mut event_rx,
                Arc::new(Mutex::new(move |e| -> AgentResult<()> {
                    events_clone.lock().unwrap().push(e);
                    Ok(())
                })),
            )
            .await
            .expect("echo tool should execute");

        let events = events.lock().unwrap();

        assert_eq!(result.id, "call_1");
        assert_eq!(result.name, "echo");
        assert_eq!(content_text(&result.output), "echo: hi");

        let started = events.iter().find_map(|e| match e {
            RuntimeEvent::ToolCallStarted {
                tool_name,
                args_json,
                ..
            } => Some((tool_name.as_str(), args_json.as_str())),
            _ => None,
        });
        assert_eq!(started, Some(("echo", r#"{"text":"hi"}"#)));

        assert!(events.iter().any(
            |e| matches!(e, RuntimeEvent::ToolCallFinished { summary, .. } if summary == "echo: hi")
        ));
    }

    #[tokio::test]
    async fn definitions_returns_registered_tools() {
        let mut registry = ToolRegistry::default();
        registry.register(EchoTool);
        let engine = ToolEngine::new(
            registry,
            None,
            None,
            Arc::new(StopOnError),
            EventBus::new(4),
        );

        let defs = engine.definitions().await;
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0]["function"]["name"], "echo");
    }

    #[tokio::test]
    async fn process_approval_without_policy_is_noop() {
        let event_bus = EventBus::new(16);
        let mut event_rx = event_bus.subscribe();
        // No policy → auto-approve → returns Ok even with a deny-all handler.
        let engine = ToolEngine::new(
            ToolRegistry::default(),
            Some(Arc::new(DenyAllApprovalHandler)),
            None,
            Arc::new(StopOnError),
            event_bus,
        );
        let sm = session_manager();
        let c = ctx(&sm);

        engine
            .process_approval(
                &SessionId::new(1),
                "echo",
                &Value::Null,
                "{}",
                &c,
                &mut event_rx,
                std::sync::Arc::new(std::sync::Mutex::new(|_| -> AgentResult<()> { Ok(()) })),
            )
            .await
            .expect("no policy should skip approval");
    }

    #[tokio::test]
    async fn process_approval_denies_when_handler_denies() {
        let event_bus = EventBus::new(16);
        let mut event_rx = event_bus.subscribe();
        let engine = ToolEngine::new(
            ToolRegistry::default(),
            Some(Arc::new(DenyAllApprovalHandler)),
            Some(Arc::new(RequireApproval)),
            Arc::new(StopOnError),
            event_bus,
        );
        let sm = session_manager();
        let c = ctx(&sm);
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let err = engine
            .process_approval(
                &SessionId::new(1),
                "echo",
                &Value::Null,
                "{}",
                &c,
                &mut event_rx,
                Arc::new(Mutex::new(move |e| -> AgentResult<()> {
                    events_clone.lock().unwrap().push(e);
                    Ok(())
                })),
            )
            .await
            .unwrap_err();

        let events = events.lock().unwrap();

        assert!(matches!(err, AgentError::ApprovalDenied { .. }));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RuntimeEvent::AwaitingApproval { .. })),
            "should emit AwaitingApproval before denial"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RuntimeEvent::ToolCallFinished { denied: true, .. })),
            "should emit a denied ToolCallFinished"
        );
    }

    #[tokio::test]
    async fn process_approval_allows_when_handler_allows() {
        let event_bus = EventBus::new(16);
        let mut event_rx = event_bus.subscribe();
        let engine = ToolEngine::new(
            ToolRegistry::default(),
            Some(Arc::new(AllowAllApprovalHandler)),
            Some(Arc::new(RequireApproval)),
            Arc::new(StopOnError),
            event_bus,
        );
        let sm = session_manager();
        let c = ctx(&sm);

        engine
            .process_approval(
                &SessionId::new(1),
                "echo",
                &Value::Null,
                "{}",
                &c,
                &mut event_rx,
                std::sync::Arc::new(std::sync::Mutex::new(|_| -> AgentResult<()> { Ok(()) })),
            )
            .await
            .expect("allow-all handler should grant approval");
    }

    #[tokio::test]
    async fn orchestrate_executes_multiple_tool_calls() {
        let mut registry = ToolRegistry::default();
        registry.register(EchoTool);

        let event_bus = EventBus::new(16);
        let mut event_rx = event_bus.subscribe();
        let engine = ToolEngine::new(registry, None, None, Arc::new(StopOnError), event_bus);

        let sm = session_manager();
        let c = ctx(&sm);

        let outcome = engine
            .orchestrate(
                &SessionId::new(1),
                &[
                    (
                        "call_a".to_string(),
                        "echo".to_string(),
                        r#"{"text":"a"}"#.to_string(),
                    ),
                    (
                        "call_b".to_string(),
                        "echo".to_string(),
                        r#"{"text":"b"}"#.to_string(),
                    ),
                ],
                &c,
                &mut event_rx,
                std::sync::Arc::new(std::sync::Mutex::new(|_| -> AgentResult<()> { Ok(()) })),
            )
            .await
            .expect("orchestrate should succeed");

        assert_eq!(outcome.results.len(), 2);
        assert!(outcome.failures.is_empty());
        assert_eq!(outcome.results[0].id, "call_a");
        assert_eq!(content_text(&outcome.results[0].output), "echo: a");
        assert_eq!(outcome.results[1].id, "call_b");
        assert_eq!(content_text(&outcome.results[1].output), "echo: b");
    }

    #[tokio::test]
    async fn orchestrate_collects_invalid_json_args_as_failure() {
        let mut registry = ToolRegistry::default();
        registry.register(EchoTool);

        let event_bus = EventBus::new(16);
        let mut event_rx = event_bus.subscribe();
        let engine = ToolEngine::new(registry, None, None, Arc::new(StopOnError), event_bus);

        let sm = session_manager();
        let c = ctx(&sm);

        let outcome = engine
            .orchestrate(
                &SessionId::new(1),
                &[(
                    "call_x".to_string(),
                    "echo".to_string(),
                    "not-json".to_string(),
                )],
                &c,
                &mut event_rx,
                std::sync::Arc::new(std::sync::Mutex::new(|_| -> AgentResult<()> { Ok(()) })),
            )
            .await
            .expect("orchestrate should not hard-fail on bad args");

        assert!(
            outcome.results.is_empty(),
            "no tool should execute for bad args"
        );
        assert_eq!(outcome.failures.len(), 1);
        assert!(matches!(
            &outcome.failures[0].error,
            AgentError::ToolArgsInvalid { .. }
        ));
    }

    #[tokio::test]
    async fn orchestrate_continues_past_bad_args_call() {
        let mut registry = ToolRegistry::default();
        registry.register(EchoTool);

        let event_bus = EventBus::new(16);
        let mut event_rx = event_bus.subscribe();
        let engine = ToolEngine::new(registry, None, None, Arc::new(StopOnError), event_bus);

        let sm = session_manager();
        let c = ctx(&sm);

        // [good, bad-args, good] — the bad call in the middle must not stop the
        // trailing good call from executing.
        let outcome = engine
            .orchestrate(
                &SessionId::new(1),
                &[
                    (
                        "call_a".to_string(),
                        "echo".to_string(),
                        r#"{"text":"a"}"#.to_string(),
                    ),
                    (
                        "call_b".to_string(),
                        "echo".to_string(),
                        "not-json".to_string(),
                    ),
                    (
                        "call_c".to_string(),
                        "echo".to_string(),
                        r#"{"text":"c"}"#.to_string(),
                    ),
                ],
                &c,
                &mut event_rx,
                std::sync::Arc::new(std::sync::Mutex::new(|_| -> AgentResult<()> { Ok(()) })),
            )
            .await
            .expect("orchestrate should succeed even with a bad call in the middle");

        assert_eq!(outcome.results.len(), 2, "both good calls should execute");
        assert_eq!(outcome.results[0].id, "call_a");
        assert_eq!(content_text(&outcome.results[0].output), "echo: a");
        assert_eq!(outcome.results[1].id, "call_c");
        assert_eq!(content_text(&outcome.results[1].output), "echo: c");

        assert_eq!(outcome.failures.len(), 1, "only the bad call should fail");
        assert_eq!(outcome.failures[0].id, "call_b");
        assert!(matches!(
            &outcome.failures[0].error,
            AgentError::ToolArgsInvalid { .. }
        ));
    }

    #[tokio::test]
    async fn orchestrate_continues_past_execution_failure() {
        let mut registry = ToolRegistry::default();
        registry.register(EchoTool);
        registry.register(FailingTool);

        let event_bus = EventBus::new(16);
        let mut event_rx = event_bus.subscribe();
        let engine = ToolEngine::new(registry, None, None, Arc::new(StopOnError), event_bus);

        let sm = session_manager();
        let c = ctx(&sm);

        let outcome = engine
            .orchestrate(
                &SessionId::new(1),
                &[
                    (
                        "call_a".to_string(),
                        "echo".to_string(),
                        r#"{"text":"a"}"#.to_string(),
                    ),
                    (
                        "call_fail".to_string(),
                        "failing".to_string(),
                        "{}".to_string(),
                    ),
                    (
                        "call_b".to_string(),
                        "echo".to_string(),
                        r#"{"text":"b"}"#.to_string(),
                    ),
                ],
                &c,
                &mut event_rx,
                std::sync::Arc::new(std::sync::Mutex::new(|_| -> AgentResult<()> { Ok(()) })),
            )
            .await
            .expect("orchestrate should succeed even with a tool execution failure");

        assert_eq!(outcome.results.len(), 2);
        assert_eq!(outcome.results[0].id, "call_a");
        assert_eq!(outcome.results[1].id, "call_b");
        assert_eq!(outcome.failures.len(), 1);
        assert!(matches!(
            &outcome.failures[0].error,
            AgentError::ToolExecution { .. }
        ));
    }

    #[tokio::test]
    async fn getters_expose_engine_state() {
        let mut registry = ToolRegistry::default();
        registry.register(EchoTool);

        let approval = Arc::new(AllowAllApprovalHandler);
        let recovery = Arc::new(StopOnError);
        let event_bus = EventBus::new(4);
        let engine = ToolEngine::new(
            registry,
            Some(approval.clone()),
            None,
            recovery.clone(),
            event_bus,
        );

        assert!(engine.approval_handler().is_some());
        assert!(Arc::ptr_eq(
            engine.error_recovery(),
            &(recovery.clone() as Arc<dyn ToolErrorRecovery>)
        ));
        assert_eq!(engine.tools_arc().read().await.len(), 1);

        let defs = engine.definitions().await;
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0]["function"]["name"], "echo");
    }

    // ── B5: tool-failure / user-event forwarding / approval edges ────────

    struct FailingTool;
    #[async_trait]
    impl Tool for FailingTool {
        fn name(&self) -> &'static str {
            "failing"
        }
        fn description(&self) -> &'static str {
            ""
        }
        fn schema(&self) -> Value {
            serde_json::json!({})
        }
        async fn call(&self, _args: &Value, _ctx: &ToolContext) -> AgentResult<Vec<Content>> {
            Err(AgentError::internal("simulated tool failure"))
        }
    }

    struct ProgressTool;
    #[async_trait]
    impl Tool for ProgressTool {
        fn name(&self) -> &'static str {
            "progress"
        }
        fn description(&self) -> &'static str {
            ""
        }
        fn schema(&self) -> Value {
            serde_json::json!({})
        }
        async fn call(&self, _args: &Value, ctx: &ToolContext) -> AgentResult<Vec<Content>> {
            ctx.emit_progress("working");
            Ok(vec![Content::text("done")])
        }
    }

    #[tokio::test]
    async fn execute_tool_returns_tool_execution_error_on_failure() {
        let mut registry = ToolRegistry::default();
        registry.register(FailingTool);

        let event_bus = EventBus::new(16);
        let mut event_rx = event_bus.subscribe();
        let engine = ToolEngine::new(registry, None, None, Arc::new(StopOnError), event_bus);

        let sm = session_manager();

        for language in [Language::En, Language::Zh] {
            let c = ExecutionContext {
                session_manager: sm.clone(),
                llm_client: None,
                language,
                tool_timeout_ms: None,
                max_output_chars: None,
                cancel_token: CancellationToken::new(),
            };
            let events = Arc::new(Mutex::new(Vec::new()));
            let events_clone = events.clone();
            let err = engine
                .execute_tool(
                    &SessionId::new(1),
                    "call_1",
                    "failing",
                    &Value::Null,
                    "{}",
                    &c,
                    &mut event_rx,
                    Arc::new(Mutex::new(move |e| -> AgentResult<()> {
                        events_clone.lock().unwrap().push(e);
                        Ok(())
                    })),
                )
                .await
                .expect_err("failing tool should error");

            let events = events.lock().unwrap();

            assert!(
                matches!(err, AgentError::ToolExecution { .. }),
                "expected ToolExecution, got {err:?}"
            );
            assert!(
                events.iter().any(
                    |e| matches!(e, RuntimeEvent::ToolCallFinished { summary, .. }
                        if summary.contains("failed") || summary.contains("失败"))
                ),
                "should emit error ToolCallFinished"
            );
        }
    }

    #[tokio::test]
    async fn execute_tool_forwards_user_events() {
        let mut registry = ToolRegistry::default();
        registry.register(ProgressTool);

        let event_bus = EventBus::new(16);
        let mut event_rx = event_bus.subscribe();
        let engine = ToolEngine::new(registry, None, None, Arc::new(StopOnError), event_bus);

        let sm = session_manager();
        let c = ctx(&sm);

        let forwarded = Arc::new(Mutex::new(Vec::new()));
        let forwarded_clone = forwarded.clone();
        let result = engine
            .execute_tool(
                &SessionId::new(1),
                "call_1",
                "progress",
                &Value::Null,
                "{}",
                &c,
                &mut event_rx,
                Arc::new(Mutex::new(move |e| -> AgentResult<()> {
                    forwarded_clone.lock().unwrap().push(e);
                    Ok(())
                })),
            )
            .await
            .expect("progress tool should execute");

        let forwarded = forwarded.lock().unwrap();

        assert_eq!(content_text(&result.output), "done");
        assert!(
            forwarded.iter().any(|e| matches!(
                e,
                RuntimeEvent::UserEvent {
                    event: UserEvent::Progress { .. },
                    ..
                }
            )),
            "should forward progress as a UserEvent"
        );
    }

    #[tokio::test]
    async fn process_approval_skips_when_cached() {
        let event_bus = EventBus::new(16);
        let mut event_rx = event_bus.subscribe();
        let engine = ToolEngine::new(
            ToolRegistry::default(),
            Some(Arc::new(DenyAllApprovalHandler)),
            Some(Arc::new(RequireApproval)),
            Arc::new(StopOnError),
            event_bus,
        );
        let sm = session_manager();
        let c = ctx(&sm);
        let sid = sm.create_session(None).await;

        // Pre-cache the approval with the same action_key the policy emits.
        sm.cache_approval(&sid, "approve:echo".to_string()).await;

        // Cached approval short-circuits even though the handler would deny.
        engine
            .process_approval(
                &sid,
                "echo",
                &Value::Null,
                "{}",
                &c,
                &mut event_rx,
                std::sync::Arc::new(std::sync::Mutex::new(|_| -> AgentResult<()> { Ok(()) })),
            )
            .await
            .expect("cached approval should skip");
    }

    #[tokio::test]
    async fn process_approval_denies_when_no_handler() {
        let event_bus = EventBus::new(16);
        let mut event_rx = event_bus.subscribe();
        let engine = ToolEngine::new(
            ToolRegistry::default(),
            None, // no approval handler
            Some(Arc::new(RequireApproval)),
            Arc::new(StopOnError),
            event_bus,
        );
        let sm = session_manager();
        let c = ctx(&sm);

        let err = engine
            .process_approval(
                &SessionId::new(1),
                "echo",
                &Value::Null,
                "{}",
                &c,
                &mut event_rx,
                std::sync::Arc::new(std::sync::Mutex::new(|_| -> AgentResult<()> { Ok(()) })),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, AgentError::ApprovalDenied { .. }));
    }
}
