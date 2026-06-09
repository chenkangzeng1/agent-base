use serde_json::json;
use tokio::sync::{broadcast, mpsc};
use std::sync::Arc;

use crate::types::{
    AgentError, AgentEvent, AgentResult, ExecutionPlan, MessageRole, PlanStatus,
    RecoveryAction, RecoveryContext, RunOutcome, RuntimeEvent, SessionId, StepResult, StepStatus,
};
use crate::tool::ToolContext;
use crate::engine::plan::{
    PlanConfig, PlanGenerator, RecoveryStrategy,
    StepContinuePolicy, StepExecutor,
};
use crate::engine::runtime::event_bus::EventBus;
use super::plan_runner::PlanRunner;

/// Control-flow signal returned by step failure handling.
pub(super) enum StepFailureAction {
    /// Retry the step (loop again without incrementing step_idx).
    Retry,
    /// Skip the step and continue to the next.
    Skip,
    /// Jump to a specific step index (used after Replan).
    JumpTo(usize),
    /// Abort the entire plan.
    Abort,
}

impl PlanRunner {
    /// Execute a pre-built `ExecutionPlan`.
    pub async fn run_plan<F>(
        &self,
        session_id: SessionId,
        plan: ExecutionPlan,
        config: PlanConfig,
        mut on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        tracing::info!(session_id = session_id.id, plan_id = %plan.id, "run_plan start");

        let mut plan = plan;

        // Empty plan check
        if plan.total_steps() == 0 {
            tracing::warn!(plan_id = %plan.id, "plan has no steps, returning completed");
            plan.status = PlanStatus::Completed;
            if let Some(store) = &config.plan_store {
                let _ = store.save_plan(&plan, json!({})).await;
            }
            return Ok(RunOutcome::Completed);
        }

        let mut event_rx = self.event_bus.subscribe();

        self.emit_and_drain(
            AgentEvent::PlanGenerated {
                session_id: session_id.clone(),
                plan: plan.clone(),
            },
            &mut event_rx,
            &mut on_event,
        );

        if let Some(store) = &config.plan_store {
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
                &config,
                &mut event_rx,
                &mut on_event,
            )
            .await;

        if let Some(store) = &config.plan_store {
            let _ = store.save_plan(&plan, json!({})).await;
        }

        result
    }

    /// Generate a plan from an objective using the provided generator, then execute it.
    pub async fn run_plan_with_generator<F>(
        &self,
        session_id: SessionId,
        objective: &str,
        generator: Arc<dyn PlanGenerator>,
        config: PlanConfig,
        mut on_event: F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        tracing::info!(session_id = session_id.id, %objective, "run_plan_with_generator start");

        let tool_definitions = self.tool_engine.definitions();
        let mut event_rx = self.event_bus.subscribe();

        // Create channel for streaming plan generation events
        let (plan_event_tx, mut plan_event_rx) = tokio::sync::mpsc::unbounded_channel();

        // Run generation and event consumption concurrently.
        let generate_fut = generator.generate_plan(objective, "", &tool_definitions, Some(plan_event_tx));

        tokio::pin!(generate_fut);

        let mut plan = loop {
            tokio::select! {
                result = &mut generate_fut => {
                    break result.map_err(|e| AgentError::plan_generation(e.to_string()))?;
                }
                Some(event) = plan_event_rx.recv() => {
                    let _ = on_event(Self::stamp_session_id(event, &session_id, ""));
                }
            }
        };

        // Drain any remaining events after generation completes
        while let Ok(event) = plan_event_rx.try_recv() {
            let _ = on_event(Self::stamp_session_id(event, &session_id, ""));
        }

        // Empty plan check
        if plan.total_steps() == 0 {
            tracing::warn!(plan_id = %plan.id, "generated plan has no steps, returning completed");
            plan.status = PlanStatus::Completed;
            if let Some(store) = &config.plan_store {
                let _ = store.save_plan(&plan, json!({})).await;
            }
            return Ok(RunOutcome::Completed);
        }

        self.emit_and_drain(
            AgentEvent::PlanGenerated {
                session_id: session_id.clone(),
                plan: plan.clone(),
            },
            &mut event_rx,
            &mut on_event,
        );

        if let Some(store) = &config.plan_store {
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
                &config,
                &mut event_rx,
                &mut on_event,
            )
            .await;

        if let Some(store) = &config.plan_store {
            let _ = store.save_plan(&plan, json!({})).await;
        }

        result
    }

    /// Internal: shared plan-step execution loop with progressive adaptive recovery.
    pub async fn run_plan_steps<F>(
        &self,
        session_id: &SessionId,
        plan: &mut ExecutionPlan,
        config: &PlanConfig,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<RunOutcome>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let executor = config.executor().cloned();
        let policy: Option<Arc<dyn StepContinuePolicy>> = Some(config.continue_policy.clone());
        let recovery: Option<Arc<dyn RecoveryStrategy>> = Some(config.recovery.clone());
        let recovery_policy = config.recovery_policy.clone();

        // step_outputs accumulates step outputs, keyed by step_id.
        let mut step_outputs = json!({});
        // Track retry count per root step (keyed by root_step_id).
        let mut retry_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        // Track alternative count per root step (keyed by root_step_id).
        let mut alternative_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        // Track replan count (global for the plan).
        let mut replan_count: usize = 0;
        // Track alternative chain: alternative_step_id -> root_step_id.
        let mut root_step_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();

        for phase_idx in 0..plan.phases.len() {
            plan.phases[phase_idx].status = crate::types::PhaseStatus::Running;

            let mut step_idx = 0usize;
            while step_idx < plan.phases[phase_idx].steps.len() {
                let step_id = plan.phases[phase_idx].steps[step_idx].id.clone();

                // Check dependencies
                if plan.phases[phase_idx].steps[step_idx].status == StepStatus::Pending
                    && !Self::check_dependencies_met(plan, &step_id)
                {
                    plan.phases[phase_idx].steps[step_idx].status = StepStatus::Skipped;
                    if let serde_json::Value::Object(ref mut map) = step_outputs {
                        map.insert(step_id.clone(), json!({"skipped": true}));
                    }
                    step_idx += 1;
                    continue;
                }

                plan.phases[phase_idx].steps[step_idx].status = StepStatus::Running;
                let step_desc = plan.phases[phase_idx].steps[step_idx].description.clone();
                let step_payload = plan.phases[phase_idx].steps[step_idx].payload.clone();

                tracing::debug!(session_id = session_id.id, %step_id, "step running");

                self.emit_and_drain(
                    AgentEvent::PlanStepStarted {
                        session_id: session_id.clone(),
                        step_id: step_id.clone(),
                        step_description: step_desc.clone(),
                        payload: Some(step_payload.clone()),
                    },
                    event_rx,
                    on_event,
                );

                let step = &plan.phases[phase_idx].steps[step_idx];
                let step_result = self
                    .execute_single_step(
                        session_id,
                        step,
                        &executor,
                        &policy,
                        plan,
                        &step_outputs,
                        &mut *on_event,
                        event_rx,
                    )
                    .await;

                // Handle result — mutable access via index
                match step_result {
                    Ok(result) => {
                        let error = result.error.clone().unwrap_or_default();
                        let success = result.success;

                        // Accumulate step output into step_outputs
                        if let serde_json::Value::Object(ref mut map) = step_outputs {
                            if success {
                                let output_val = result.output.as_deref()
                                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                                    .unwrap_or_else(|| json!(result.output));
                                map.insert(step_id.clone(), output_val);
                            } else {
                                map.insert(step_id.clone(), json!({"error": error, "success": false}));
                            }
                        }

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
                                    payload: Some(step_payload),
                                },
                                event_rx,
                                on_event,
                            );

                            step_idx += 1;
                        } else {
                            match self.handle_step_failure_progressive(
                                session_id, plan, phase_idx, step_idx,
                                &step_id, &error, &step_payload,
                                &mut retry_counts, &mut alternative_counts,
                                &mut root_step_map, &mut replan_count,
                                config, &recovery, &recovery_policy,
                                &step_outputs, event_rx, on_event,
                            ).await? {
                                StepFailureAction::Retry => { /* step_idx not incremented */ }
                                StepFailureAction::Skip => { step_idx += 1; }
                                StepFailureAction::JumpTo(idx) => { step_idx = idx; }
                                StepFailureAction::Abort => {
                                    return Ok(RunOutcome::Failed {
                                        error: format!("Step '{}' failed: {}", step_id, error),
                                    });
                                }
                            }
                        }
                    }
                    Err(e) => {
                        match self.handle_step_failure_progressive(
                            session_id, plan, phase_idx, step_idx,
                            &step_id, &e.to_string(), &step_payload,
                            &mut retry_counts, &mut alternative_counts,
                            &mut root_step_map, &mut replan_count,
                            config, &recovery, &recovery_policy,
                            &step_outputs, event_rx, &mut *on_event,
                        ).await? {
                            StepFailureAction::Retry => { /* step_idx not incremented */ }
                            StepFailureAction::Skip => { step_idx += 1; }
                            StepFailureAction::JumpTo(idx) => { step_idx = idx; }
                            StepFailureAction::Abort => {
                                return Ok(RunOutcome::Failed {
                                    error: format!("Step '{}' failed: {}", step_id, e),
                                });
                            }
                        }
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

    /// Progressive step failure handler implementing the 4-level recovery pipeline.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn handle_step_failure_progressive<F>(
        &self,
        session_id: &SessionId,
        plan: &mut ExecutionPlan,
        phase_idx: usize,
        step_idx: usize,
        step_id: &str,
        error: &str,
        step_payload: &serde_json::Value,
        retry_counts: &mut std::collections::HashMap<String, usize>,
        alternative_counts: &mut std::collections::HashMap<String, usize>,
        root_step_map: &mut std::collections::HashMap<String, String>,
        replan_count: &mut usize,
        config: &PlanConfig,
        recovery: &Option<Arc<dyn RecoveryStrategy>>,
        recovery_policy: &Option<Arc<crate::engine::plan::RecoveryPolicy>>,
        step_outputs: &serde_json::Value,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) -> AgentResult<StepFailureAction>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        // Resolve root step ID for quota accounting
        let root_id = root_step_map
            .get(step_id)
            .cloned()
            .unwrap_or_else(|| step_id.to_string());
        let retry = retry_counts.get(&root_id).copied().unwrap_or(0);

        // ── Adaptive recovery path (when configured) ──
        if config.adaptive_recovery.is_some() {
            // Level 0: Framework-level retry with linear backoff
            if retry < config.max_retries {
                retry_counts.insert(root_id.clone(), retry + 1);
                let backoff_ms = 100 * (retry + 1) as u64;
                tracing::debug!(
                    session_id = session_id.id, %step_id,
                    retry = retry + 1, backoff_ms,
                    "Level 0: framework retry with backoff"
                );

                self.emit_and_drain(
                    AgentEvent::StepRetry {
                        session_id: session_id.clone(),
                        step_id: step_id.to_string(),
                        retry_count: retry + 1,
                        backoff_ms,
                    },
                    event_rx,
                    on_event,
                );

                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;

                plan.phases[phase_idx].steps[step_idx].status = StepStatus::Pending;
                plan.phases[phase_idx].steps[step_idx].result = None;
                return Ok(StepFailureAction::Retry);
            }

            // Level 1 & 2: AdaptiveRecoveryStrategy
            let alt = alternative_counts.get(&root_id).copied().unwrap_or(0);
            let strategy = config.adaptive_recovery.as_ref().unwrap();

            let ctx = RecoveryContext {
                session_id: session_id.clone(),
                failed_step: plan.phases[phase_idx].steps[step_idx].clone(),
                root_step_id: root_id.clone(),
                error: error.to_string(),
                retry_count: retry,
                alternative_count: alt,
                replan_count: *replan_count,
                max_retries: config.max_retries,
                max_alternatives: config.max_alternatives,
                max_replans: config.max_replans,
                plan: plan.clone(),
                step_outputs: step_outputs.clone(),
                available_tools: self.tool_engine.definitions(),
            };

            let action = strategy.recover(&ctx).await.unwrap_or(RecoveryAction::Abort);

            match action {
                RecoveryAction::Alternative { step: new_step, root_step_id } => {
                    alternative_counts.insert(root_step_id.clone(), alt + 1);
                    root_step_map.insert(new_step.id.clone(), root_step_id.clone());
                    let inherited_retry = retry_counts.get(&root_step_id).copied().unwrap_or(0);
                    retry_counts.insert(root_step_id.clone(), inherited_retry);

                    let new_step_id = new_step.id.clone();
                    tracing::debug!(
                        session_id = session_id.id,
                        original_step = step_id,
                        alternative_step = %new_step_id,
                        alternative_count = alt + 1,
                        "Level 1: trying alternative step"
                    );

                    self.emit_and_drain(
                        AgentEvent::StepAlternativeTrying {
                            session_id: session_id.clone(),
                            original_step_id: step_id.to_string(),
                            alternative_step_id: new_step_id.clone(),
                            alternative_count: alt + 1,
                        },
                        event_rx,
                        on_event,
                    );

                    let new_start_idx = plan.phases[phase_idx].steps.len();
                    plan.phases[phase_idx].steps.push(new_step);
                    plan.phases[phase_idx].steps[step_idx].status = StepStatus::Failed;

                    for orphan_idx in (step_idx + 1)..new_start_idx {
                        if plan.phases[phase_idx].steps[orphan_idx].status == StepStatus::Pending {
                            plan.phases[phase_idx].steps[orphan_idx].status = StepStatus::Skipped;
                        }
                    }

                    return Ok(StepFailureAction::JumpTo(new_start_idx));
                }
                RecoveryAction::Replan { steps: new_steps, clear_future_phases }
                    if *replan_count < config.max_replans =>
                {
                    *replan_count += 1;
                    tracing::debug!(
                        session_id = session_id.id,
                        plan_id = %plan.id,
                        replan_count = *replan_count,
                        "Level 2: replanning"
                    );

                    self.emit_and_drain(
                        AgentEvent::PlanReplanning {
                            session_id: session_id.clone(),
                            plan_id: plan.id.clone(),
                            replan_count: *replan_count,
                        },
                        event_rx,
                        on_event,
                    );

                    plan.phases[phase_idx].steps[step_idx].status = StepStatus::Failed;
                    let new_start_idx = plan.phases[phase_idx].steps.len();
                    plan.phases[phase_idx].steps.extend(new_steps.clone());

                    if clear_future_phases {
                        for future_phase in plan.phases.iter_mut().skip(phase_idx + 1) {
                            future_phase.steps.retain(|s| {
                                !matches!(s.status, StepStatus::Pending)
                            });
                            if future_phase.steps.is_empty() {
                                future_phase.status = crate::types::PhaseStatus::Skipped;
                            }
                        }
                    }

                    self.emit_and_drain(
                        AgentEvent::PlanReplanned {
                            session_id: session_id.clone(),
                            plan_id: plan.id.clone(),
                            new_steps: new_steps.len(),
                        },
                        event_rx,
                        on_event,
                    );

                    for orphan_idx in (step_idx + 1)..new_start_idx {
                        if plan.phases[phase_idx].steps[orphan_idx].status == StepStatus::Pending {
                            plan.phases[phase_idx].steps[orphan_idx].status = StepStatus::Skipped;
                        }
                    }

                    return Ok(StepFailureAction::JumpTo(new_start_idx));
                }
                RecoveryAction::Skip => {
                    plan.phases[phase_idx].steps[step_idx].status = StepStatus::Skipped;
                    tracing::debug!(session_id = session_id.id, %step_id, "adaptive: skip");
                    self.emit_and_drain(
                        AgentEvent::PlanStepCompleted {
                            session_id: session_id.clone(),
                            step_id: step_id.to_string(),
                            success: false,
                            result: Some(format!("Skipped (adaptive): {}", error)),
                            payload: Some(step_payload.clone()),
                        },
                        event_rx,
                        on_event,
                    );
                    return Ok(StepFailureAction::Skip);
                }
                _ => {}
            }
        }

        // ── Level 3: Fallback to old RecoveryStrategy ──
        let current_retry = retry_counts.get(&root_id).copied().unwrap_or(0);
        let action: RecoveryAction = if let Some(rp) = recovery_policy {
            let err = AgentError::internal(error);
            rp.with_context(&err, current_retry)
        } else if let Some(r) = recovery {
            r.handle_step_failure(
                &plan.phases[phase_idx].steps[step_idx],
                error,
                current_retry,
                plan,
                step_outputs,
            )
            .await
            .unwrap_or(RecoveryAction::Abort)
        } else {
            RecoveryAction::Abort
        };

        match action {
            RecoveryAction::Retry => {
                plan.phases[phase_idx].steps[step_idx].status = StepStatus::Pending;
                plan.phases[phase_idx].steps[step_idx].result = None;
                *retry_counts.entry(root_id.clone()).or_insert(0) += 1;
                tracing::debug!(
                    session_id = session_id.id, %step_id,
                    retry = retry_counts[&root_id],
                    "Level 3: fallback retry"
                );
                Ok(StepFailureAction::Retry)
            }
            RecoveryAction::Skip => {
                plan.phases[phase_idx].steps[step_idx].status = StepStatus::Skipped;
                tracing::debug!(session_id = session_id.id, %step_id, "Level 3: fallback skip");

                self.emit_and_drain(
                    AgentEvent::PlanStepCompleted {
                        session_id: session_id.clone(),
                        step_id: step_id.to_string(),
                        success: false,
                        result: Some(format!("Skipped (fallback): {}", error)),
                        payload: Some(step_payload.clone()),
                    },
                    event_rx,
                    on_event,
                );
                Ok(StepFailureAction::Skip)
            }
            _ => {
                plan.phases[phase_idx].steps[step_idx].status = StepStatus::Failed;
                plan.phases[phase_idx].status = crate::types::PhaseStatus::Failed;
                plan.status = PlanStatus::Failed;
                tracing::warn!(session_id = session_id.id, %step_id, "recovery exhausted, abort");

                self.emit_and_drain(
                    AgentEvent::PlanRecoveryExhausted {
                        session_id: session_id.clone(),
                        step_id: step_id.to_string(),
                        retries: retry_counts.get(&root_id).copied().unwrap_or(0),
                        alternatives: alternative_counts.get(&root_id).copied().unwrap_or(0),
                        replans: *replan_count,
                    },
                    event_rx,
                    on_event,
                );

                self.emit_and_drain(
                    AgentEvent::PlanStepCompleted {
                        session_id: session_id.clone(),
                        step_id: step_id.to_string(),
                        success: false,
                        result: Some(error.to_string()),
                        payload: Some(step_payload.clone()),
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

                Ok(StepFailureAction::Abort)
            }
        }
    }

    /// Execute a single plan step — runtime adaptive.
    pub async fn execute_single_step<F>(
        &self,
        session_id: &SessionId,
        step: &crate::types::PlanStep,
        executor: &Option<Arc<dyn StepExecutor>>,
        policy: &Option<Arc<dyn StepContinuePolicy>>,
        plan: &ExecutionPlan,
        step_outputs: &serde_json::Value,
        on_event: &mut F,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
    ) -> AgentResult<StepResult>
    where
        F: FnMut(RuntimeEvent) -> AgentResult<()> + Send,
    {
        let has_tool_name = step.payload.get("tool_name").and_then(|v| v.as_str()).is_some();

        if let (Some(exec), true) = (executor.as_ref(), has_tool_name) {
            let should_continue = if let Some(p) = policy {
                p.should_continue(plan, step, step_outputs)
                    .await
                    .unwrap_or(true)
            } else {
                true
            };

            if !should_continue {
                return Ok(StepResult::success("Skipped", 0));
            }

            let (user_event_tx, _user_event_rx) = mpsc::unbounded_channel::<crate::types::UserEvent>();
            let tool_ctx = ToolContext {
                session_id: session_id.clone(),
                user_event_tx,
                llm_client: Some(self.llm_engine.client.clone()),
                session_store: Some(self.session_manager.session_store().clone()),
                language: crate::types::Language::En,
            };
            exec.execute_step(step, step_outputs, &tool_ctx).await
        } else {
            self.with_session_mut(session_id, |session| {
                session.push_message(MessageRole::User, &step.description);
            }).await?;

            let mut step_events = Vec::new();
            let outcome = self
                .run(session_id.clone(), |event| {
                    step_events.push(event.clone());
                    on_event(event)
                })
                .await;

            let _ = EventBus::drain_async_events(event_rx, on_event);

            match outcome {
                Ok(RunOutcome::Completed) => Ok(StepResult::success("Step completed", 0)),
                Ok(RunOutcome::Failed { error }) => Ok(StepResult::failure(error, 0)),
                Ok(RunOutcome::MaxTurnsExceeded { .. }) => {
                    Ok(StepResult::failure("Max turns exceeded".to_string(), 0))
                }
                Ok(RunOutcome::Cancelled) => Ok(StepResult::failure("Cancelled".to_string(), 0)),
                Err(e) => Err(e),
            }
        }
    }

    /// Check if all dependencies of a step are met.
    pub(super) fn check_dependencies_met(plan: &ExecutionPlan, step_id: &str) -> bool {
        let step = match plan.find_step(step_id) {
            Some(s) => s,
            None => return true,
        };

        if step.dependencies.is_empty() {
            return true;
        }

        step.dependencies.iter().all(|dep_id: &String| {
            plan.find_step(dep_id)
                .map(|s| matches!(s.status, StepStatus::Completed | StepStatus::Skipped))
                .unwrap_or(false)
        })
    }

    pub(super) fn emit_and_drain<F>(
        &self,
        event: AgentEvent,
        event_rx: &mut broadcast::Receiver<AgentEvent>,
        on_event: &mut F,
    ) where
        F: FnMut(RuntimeEvent) -> AgentResult<()>,
    {
        self.event_bus.emit(event);
        let _ = EventBus::drain_async_events(event_rx, on_event);
    }

    /// Stamp real session_id and plan_id onto plan generation events.
    pub(super) fn stamp_session_id(event: RuntimeEvent, session_id: &SessionId, plan_id: &str) -> RuntimeEvent {
        match event {
            RuntimeEvent::PlanGenerating { .. } => RuntimeEvent::PlanGenerating {
                session_id: session_id.clone(),
                plan_id: plan_id.to_string(),
            },
            RuntimeEvent::PlanStepParsed { step_index, step_id, step_description, .. } => {
                RuntimeEvent::PlanStepParsed {
                    session_id: session_id.clone(),
                    plan_id: plan_id.to_string(),
                    step_index,
                    step_id,
                    step_description,
                }
            }
            RuntimeEvent::ThoughtDelta { text, .. } => RuntimeEvent::ThoughtDelta {
                session_id: session_id.clone(),
                text,
            },
            other => other,
        }
    }
}
