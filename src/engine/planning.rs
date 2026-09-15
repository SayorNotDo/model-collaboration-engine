//! Planning runs inside the same admitted task, ledger and cancellation boundary.
mod attempt;
use super::{Engine, RunContext};
use crate::{
    contracts::{now_ms, EffectivePlan, EngineError, Result, SubmissionSpec, TaskSpec},
    planning::validate_plan,
    router::RoutingSnapshot,
};
use serde_json::{json, Value};

impl Engine {
    pub(super) async fn prepare_submission(
        &self,
        submission: &SubmissionSpec,
        task: &TaskSpec,
        context: &RunContext,
    ) -> Result<(TaskSpec, RoutingSnapshot)> {
        self.planning_resources(task, context).await?;
        let (mut plan, evidence) = if submission.needs_planning() {
            self.plan_submission(submission, task, context).await?
        } else {
            (
                validate_plan(submission, None, None)?,
                json!({"skipped":true}),
            )
        };
        let effective_task = plan.task(submission);
        effective_task
            .validate(&self.config)
            .map_err(|error| EngineError::new("plan_validation", error.message))?;
        let routing = RoutingSnapshot::capture(&self.config, &mut plan, now_ms());
        self.store
            .save_plan(
                &task.task_id,
                json!({"effective_plan":plan,"planning":evidence,"routing_snapshot":routing}),
                json!({"version":1,"phase":"planned"}),
            )
            .await?;
        self.emit(task, context, None, plan.proposal_id.as_deref(), "plan_validated",
            json!({"plan_version":plan.plan_version,"task_type":plan.task_type,"strategy":plan.strategy}),
        ).await?;
        self.planning_resources(task, context).await?;
        self.store
            .checkpoint(&task.task_id, json!({"version":1,"phase":"executing"}))
            .await?;
        Ok((effective_task, routing))
    }

    async fn planning_resources(&self, task: &TaskSpec, context: &RunContext) -> Result<()> {
        if context.cancel.is_cancelled() {
            return Err(EngineError::new("cancelled", "task cancelled"));
        }
        if now_ms() >= task.deadline_ms.saturating_sub(task.finalization_ms) {
            return Err(EngineError::new(
                "deadline",
                "task execution deadline reached",
            ));
        }
        let ledger = self.store.ledger(&task.task_id).await?;
        if ledger.calls >= task.max_calls || (ledger.total > 0 && ledger.available() == 0) {
            return Err(EngineError::new("budget", "no execution resources remain"));
        }
        Ok(())
    }

    async fn plan_submission(
        &self,
        submission: &SubmissionSpec,
        task: &TaskSpec,
        context: &RunContext,
    ) -> Result<(EffectivePlan, Value)> {
        let stage_deadline = task
            .deadline_ms
            .saturating_sub(task.finalization_ms)
            .min(now_ms().saturating_add(submission.planning.timeout_ms));
        self.store
            .checkpoint(&task.task_id, json!({"version":1,"phase":"planning"}))
            .await?;
        self.emit(
            task,
            context,
            Some("planner"),
            None,
            "planning_started",
            json!({"proposal_version":1}),
        )
        .await?;
        // Only inner proposal/provider failures may fall back. Outer accounting failures terminate.
        let (attempt, output) = self
            .planning_attempt(submission, task, context, stage_deadline)
            .await?;
        let mut evidence = json!({"attempt_id":attempt});
        let validated = match output {
            Ok(proposal) => {
                evidence["proposal"] = json!(proposal);
                self.emit(
                    task,
                    context,
                    Some("planner"),
                    attempt.as_deref(),
                    "planning_completed",
                    json!({"proposal_version":1}),
                )
                .await?;
                let proposal_id = attempt.as_deref().expect("proposal requires an attempt");
                validate_plan(submission, Some((proposal_id, &proposal)), None)
            }
            Err(error) => Err(error),
        };
        let validated = validated.and_then(|plan| {
            plan.task(submission)
                .validate(&self.config)
                .map_err(|error| EngineError::new("plan_validation", error.message))?;
            Ok(plan)
        });
        match validated {
            Ok(plan) => Ok((plan, evidence)),
            Err(error) => {
                evidence["error"] = json!(error);
                self.store
                    .save_plan(
                        &task.task_id,
                        json!({"planning":evidence}),
                        json!({"version":1,"phase":"planning_failed"}),
                    )
                    .await?;
                // No fallback after cancellation, overall deadline or exhausted total resources.
                if matches!(
                    error.kind.as_str(),
                    "cancelled" | "deadline" | "budget" | "storage"
                ) {
                    return Err(error);
                }
                self.planning_resources(task, context).await?;
                self.emit(
                    task,
                    context,
                    Some("planner"),
                    attempt.as_deref(),
                    "planning_failed",
                    json!({"kind":error.kind,"reason":error.message}),
                )
                .await?;
                let fallback = submission.planning.fallback.as_ref().ok_or(error)?;
                let plan = validate_plan(submission, None, Some(fallback))?;
                evidence["fallback"] = json!(fallback);
                self.emit(
                    task,
                    context,
                    Some("planner"),
                    attempt.as_deref(),
                    "planning_fallback",
                    json!({"task_type":plan.task_type,"strategy":plan.strategy,"reason":evidence["error"]["kind"]}),
                )
                .await?;
                Ok((plan, evidence))
            }
        }
    }
}
