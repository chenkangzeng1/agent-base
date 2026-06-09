use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::engine::{EventBus, PlanGenerator, PlanStore, StepExecutor};
use crate::engine::plan::RecoveryPolicy;
use crate::engine::circuit_breaker::CircuitBreaker;
use crate::tool::{Tool, ToolContext, ToolControlFlow, ToolOutput};
use crate::types::{AgentError, AgentEvent, AgentResult, PlanStatus, RuntimeEvent, StepStatus};

use log;

/// PlanOrchestrator is a domain-agnostic tool for creating execution plans.
/// It delegates plan generation to a `PlanGenerator` implementation and
/// stores the plan via a `PlanStore`.
#[derive(Clone)]
pub struct PlanOrchestrator {
    plan_generator: Arc<dyn PlanGenerator>,
    step_executor: Arc<dyn StepExecutor>,
    plan_store: Arc<dyn PlanStore>,
    event_bus: Option<EventBus>,
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
            event_bus: None,
        }
    }

    /// Inject the internal event bus. Called by `AgentBuilder::build()`.
    pub(crate) fn set_event_bus(&mut self, event_bus: EventBus) {
        self.event_bus = Some(event_bus);
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

    fn as_any(&self) -> Option<&dyn std::any::Any> { Some(self) }

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
                if let Some(bus) = &self.event_bus {
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

                let _ = self.event_bus.as_ref().expect("EventBus must be injected by AgentBuilder::build()").emit(AgentEvent::PlanGenerated {
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
                let _ = self.event_bus.as_ref().expect("EventBus must be injected by AgentBuilder::build()").emit(AgentEvent::PlanFailed {
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
    event_bus: Option<EventBus>,
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
            event_bus: None,
            recovery_policy: None,
            circuit_breaker: None,
        }
    }

    /// Inject the internal event bus. Called by `AgentBuilder::build()`.
    pub(crate) fn set_event_bus(&mut self, event_bus: EventBus) {
        self.event_bus = Some(event_bus);
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

    fn as_any(&self) -> Option<&dyn std::any::Any> { Some(self) }

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
        let plan_metadata = plan_data.metadata.clone();
        let mut step_outputs = serde_json::json!({});
        let mut execution_summary = if is_zh {
            format!("计划 '{}' 的执行结果:\n", objective)
        } else {
            format!("Execution results for plan '{}':\n", objective)
        };
        let mut step_results = Vec::new();
        let mut overall_success = true;
        let mut failed_step_name: Option<String> = None;
        let mut _completed_count = 0usize;
        let mut global_step_index = 0usize;

        'phase_loop: for phase_idx in 0..plan.phases.len() {
            plan.phases[phase_idx].status = crate::types::PhaseStatus::Running;

            for step_idx in 0..plan.phases[phase_idx].steps.len() {
                if plan.phases[phase_idx].steps[step_idx].status != StepStatus::Pending {
                    global_step_index += 1;
                    continue;
                }

                let step_id = plan.phases[phase_idx].steps[step_idx].id.clone();
                let step_desc = plan.phases[phase_idx].steps[step_idx].description.clone();
                let step_payload = plan.phases[phase_idx].steps[step_idx].payload.clone();

                log::info!(
                    "PlanExecTool: executing step {} ({})",
                    step_id,
                    step_desc
                );

                plan.phases[phase_idx].steps[step_idx].status = StepStatus::Running;

                let _ = self.event_bus.as_ref().expect("EventBus must be injected by AgentBuilder::build()").emit(AgentEvent::PlanStepStarted {
                    session_id: ctx.session_id.clone(),
                    step_id: step_id.clone(),
                    step_description: step_desc.clone(),
                    payload: Some(step_payload.clone()),
                });

                execution_summary.push_str(&if is_zh {
                    format!("步骤 {}: {}\n", global_step_index + 1, step_desc)
                } else {
                    format!("Step {}: {}\n", global_step_index + 1, step_desc)
                });

                // Circuit breaker check — skip step if circuit is open
                if let Some(weak_cb) = &self.circuit_breaker
                    && let Some(cb) = weak_cb.upgrade()
                    && !cb.is_available()
                {
                    log::warn!(
                        "PlanExecTool: circuit breaker open, skipping step {}",
                        step_id
                    );
                    plan.phases[phase_idx].steps[step_idx].status = StepStatus::Skipped;
                    execution_summary.push_str(&if is_zh {
                        "  [跳过] 熔断器开启，跳过该步骤\n"
                    } else {
                        "  [Skipped] Circuit breaker open, skipping step\n"
                    });
                    overall_success = false;
                    global_step_index += 1;
                    continue;
                }

                match self
                    .step_executor
                    .execute_step(&plan.phases[phase_idx].steps[step_idx], &plan_metadata, ctx)
                    .await
                {
                    Ok(result) => {
                        let step_success = result.success;
                        log::info!(
                            "PlanExecTool: step {} completed with success={}",
                            step_id,
                            step_success
                        );

                        // Record circuit breaker outcome
                        if let Some(weak_cb) = &self.circuit_breaker
                            && let Some(cb) = weak_cb.upgrade()
                        {
                            if step_success {
                                cb.record_success();
                            } else {
                                cb.record_failure();
                            }
                        }

                        plan.phases[phase_idx].steps[step_idx].status = if step_success {
                            StepStatus::Completed
                        } else {
                            StepStatus::Failed
                        };
                        plan.phases[phase_idx].steps[step_idx].result = Some(result.clone());

                        // Accumulate step output into step_outputs
                        if let serde_json::Value::Object(ref mut map) = step_outputs {
                            if step_success {
                                let output_val = result.output.as_deref()
                                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                                    .unwrap_or_else(|| serde_json::json!(result.output));
                                map.insert(step_id.clone(), output_val);
                            } else {
                                map.insert(step_id.clone(), serde_json::json!({
                                    "error": result.output.as_deref().unwrap_or(""),
                                    "success": false
                                }));
                            }
                        }

                        execution_summary.push_str(&if is_zh {
                            format!(
                                "  结果: {}\n",
                                if step_success { "成功" } else { "失败" }
                            )
                        } else {
                            format!(
                                "  Result: {}\n",
                                if step_success { "OK" } else { "FAILED" }
                            )
                        });

                        let _ = self.event_bus.as_ref().expect("EventBus must be injected by AgentBuilder::build()").emit(AgentEvent::PlanStepCompleted {
                            session_id: ctx.session_id.clone(),
                            step_id: step_id.clone(),
                            success: step_success,
                            result: result.output.clone(),
                            payload: Some(step_payload.clone()),
                        });

                        if step_success {
                            _completed_count += 1;
                        } else {
                            if let Some(ref output) = result.output {
                                execution_summary.push_str(&if is_zh {
                                    format!("  错误: {output}\n")
                                } else {
                                    format!("  Error: {output}\n")
                                });
                            }

                            match self
                                .recovery
                                .handle_step_failure(
                                    &plan.phases[phase_idx].steps[step_idx],
                                    result.output.as_deref().unwrap_or(""),
                                    0,
                                    &plan,
                                    &step_outputs,
                                )
                                .await
                            {
                                Ok(action) => match action {
                                    crate::types::RecoveryAction::Retry => {
                                        execution_summary.push_str(
                                            if is_zh {
                                                "  [重试] 系统建议重试该步骤（计划标记为未完全成功）\n"
                                            } else {
                                                "  [Retry] System suggests retrying this step (plan marked as not fully successful)\n"
                                            },
                                        );
                                        overall_success = false;
                                    }
                                    crate::types::RecoveryAction::Skip => {
                                        execution_summary.push_str(
                                            if is_zh {
                                                "  [跳过] 系统建议跳过该步骤（计划标记为未完全成功）\n"
                                            } else {
                                                "  [Skip] System suggests skipping this step (plan marked as not fully successful)\n"
                                            },
                                        );
                                        plan.phases[phase_idx].steps[step_idx].status = StepStatus::Skipped;
                                        overall_success = false;
                                    }
                                    crate::types::RecoveryAction::Abort => {
                                        execution_summary.push_str(
                                            if is_zh {
                                                "  [中止] 系统建议中止计划\n"
                                            } else {
                                                "  [Abort] System suggests aborting the plan\n"
                                            },
                                        );
                                        overall_success = false;
                                        failed_step_name = Some(step_desc.clone());
                                        plan.phases[phase_idx].status = crate::types::PhaseStatus::Failed;
                                        break 'phase_loop;
                                    }
                                    // Alternative/Replan not handled in PlanExecTool path
                                    _ => {
                                        overall_success = false;
                                        failed_step_name = Some(step_desc.clone());
                                        plan.phases[phase_idx].status = crate::types::PhaseStatus::Failed;
                                        break 'phase_loop;
                                    }
                                },
                                Err(_e) => {
                                    overall_success = false;
                                    failed_step_name = Some(step_desc.clone());
                                    plan.phases[phase_idx].status = crate::types::PhaseStatus::Failed;
                                    break 'phase_loop;
                                }
                            }
                        }

                        step_results.push(json!({
                            "step": step_desc,
                            "success": step_success,
                            "output": result.output,
                        }));
                    }
                    Err(e) => {
                        // Record circuit breaker failure
                        if let Some(weak_cb) = &self.circuit_breaker
                            && let Some(cb) = weak_cb.upgrade()
                        {
                            cb.record_failure();
                        }

                        plan.phases[phase_idx].steps[step_idx].status = StepStatus::Failed;
                        execution_summary.push_str(&if is_zh {
                            format!("  执行错误: {e}\n")
                        } else {
                            format!("  Execution error: {e}\n")
                        });

                        let _ = self.event_bus.as_ref().expect("EventBus must be injected by AgentBuilder::build()").emit(AgentEvent::PlanStepCompleted {
                            session_id: ctx.session_id.clone(),
                            step_id: step_id.clone(),
                            success: false,
                            result: Some(e.to_string()),
                            payload: Some(step_payload.clone()),
                        });

                        // Use RecoveryPolicy for error-kind-aware decision when available
                        if let Some(ref policy) = self.recovery_policy {
                            let action = policy.with_context(&e, 0);
                            match action {
                                crate::types::RecoveryAction::Retry => {
                                    execution_summary.push_str(
                                        if is_zh {
                                            "  [重试] 系统建议重试该步骤（基于错误类型）\n"
                                        } else {
                                            "  [Retry] System suggests retrying this step (error-kind based)\n"
                                        },
                                    );
                                    overall_success = false;
                                    // Don't break — continue to next step (retry would need loop restructuring)
                                }
                                crate::types::RecoveryAction::Skip => {
                                    execution_summary.push_str(
                                        if is_zh {
                                            "  [跳过] 系统建议跳过该步骤\n"
                                        } else {
                                            "  [Skip] System suggests skipping this step\n"
                                        },
                                    );
                                    plan.phases[phase_idx].steps[step_idx].status = StepStatus::Skipped;
                                    overall_success = false;
                                }
                                crate::types::RecoveryAction::Abort => {
                                    overall_success = false;
                                    failed_step_name = Some(step_desc.clone());
                                    plan.phases[phase_idx].status = crate::types::PhaseStatus::Failed;
                                    break 'phase_loop;
                                }
                                // Alternative/Replan not handled in PlanExecTool path
                                _ => {
                                    overall_success = false;
                                    failed_step_name = Some(step_desc.clone());
                                    plan.phases[phase_idx].status = crate::types::PhaseStatus::Failed;
                                    break 'phase_loop;
                                }
                            }
                        } else {
                            overall_success = false;
                            failed_step_name = Some(step_desc.clone());
                            plan.phases[phase_idx].status = crate::types::PhaseStatus::Failed;
                            break 'phase_loop;
                        }
                    }
                }

                global_step_index += 1;
            }

            // Phase completed or already marked failed
            if plan.phases[phase_idx].status == crate::types::PhaseStatus::Running {
                plan.phases[phase_idx].status = if plan.phases[phase_idx].has_failed() {
                    crate::types::PhaseStatus::Failed
                } else {
                    crate::types::PhaseStatus::Completed
                };
            }

            // Skip remaining phases if any step in this phase failed
            if !overall_success {
                break 'phase_loop;
            }
        }

        plan.status = if overall_success && plan.is_completed() {
            PlanStatus::Completed
        } else if plan.has_failed() {
            PlanStatus::Failed
        } else {
            PlanStatus::Executing
        };

        self.plan_store
            .save_plan(&plan, plan_data.metadata)
            .await?;

        if overall_success {
            execution_summary.push_str(
                if is_zh { "所有步骤执行完毕。" } else { "All steps completed." }
            );
        }

        let _ = self.event_bus.as_ref().expect("EventBus must be injected by AgentBuilder::build()").emit(AgentEvent::PlanCompleted {
            session_id: ctx.session_id.clone(),
            plan_id: plan_id.clone(),
            success: overall_success,
        });

        Ok(ToolOutput {
            summary: execution_summary,
            raw: Some(json!({
                "objective": objective,
                "plan_id": plan_id,
                "steps": step_results,
                "success": overall_success,
                "failed_step": failed_step_name,
            })),
            control_flow: ToolControlFlow::Continue,
            truncation: None,
        })
    }
}
