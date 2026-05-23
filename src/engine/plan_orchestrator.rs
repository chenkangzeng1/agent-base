use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::engine::{PlanGenerator, PlanStore, StepExecutor};
use crate::tool::{Tool, ToolContext, ToolControlFlow, ToolOutput};
use crate::types::{AgentError, AgentEvent, AgentResult, PlanStatus, StepStatus};

/// PlanOrchestrator is a domain-agnostic tool for creating execution plans.
/// It delegates plan generation to a `PlanGenerator` implementation and
/// stores the plan via a `PlanStore`.
#[derive(Clone)]
pub struct PlanOrchestrator {
    plan_generator: Arc<dyn PlanGenerator>,
    step_executor: Arc<dyn StepExecutor>,
    plan_store: Arc<dyn PlanStore>,
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

        let event_bus_g = ctx.event_bus.clone();
        let session_id_g = ctx.session_id.clone();
        let plan_id_g = plan_id.clone();
        let on_generating = Box::new(move || {
            let _ = event_bus_g.send(AgentEvent::PlanGenerating {
                session_id: session_id_g.clone(),
                plan_id: plan_id_g.clone(),
            });
        });

        let event_bus_s = ctx.event_bus.clone();
        let session_id_s = ctx.session_id.clone();
        let plan_id_s = plan_id.clone();
        let on_step_parsed = Box::new(move |index: usize, step_id: String, description: String| {
            let _ = event_bus_s.send(AgentEvent::PlanStepParsed {
                session_id: session_id_s.clone(),
                plan_id: plan_id_s.clone(),
                step_index: index,
                step_id,
                step_description: description,
            });
        });

        let event_bus_t = ctx.event_bus.clone();
        let session_id_t = ctx.session_id.clone();
        let on_raw_chunk = Box::new(move |text: String| {
            let _ = event_bus_t.send(AgentEvent::ThoughtDelta {
                session_id: session_id_t.clone(),
                text,
            });
        });

        let tools = vec![];

        match self
            .plan_generator
            .generate_plan_streaming(
                &objective,
                &context,
                &tools,
                on_generating,
                on_step_parsed,
                on_raw_chunk,
            )
            .await
        {
            Ok(mut plan) => {
                plan.id = plan_id.clone();
                plan.objective = objective.clone();

                self.plan_store
                    .save_plan(&plan, json!({"session_id": ctx.session_id.to_string()}))
                    .await?;

                let _ = ctx.event_bus.send(AgentEvent::PlanGenerated {
                    session_id: ctx.session_id.clone(),
                    plan: plan.clone(),
                });

                let step_details: Vec<Value> = plan
                    .steps
                    .iter()
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
                        plan.steps.len(),
                        plan_id
                    )
                } else {
                    format!(
                        "Plan generated with {} steps, awaiting user confirmation. plan_id: {}",
                        plan.steps.len(),
                        plan_id
                    )
                };

                Ok(ToolOutput {
                    summary,
                    raw: Some(json!({
                        "objective": objective,
                        "plan_id": plan_id,
                        "steps_count": plan.steps.len(),
                        "steps": step_details,
                        "success": true,
                        "status": "awaiting_confirmation",
                    })),
                    control_flow: ToolControlFlow::Continue,
                    truncation: None,
                })
            }
            Err(e) => {
                let _ = ctx.event_bus.send(AgentEvent::PlanFailed {
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
#[derive(Clone)]
pub struct PlanExecTool {
    step_executor: Arc<dyn StepExecutor>,
    plan_store: Arc<dyn PlanStore>,
    recovery: Arc<dyn crate::engine::RecoveryStrategy>,
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
        }
    }
}

#[async_trait]
impl Tool for PlanExecTool {
    fn name(&self) -> &'static str {
        "execute_plan"
    }

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
        let mut execution_summary = if is_zh {
            format!("计划 '{}' 的执行结果:\n", objective)
        } else {
            format!("Execution results for plan '{}':\n", objective)
        };
        let mut step_results = Vec::new();
        let mut overall_success = true;
        let mut failed_step_name: Option<String> = None;
        let mut _completed_count = 0usize;

        for (index, step) in plan.steps.iter_mut().enumerate() {
            if step.status != StepStatus::Pending {
                continue;
            }

            step.status = StepStatus::Running;

            let _ = ctx.event_bus.send(AgentEvent::PlanStepStarted {
                session_id: ctx.session_id.clone(),
                step_id: step.id.clone(),
                step_description: step.description.clone(),
            });

            execution_summary.push_str(&if is_zh {
                format!("步骤 {}: {}\n", index + 1, step.description)
            } else {
                format!("Step {}: {}\n", index + 1, step.description)
            });

            match self
                .step_executor
                .execute_step(step, &plan_data.metadata)
                .await
            {
                Ok(result) => {
                    let step_success = result.success;
                    step.status = if step_success {
                        StepStatus::Completed
                    } else {
                        StepStatus::Failed
                    };
                    step.result = Some(result.clone());

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

                    let _ = ctx.event_bus.send(AgentEvent::PlanStepCompleted {
                        session_id: ctx.session_id.clone(),
                        step_id: step.id.clone(),
                        success: step_success,
                        result: result.output.clone(),
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
                                step,
                                result.output.as_deref().unwrap_or(""),
                                0,
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
                                    step.status = StepStatus::Skipped;
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
                                    failed_step_name = Some(step.description.clone());
                                    break;
                                }
                            },
                            Err(_e) => {
                                overall_success = false;
                                failed_step_name = Some(step.description.clone());
                                break;
                            }
                        }
                    }

                    step_results.push(json!({
                        "step": step.description,
                        "success": step_success,
                        "output": result.output,
                    }));
                }
                Err(e) => {
                    step.status = StepStatus::Failed;
                    execution_summary.push_str(&if is_zh {
                        format!("  执行错误: {e}\n")
                    } else {
                        format!("  Execution error: {e}\n")
                    });

                    let _ = ctx.event_bus.send(AgentEvent::PlanStepCompleted {
                        session_id: ctx.session_id.clone(),
                        step_id: step.id.clone(),
                        success: false,
                        result: Some(e.to_string()),
                    });

                    overall_success = false;
                    failed_step_name = Some(step.description.clone());
                    break;
                }
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

        let _ = ctx.event_bus.send(AgentEvent::PlanCompleted {
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
