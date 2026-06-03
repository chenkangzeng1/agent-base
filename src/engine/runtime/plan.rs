use serde_json::json;
use tokio::sync::{broadcast, mpsc};

use crate::types::{
    AgentError, AgentEvent, AgentResult, ExecutionPlan, PlanStatus, RecoveryAction,
    RunOutcome, RuntimeEvent, SessionId, StepStatus, UserEvent,
};
use crate::tool::ToolContext;
use crate::engine::plan::{
    AlwaysContinue, AbortOnFailure, PlanGenerator, PlanStore, RecoveryStrategy,
    StepContinuePolicy, StepExecutor,
};
use crate::engine::runtime::event_bus::EventBus;
use super::AgentRuntime;
use std::sync::Arc;

impl AgentRuntime {
    /// Run a plan in **agentic** mode: each step becomes an agent turn.
    ///
    /// The agent receives step instructions as user input and decides autonomously
    /// which tools to call. No `StepExecutor` is needed.
    pub async fn run_plan_agentic<F>(
        &self,
        session_id: SessionId,
        objective: &str,
        generator: Arc<dyn PlanGenerator>,
        plan_store: Option<Arc<dyn PlanStore>>,
        mut on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        tracing::info!(session_id = session_id.id, %objective, "run plan agentic start");
        let tool_definitions = self.tool_engine.definitions();
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
        &self,
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
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        tracing::info!(session_id = session_id.id, %objective, "run plan deterministic start");
        let tool_definitions = self.tool_engine.definitions();
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

    /// Internal: shared plan-step execution loop (phase-aware).
    ///
    /// Iterates through phases in order. Within each phase, iterates through
    /// steps. Steps whose dependencies are not met are skipped.
    ///
    /// Uses index-based iteration to avoid borrow-checker conflicts when
    /// reading `plan` (for dependency checks) while mutating phases/steps.
    ///
    /// - If `executor` is `None` → agentic mode (step becomes agent turn).
    /// - If `executor` is `Some` → deterministic mode (step goes to executor).
    async fn run_plan_steps<F>(
        &self,
        session_id: &SessionId,
        plan: &mut ExecutionPlan,
        executor: Option<Arc<dyn StepExecutor>>,
        policy: Option<Arc<dyn StepContinuePolicy>>,
        recovery: Option<Arc<dyn RecoveryStrategy>>,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        for phase_idx in 0..plan.phases.len() {
            plan.phases[phase_idx].status = crate::types::PhaseStatus::Running;

            let mut step_idx = 0usize;
            while step_idx < plan.phases[phase_idx].steps.len() {
                let step_id = plan.phases[phase_idx].steps[step_idx].id.clone();

                // Check dependencies (immutable borrow of plan, no conflict with index-based access)
                if plan.phases[phase_idx].steps[step_idx].status == StepStatus::Pending
                    && !Self::check_dependencies_met(plan, &step_id)
                {
                    plan.phases[phase_idx].steps[step_idx].status = StepStatus::Skipped;
                    step_idx += 1;
                    continue;
                }

                plan.phases[phase_idx].steps[step_idx].status = StepStatus::Running;
                let step_desc = plan.phases[phase_idx].steps[step_idx].description.clone();

                tracing::debug!(session_id = session_id.id, %step_id, "step running");

                self.emit_and_drain(
                    AgentEvent::PlanStepStarted {
                        session_id: session_id.clone(),
                        step_id: step_id.clone(),
                        step_description: step_desc.clone(),
                        payload: None,
                    },
                    event_rx,
                    on_event,
                );

                // Step execution — immutable borrow of step, released before mutation below
                let step = &plan.phases[phase_idx].steps[step_idx];
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
                        // Deterministic mode: create a ToolContext for the step executor.
                        // Note: UserEvents emitted by the step's tool are not forwarded to
                        // the external callback here — deterministic mode executes pre-planned
                        // steps without real-time monitoring. If UserEvent forwarding is needed,
                        // consume `user_event_rx` and map to `on_event` in a select loop.
                        let (user_event_tx, _user_event_rx) = mpsc::unbounded_channel::<UserEvent>();
                        let tool_ctx = ToolContext {
                            session_id: session_id.clone(),
                            user_event_tx,
                            llm_client: Some(self.llm_engine().client.clone()),
                            session_store: Some(self.session_manager.session_store().clone()),
                            language: crate::types::Language::En,
                        };
                        exec.execute_step(step, &plan.context, &tool_ctx).await
                    }
                } else {
                    // Agentic mode: run as a full agent turn
                    let mut step_events = Vec::new();
                    let outcome = self
                        .run(session_id.clone(), |event| {
                            step_events.push(event.clone());
                            on_event(event)
                        })
                        .await;

                    let _ = EventBus::drain_async_events(event_rx, on_event);

                    match outcome {
                        Ok(RunOutcome::Completed) => Ok(crate::types::StepResult::success("Step completed", 0)),
                        Ok(RunOutcome::Failed { error }) => Ok(crate::types::StepResult::failure(error, 0)),
                        Ok(RunOutcome::MaxTurnsExceeded { .. }) => Ok(crate::types::StepResult::failure("Max turns exceeded".to_string(), 0)),
                        Ok(RunOutcome::Cancelled) => Ok(crate::types::StepResult::failure("Cancelled".to_string(), 0)),
                        Err(e) => Err(e),
                    }
                };

                // Handle result — mutable access via index
                match step_result {
                    Ok(result) => {
                        let error = result.error.clone().unwrap_or_default();
                        let success = result.success;
                        plan.phases[phase_idx].steps[step_idx].result = Some(result);

                        if success {
                            plan.phases[phase_idx].steps[step_idx].status = StepStatus::Completed;
                            tracing::debug!(session_id = session_id.id, %step_id, "step completed");

                            let output = plan.phases[phase_idx].steps[step_idx]
                                .result.as_ref().unwrap().output.clone();

                            self.emit_and_drain(
                                AgentEvent::PlanStepCompleted {
                                    session_id: session_id.clone(),
                                    step_id: step_id.clone(),
                                    success: true,
                                    result: output,
                                    payload: None,
                                },
                                event_rx,
                                on_event,
                            );

                            step_idx += 1;
                        } else {
                            let action: RecoveryAction = if let Some(r) = &recovery {
                                r.handle_step_failure(&plan.phases[phase_idx].steps[step_idx], &error, 0)
                                    .await
                                    .unwrap_or(RecoveryAction::Abort)
                            } else {
                                RecoveryAction::Abort
                            };

                            match action {
                                RecoveryAction::Retry => {
                                    plan.phases[phase_idx].steps[step_idx].status = StepStatus::Pending;
                                    plan.phases[phase_idx].steps[step_idx].result = None;
                                    tracing::debug!(session_id = session_id.id, %step_id, "step retry");
                                    // step_idx is NOT incremented, so this step will be retried
                                }
                                RecoveryAction::Skip => {
                                    plan.phases[phase_idx].steps[step_idx].status = StepStatus::Skipped;
                                    tracing::debug!(session_id = session_id.id, %step_id, "step skipped");

                                    self.emit_and_drain(
                                        AgentEvent::PlanStepCompleted {
                                            session_id: session_id.clone(),
                                            step_id: step_id.clone(),
                                            success: false,
                                            result: Some(format!("Skipped: {}", error)),
                                            payload: None,
                                        },
                                        event_rx,
                                        on_event,
                                    );

                                    step_idx += 1;
                                }
                                RecoveryAction::Abort => {
                                    plan.phases[phase_idx].steps[step_idx].status = StepStatus::Failed;
                                    plan.phases[phase_idx].status = crate::types::PhaseStatus::Failed;
                                    plan.status = PlanStatus::Failed;
                                    tracing::warn!(session_id = session_id.id, %step_id, "step abort");

                                    self.emit_and_drain(
                                        AgentEvent::PlanStepCompleted {
                                            session_id: session_id.clone(),
                                            step_id: step_id.clone(),
                                            success: false,
                                            result: Some(error.clone()),
                                            payload: None,
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
                                        error: format!("Step '{}' failed: {}", step_id, error),
                                    });
                                }
                            }
                        }
                    }
                    Err(e) => {
                        plan.phases[phase_idx].steps[step_idx].status = StepStatus::Failed;
                        plan.phases[phase_idx].status = crate::types::PhaseStatus::Failed;
                        plan.status = PlanStatus::Failed;

                        self.emit_and_drain(
                            AgentEvent::PlanStepCompleted {
                                session_id: session_id.clone(),
                                step_id: step_id.clone(),
                                success: false,
                                result: Some(e.to_string()),
                                payload: None,
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

            // Phase completed
            plan.phases[phase_idx].status = crate::types::PhaseStatus::Completed;
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

    /// Check if all dependencies of a step (found by id across all phases) are met.
    ///
    /// This is a static method that only performs immutable reads on the plan,
    /// safe to call alongside index-based mutation in `run_plan_steps`.
    fn check_dependencies_met(plan: &ExecutionPlan, step_id: &str) -> bool {
        let step = match plan.find_step(step_id) {
            Some(s) => s,
            None => return true,
        };

        if step.dependencies.is_empty() {
            return true;
        }

        tracing::debug!(%step_id, deps_count = step.dependencies.len(), "checking step dependencies");

        step.dependencies.iter().all(|dep_id: &String| {
            plan.find_step(dep_id)
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
        F: FnMut(RuntimeEvent) -> AgentResult<()>,
    {
        self.emit_event(event);
        let _ = EventBus::drain_async_events(event_rx, on_event);
    }
}
