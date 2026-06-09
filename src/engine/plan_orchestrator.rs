use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::engine::{EventBus, PlanGenerator, PlanStore, StepExecutor};
use crate::engine::plan::{RecoveryPolicy, PlanConfig};
use crate::engine::circuit_breaker::CircuitBreaker;
use crate::engine::runtime::PlanRunner;
use crate::tool::{FrameworkTool, Tool, ToolContext, ToolControlFlow, ToolOutput};
use crate::types::{AgentError, AgentEvent, AgentResult, RuntimeEvent};

/// PlanOrchestrator is a domain-agnostic tool for creating execution plans.
/// It delegates plan generation to a `PlanGenerator` implementation and
/// stores the plan via a `PlanStore`.
#[derive(Clone)]
pub struct PlanOrchestrator {
    plan_generator: Arc<dyn PlanGenerator>,
    step_executor: Arc<dyn StepExecutor>,
    plan_store: Arc<dyn PlanStore>,
    event_bus: std::sync::OnceLock<EventBus>,
}

impl PlanOrchestrator {
    pub fn new(
        plan_generator: Arc<dyn PlanGenerator>,
        step_executor: Arc<dyn StepExecutor>,
        plan_store: Arc<dyn PlanStore>,
    ) -> Self {
        Self {
            plan_generator,
            step_executor,
            plan_store,
            event_bus: std::sync::OnceLock::new(),
        }
    }

    pub fn with_step_executor(&mut self, step_executor: Arc<dyn StepExecutor>) {
        self.step_executor = step_executor;
    }
}

#[async_trait]
impl Tool for PlanOrchestrator {
    fn name(&self) -> &'static str {
        "create_plan"
    }

    #[allow(private_interfaces)]
    fn as_framework_tool(&self) -> Option<&dyn FrameworkTool> { Some(self) }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "create_plan",
                "description": "Analyze a task and generate an execution plan (without executing commands). Used for complex tasks that require multiple steps. The system will analyze the objective and generate a plan; after user review and confirmation, use execute_plan to execute it.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "objective": {
                            "type": "string",
                            "description": "The overall goal of the task, e.g. 'check disk space', 'troubleshoot network issues'"
                        },
                        "context": {
                            "type": "string",
                            "description": "Additional context information, such as target host, environment variables, etc."
                        }
                    },
                    "required": ["objective"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let objective = args
            .get("objective")
            .and_then(Value::as_str)
            .unwrap_or("unnamed task")
            .to_string();
        let context = args
            .get("context")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let plan_id = {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            static COUNTER: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            format!("plan-{timestamp}-{count}")
        };

        let tools = vec![];

        // Create channel for streaming plan generation events
        let (plan_event_tx, mut plan_event_rx) = tokio::sync::mpsc::unbounded_channel();

        match self
            .plan_generator
            .generate_plan(
                &objective,
                &context,
                &tools,
                Some(plan_event_tx),
            )
            .await
        {
            Ok(mut plan) => {
                plan.id = plan_id.clone();
                plan.objective = objective.clone();
                plan.status = crate::types::PlanStatus::AwaitingConfirmation;

                // Drain plan generation events and emit to EventBus
                if let Some(bus) = self.event_bus.get() {
                    while let Ok(event) = plan_event_rx.try_recv() {
                        let agent_event = match event {
                            RuntimeEvent::PlanGenerating { .. } => AgentEvent::PlanGenerating {
                                session_id: ctx.session_id.clone(),
                                plan_id: plan_id.clone(),
                            },
                            RuntimeEvent::PlanStepParsed { step_index, step_id, step_description, .. } => {
                                AgentEvent::PlanStepParsed {
                                    session_id: ctx.session_id.clone(),
                                    plan_id: plan_id.clone(),
                                    step_index,
                                    step_id,
                                    step_description,
                                }
                            }
                            RuntimeEvent::ThoughtDelta { text, .. } => AgentEvent::ThoughtDelta {
                                session_id: ctx.session_id.clone(),
                                text,
                            },
                            _ => continue,
                        };
                        bus.emit(agent_event);
                    }
                }

                self.plan_store
                    .save_plan(&plan, json!({"session_id": ctx.session_id.to_string()}))
                    .await?;

                let _ = self.event_bus.get().expect("EventBus must be injected by AgentBuilder::build()").emit(AgentEvent::PlanGenerated {
                    session_id: ctx.session_id.clone(),
                    plan: plan.clone(),
                });

                let step_details: Vec<Value> = plan
                    .all_steps()
                    .map(|s| {
                        json!({
                            "id": s.id,
                            "description": s.description,
                        })
                    })
                    .collect();

                let summary = if ctx.language == crate::types::Language::Zh {
                    format!(
                        "计划已生成，包含 {} 个步骤，等待用户确认。计划ID: {}",
                        plan.total_steps(),
                        plan_id
                    )
                } else {
                    format!(
                        "Plan generated with {} steps, awaiting user confirmation. plan_id: {}",
                        plan.total_steps(),
                        plan_id
                    )
                };

                Ok(ToolOutput {
                    summary,
                    raw: Some(json!({
                        "objective": objective,
                        "plan_id": plan_id,
                        "steps_count": plan.total_steps(),
                        "steps": step_details,
                        "success": true,
                        "status": "awaiting_confirmation",
                    })),
                    control_flow: ToolControlFlow::Continue,
                    truncation: None,
                })
            }
            Err(e) => {
                let _ = self.event_bus.get().expect("EventBus must be injected by AgentBuilder::build()").emit(AgentEvent::PlanFailed {
                    session_id: ctx.session_id.clone(),
                    plan_id: plan_id.clone(),
                    error: e.to_string(),
                });

                let summary = if ctx.language == crate::types::Language::Zh {
                    format!("计划生成失败: {e}")
                } else {
                    format!("Plan generation failed: {e}")
                };

                Ok(ToolOutput {
                    summary,
                    raw: Some(json!({
                        "objective": objective,
                        "plan_id": plan_id,
                        "success": false,
                        "error": e.to_string(),
                    })),
                    control_flow: ToolControlFlow::Continue,
                    truncation: None,
                })
            }
        }
    }
}

impl FrameworkTool for PlanOrchestrator {
    fn set_event_bus(&self, event_bus: EventBus) {
        let _ = self.event_bus.set(event_bus);
    }
}

/// PlanExecTool is a domain-agnostic tool for executing previously created plans.
///
/// Steps are executed by the configured StepExecutor. If the StepExecutor is
/// ToolCallingStepExecutor, each step's payload should contain `tool_name` and
/// `args`, and the target tool itself handles any confirmation flow internally.
///
/// Optionally supports adaptive recovery via `RecoveryPolicy` and fault isolation
/// via `CircuitBreaker` (held as `Weak` to avoid preventing cleanup).
#[derive(Clone)]
pub struct PlanExecTool {
    step_executor: Arc<dyn StepExecutor>,
    plan_store: Arc<dyn PlanStore>,
    recovery: Arc<dyn crate::engine::RecoveryStrategy>,
    event_bus: std::sync::OnceLock<EventBus>,
    /// Deferred injection: set once after PlanRunner is constructed.
    /// Uses `OnceLock` to break the circular dependency (PlanRunner owns
    /// ToolEngine which owns this tool, but this tool needs a ref to PlanRunner).
    plan_runner: std::sync::OnceLock<std::sync::Weak<PlanRunner>>,
    recovery_policy: Option<Arc<RecoveryPolicy>>,
    circuit_breaker: Option<std::sync::Weak<CircuitBreaker>>,
}

impl PlanExecTool {
    pub fn new(
        step_executor: Arc<dyn StepExecutor>,
        plan_store: Arc<dyn PlanStore>,
        recovery: Arc<dyn crate::engine::RecoveryStrategy>,
    ) -> Self {
        Self {
            step_executor,
            plan_store,
            recovery,
            event_bus: std::sync::OnceLock::new(),
            plan_runner: std::sync::OnceLock::new(),
            recovery_policy: None,
            circuit_breaker: None,
        }
    }

    /// Set an adaptive recovery policy for error-kind-aware decisions.
    pub fn with_recovery_policy(mut self, policy: Arc<RecoveryPolicy>) -> Self {
        self.recovery_policy = Some(policy);
        self
    }

    /// Set a circuit breaker (via `Weak` reference) for fault isolation.
    pub fn with_circuit_breaker(mut self, cb: std::sync::Weak<CircuitBreaker>) -> Self {
        self.circuit_breaker = Some(cb);
        self
    }
}

#[async_trait]
impl Tool for PlanExecTool {
    fn name(&self) -> &'static str {
        "execute_plan"
    }

    #[allow(private_interfaces)]
    fn as_framework_tool(&self) -> Option<&dyn FrameworkTool> { Some(self) }

    fn definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "execute_plan",
                "description": "Execute a previously generated plan. First use create_plan to generate a plan, then after user review and confirmation, use this tool to execute it.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "plan_id": {
                            "type": "string",
                            "description": "The plan ID to execute (obtained from the create_plan result)"
                        }
                    },
                    "required": ["plan_id"]
                }
            }
        })
    }

    async fn call(&self, args: &Value, ctx: &ToolContext) -> AgentResult<ToolOutput> {
        let plan_id = args
            .get("plan_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let is_zh = ctx.language == crate::types::Language::Zh;

        if plan_id.is_empty() {
            let summary = if is_zh {
                "缺少 plan_id 参数".to_string()
            } else {
                "Missing plan_id parameter".to_string()
            };
            return Ok(ToolOutput {
                summary,
                raw: Some(json!({
                    "success": false,
                    "error": "plan_id is required",
                })),
                control_flow: ToolControlFlow::Continue,
                truncation: None,
            });
        }

        let runner = self.plan_runner.get()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| {
                AgentError::internal("PlanExecTool: PlanRunner not available (dropped or not injected)")
            })?;

        let plan_data = self
            .plan_store
            .load_plan(&plan_id)
            .await?
            .ok_or_else(|| AgentError::plan_storage(
                if is_zh {
                    format!("计划 {plan_id} 不存在，可能已过期或从未创建")
                } else {
                    format!("Plan {plan_id} does not exist, it may have expired or never been created")
                }
            ))?;

        let mut plan = plan_data.plan;
        let objective = plan.objective.clone();
        
        let mut config = PlanConfig::new()
            .with_executor(self.step_executor.clone())
            .recovery(self.recovery.clone());
            
        if let Some(ref policy) = self.recovery_policy {
            config = config.recovery_policy(policy.clone());
        }
        
        // Use the runner to execute the plan steps with full adaptive recovery support
        let mut event_rx = runner.event_bus.subscribe();
        
        let outcome = runner.run_plan_steps(
            &ctx.session_id,
            &mut plan,
            &config,
            &mut event_rx,
            &mut |_| Ok(()), // Events are already emitted by the runner to the bus
        ).await?;

        // Save the updated plan state
        self.plan_store.save_plan(&plan, plan_data.metadata).await?;

        let (summary, success, outcome_str) = match &outcome {
            crate::types::RunOutcome::Completed => (
                if is_zh { format!("计划 '{}' 执行成功。", objective) } else { format!("Plan '{}' completed successfully.", objective) },
                true,
                "completed",
            ),
            crate::types::RunOutcome::Failed { error } => (
                if is_zh { format!("计划 '{}' 执行失败: {}", objective, error) } else { format!("Plan '{}' failed: {}", objective, error) },
                false,
                "failed",
            ),
            crate::types::RunOutcome::MaxTurnsExceeded { .. } => (
                if is_zh { format!("计划 '{}' 执行中断。", objective) } else { format!("Plan '{}' execution interrupted.", objective) },
                false,
                "max_turns_exceeded",
            ),
            crate::types::RunOutcome::Cancelled => (
                if is_zh { format!("计划 '{}' 执行中断。", objective) } else { format!("Plan '{}' execution interrupted.", objective) },
                false,
                "cancelled",
            ),
        };

        Ok(ToolOutput {
            summary,
            raw: Some(json!({
                "objective": objective,
                "plan_id": plan_id,
                "success": success,
                "outcome": outcome_str,
            })),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}

impl FrameworkTool for PlanExecTool {
    fn set_event_bus(&self, event_bus: EventBus) {
        let _ = self.event_bus.set(event_bus);
    }

    fn set_plan_runner(&self, runner: &Arc<PlanRunner>) {
        let _ = self.plan_runner.set(Arc::downgrade(runner));
    }
}
