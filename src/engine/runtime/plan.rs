use serde_json::json;
use tokio::sync::broadcast;

use crate::types::{
    AgentError, AgentEvent, AgentResult, ExecutionPlan, PlanStatus, RecoveryAction,
    RunOutcome, SessionId, StepStatus,
};
use crate::engine::plan::{
    AlwaysContinue, AbortOnFailure, PlanGenerator, PlanStore, RecoveryStrategy,
    StepContinuePolicy, StepExecutor,
};
use super::AgentRuntime;
use std::sync::Arc;

impl AgentRuntime {
    /// Run a plan in **agentic** mode: each step becomes an agent turn.
    ///
    /// The agent receives step instructions as user input and decides autonomously
    /// which tools to call. No `StepExecutor` is needed.
    pub async fn run_plan_agentic<F>(
        &mut self,
        session_id: SessionId,
        objective: &str,
        generator: Arc<dyn PlanGenerator>,
        plan_store: Option<Arc<dyn PlanStore>>,
        mut on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let tool_definitions = self.tools.definitions();
        let mut event_rx = self.subscribe_events();

        let mut plan = generator
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

        let result = self
            .run_plan_steps(
                &session_id,
                &mut plan,
                None::<Arc<dyn StepExecutor>>,
                None::<Arc<dyn StepContinuePolicy>>,
                None::<Arc<dyn RecoveryStrategy>>,
                &mut event_rx,
                &mut on_event,
            )
            .await;

        if let Some(store) = &plan_store {
            let _ = store.save_plan(&plan, json!({})).await;
        }

        result
    }

    /// Run a plan in **deterministic** mode: each step is executed directly
    /// through the provided `StepExecutor`.
    ///
    /// Use this when you want the plan to be executed without LLM turn
    /// overhead (e.g. predetermined SSH commands).
    #[allow(clippy::too_many_arguments)]
    pub async fn run_plan_deterministic<F>(
        &mut self,
        session_id: SessionId,
        objective: &str,
        generator: Arc<dyn PlanGenerator>,
        executor: Arc<dyn StepExecutor>,
        policy: Option<Arc<dyn StepContinuePolicy>>,
        recovery: Option<Arc<dyn RecoveryStrategy>>,
        plan_store: Option<Arc<dyn PlanStore>>,
        mut on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let tool_definitions = self.tools.definitions();
        let mut event_rx = self.subscribe_events();

        let mut plan = generator
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

        let result = self
            .run_plan_steps(
                &session_id,
                &mut plan,
                Some(executor),
                policy.or_else(|| Some(Arc::new(AlwaysContinue))),
                recovery.or_else(|| Some(Arc::new(AbortOnFailure))),
                &mut event_rx,
                &mut on_event,
            )
            .await;

        if let Some(store) = &plan_store {
            let _ = store.save_plan(&plan, json!({})).await;
        }

        result
    }

    /// Internal: shared plan-step execution loop.
    ///
    /// - If `executor` is `None` → agentic mode (step becomes agent turn).
    /// - If `executor` is `Some` → deterministic mode (step goes to executor).
    async fn run_plan_steps<F>(
        &mut self,
        session_id: &SessionId,
        plan: &mut ExecutionPlan,
        executor: Option<Arc<dyn StepExecutor>>,
        policy: Option<Arc<dyn StepContinuePolicy>>,
        recovery: Option<Arc<dyn RecoveryStrategy>>,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(AgentEvent) -> AgentResult<()>,
    {
        let mut i = 0usize;
        while i < plan.steps.len() {
            // Check dependencies before running
            if plan.steps[i].status == StepStatus::Pending
                && !self.check_dependencies_met(plan, i)
            {
                plan.steps[i].status = StepStatus::Skipped;
                i += 1;
                continue;
            }

            plan.steps[i].status = StepStatus::Running;

            self.emit_and_drain(
                AgentEvent::PlanStepStarted {
                    session_id: session_id.clone(),
                    step_id: plan.steps[i].id.clone(),
                    step_description: plan.steps[i].description.clone(),
                },
                event_rx,
                on_event,
            );

            // Step execution
            let step = &plan.steps[i];
            let step_result = if let Some(exec) = &executor {
                // Deterministic mode
                let should_continue = if let Some(p) = &policy {
                    p.should_continue(plan, step)
                        .await
                        .unwrap_or(true)
                } else {
                    true
                };

                if !should_continue {
                    Ok(crate::types::StepResult::success("Skipped", 0))
                } else {
                    exec.execute_step(step, &plan.context).await
                }
            } else {
                // Agentic mode: run as a full agent turn
                let step_input = format!(
                    "Execute plan step: {}\nDescription: {}",
                    step.id, step.description
                );

                let outcome = self
                    .run_turn_with_handler(session_id.clone(), &step_input, |event| {
                        on_event(event)
                    })
                    .await;

                let _ = Self::drain_async_events(event_rx, on_event);

                match outcome {
                    Ok(RunOutcome::Completed) => Ok(crate::types::StepResult::success("Step completed", 0)),
                    Ok(RunOutcome::Failed { error }) => Ok(crate::types::StepResult::failure(error, 0)),
                    Err(e) => Err(e),
                }
            };

            match step_result {
                Ok(result) => {
                    let error = result.error.clone().unwrap_or_default();
                    let success = result.success;
                    plan.steps[i].result = Some(result);

                    if success {
                        plan.steps[i].status = StepStatus::Completed;

                        self.emit_and_drain(
                            AgentEvent::PlanStepCompleted {
                                session_id: session_id.clone(),
                                step_id: plan.steps[i].id.clone(),
                                success: true,
                                result: plan.steps[i].result.as_ref().unwrap().output.clone(),
                            },
                            event_rx,
                            on_event,
                        );

                        i += 1; // Move to next step
                    } else {
                        let action: RecoveryAction = if let Some(r) = &recovery {
                            r.handle_step_failure(&plan.steps[i], &error, 0)
                                .await
                                .unwrap_or(RecoveryAction::Abort)
                        } else {
                            RecoveryAction::Abort
                        };

                        match action {
                            RecoveryAction::Retry => {
                                plan.steps[i].status = StepStatus::Pending;
                                plan.steps[i].result = None;
                                // i is NOT incremented, so this step will be retried
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
                                    event_rx,
                                    on_event,
                                );

                                i += 1; // Move to next step
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
                                    event_rx,
                                    on_event,
                                );

                                self.emit_and_drain(
                                    AgentEvent::PlanCompleted {
                                        session_id: session_id.clone(),
                                        plan_id: plan.id.clone(),
                                        success: false,
                                    },
                                    event_rx,
                                    on_event,
                                );

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
                        event_rx,
                        on_event,
                    );

                    self.emit_and_drain(
                        AgentEvent::PlanCompleted {
                            session_id: session_id.clone(),
                            plan_id: plan.id.clone(),
                            success: false,
                        },
                        event_rx,
                        on_event,
                    );

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
            event_rx,
            on_event,
        );

        Ok(RunOutcome::Completed)
    }

    fn check_dependencies_met(&self, plan: &ExecutionPlan, step_index: usize) -> bool {
        let step = &plan.steps[step_index];
        if step.dependencies.is_empty() {
            return true;
        }

        step.dependencies.iter().all(|dep_id: &String| {
            plan.steps
                .iter()
                .find(|s| s.id == *dep_id)
                .map(|s| matches!(s.status, StepStatus::Completed | StepStatus::Skipped))
                .unwrap_or(false)
        })
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
}
