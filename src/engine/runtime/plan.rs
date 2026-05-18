use serde_json::json;
use tokio::sync::broadcast;

use crate::types::{
    AgentError, AgentEvent, AgentResult, ExecutionPlan, PlanStatus, RecoveryAction,
    RunOutcome, SessionId, StepStatus,
};
use crate::engine::plan::{PlanExecutor, PlanStore};
use super::AgentRuntime;
use std::sync::Arc;

impl AgentRuntime {
    pub async fn run_with_plan<F>(
        &mut self,
        session_id: SessionId,
        objective: &str,
        plan_executor: Arc<dyn PlanExecutor>,
        plan_store: Option<Arc<dyn PlanStore>>,
        mut on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let tool_definitions = self.tools.definitions();
        let mut event_rx = self.subscribe_events();

        let mut plan = plan_executor
            .generate_plan(objective, "", &tool_definitions)
            .await
            .map_err(|e| AgentError::plan_generation(e.to_string()))?;

        self.emit_and_drain(
            AgentEvent::PlanGenerated {
                session_id: session_id.clone(),
                plan: plan.clone(),
            },
            &mut event_rx,
            &mut on_event,
        );

        if let Some(store) = &plan_store {
            store
                .save_plan(&plan, json!({}))
                .await
                .map_err(|e| AgentError::plan_storage(e.to_string()))?;
        }

        plan.status = PlanStatus::Executing;

        for i in 0..plan.steps.len() {
            plan.steps[i].status = StepStatus::Running;

            self.emit_and_drain(
                AgentEvent::PlanStepStarted {
                    session_id: session_id.clone(),
                    step_id: plan.steps[i].id.clone(),
                    step_description: plan.steps[i].description.clone(),
                },
                &mut event_rx,
                &mut on_event,
            );

            let step = &plan.steps[i];
            let step_result = self
                .execute_plan_step(&session_id, step, &plan, &plan_executor, &mut event_rx, &mut on_event)
                .await;

            match step_result {
                Ok(result) => {
                    plan.steps[i].result = Some(result.clone());
                    if result.success {
                        plan.steps[i].status = StepStatus::Completed;

                        self.emit_and_drain(
                            AgentEvent::PlanStepCompleted {
                                session_id: session_id.clone(),
                                step_id: plan.steps[i].id.clone(),
                                success: true,
                                result: result.output,
                            },
                            &mut event_rx,
                            &mut on_event,
                        );

                        if let Some(store) = &plan_store {
                            let _ = store.save_plan(&plan, json!({})).await;
                        }
                    } else {
                        let error = result.error.unwrap_or_default();
                        let step = &plan.steps[i];
                        let recovery = plan_executor
                            .handle_step_failure(step, &error, 0)
                            .await
                            .unwrap_or(RecoveryAction::Abort);

                        match recovery {
                            RecoveryAction::Retry => {
                                plan.steps[i].status = StepStatus::Pending;
                                plan.steps[i].result = None;
                            }
                            RecoveryAction::Skip => {
                                plan.steps[i].status = StepStatus::Skipped;

                                self.emit_and_drain(
                                    AgentEvent::PlanStepCompleted {
                                        session_id: session_id.clone(),
                                        step_id: plan.steps[i].id.clone(),
                                        success: false,
                                        result: Some(format!("Skipped: {}", error)),
                                    },
                                    &mut event_rx,
                                    &mut on_event,
                                );
                            }
                            RecoveryAction::Abort => {
                                plan.steps[i].status = StepStatus::Failed;
                                plan.status = PlanStatus::Failed;

                                self.emit_and_drain(
                                    AgentEvent::PlanStepCompleted {
                                        session_id: session_id.clone(),
                                        step_id: plan.steps[i].id.clone(),
                                        success: false,
                                        result: Some(error.clone()),
                                    },
                                    &mut event_rx,
                                    &mut on_event,
                                );

                                self.emit_and_drain(
                                    AgentEvent::PlanCompleted {
                                        session_id: session_id.clone(),
                                        plan_id: plan.id.clone(),
                                        success: false,
                                    },
                                    &mut event_rx,
                                    &mut on_event,
                                );

                                if let Some(store) = &plan_store {
                                    let _ = store.save_plan(&plan, json!({})).await;
                                }

                                return Ok(RunOutcome::Failed {
                                    error: format!("Step '{}' failed: {}", plan.steps[i].id, error),
                                });
                            }
                        }
                    }
                }
                Err(e) => {
                    plan.steps[i].status = StepStatus::Failed;
                    plan.status = PlanStatus::Failed;

                    self.emit_and_drain(
                        AgentEvent::PlanStepCompleted {
                            session_id: session_id.clone(),
                            step_id: plan.steps[i].id.clone(),
                            success: false,
                            result: Some(e.to_string()),
                        },
                        &mut event_rx,
                        &mut on_event,
                    );

                    self.emit_and_drain(
                        AgentEvent::PlanCompleted {
                            session_id: session_id.clone(),
                            plan_id: plan.id.clone(),
                            success: false,
                        },
                        &mut event_rx,
                        &mut on_event,
                    );

                    if let Some(store) = &plan_store {
                        let _ = store.save_plan(&plan, json!({})).await;
                    }

                    return Err(e);
                }
            }
        }

        plan.status = PlanStatus::Completed;

        self.emit_and_drain(
            AgentEvent::PlanCompleted {
                session_id: session_id.clone(),
                plan_id: plan.id.clone(),
                success: true,
            },
            &mut event_rx,
            &mut on_event,
        );

        if let Some(store) = &plan_store {
            let _ = store.save_plan(&plan, json!({})).await;
        }

        Ok(RunOutcome::Completed)
    }

    fn emit_and_drain<F>(
        &self,
        event: AgentEvent,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        self.emit_event(event);
        let _ = Self::drain_async_events(event_rx, on_event);
    }

    async fn execute_plan_step<F>(
        &mut self,
        session_id: &SessionId,
        step: &crate::types::PlanStep,
        plan: &ExecutionPlan,
        plan_executor: &Arc<dyn PlanExecutor>,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<crate::types::StepResult>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let should_continue = plan_executor
            .should_continue(plan, step)
            .await
            .unwrap_or(true);

        if !should_continue {
            return Ok(crate::types::StepResult::success("Skipped", 0));
        }

        let step_input = format!(
            "Execute plan step: {}\nDescription: {}",
            step.id, step.description
        );

        let outcome = self
            .run_turn_with_handler(session_id.clone(), &step_input, |event| {
                on_event(event)
            })
            .await?;

        let _ = Self::drain_async_events(event_rx, on_event);

        match outcome {
            RunOutcome::Completed => Ok(crate::types::StepResult::success("Step completed", 0)),
            RunOutcome::Failed { error } => {
                Ok(crate::types::StepResult::failure(error, 0))
            }
        }
    }
}
