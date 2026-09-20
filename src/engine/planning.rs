//! Planning runs inside the same admitted task, ledger and cancellation boundary.
mod attempt;
use super::{Engine, RunContext};
use crate::{
    assessment::{evaluate_rules, merge_assessment_with_evidence, TaskAssessment, TaskFacts},
    contracts::{id, now_ms, EffectivePlan, EngineError, Result, SubmissionSpec, TaskSpec},
    decision::{DecisionAssessment, DecisionRecommendation, DecisionRequest, DecisionState},
    planning::validate_plan,
    router::RoutingSnapshot,
};
use serde_json::{json, Value};
use std::time::Duration;

struct DecisionEvaluation {
    assessment: DecisionAssessment,
    recommendation: DecisionRecommendation,
}

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
        let decision = self.decision_assessment(submission, task, context).await?;
        plan.assessment = Some(build_assessment(
            submission,
            &self.config,
            &plan,
            decision.as_ref(),
        )?);
        let metrics = self.store.metrics().await?;
        let routing =
            RoutingSnapshot::capture_with_metrics(&self.config, &mut plan, now_ms(), &metrics);
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

    async fn decision_assessment(
        &self,
        submission: &SubmissionSpec,
        task: &TaskSpec,
        context: &RunContext,
    ) -> Result<Option<DecisionEvaluation>> {
        let Some(config) = self.config.decision.as_ref() else {
            return Ok(None);
        };
        let model = self.decision_model.as_ref().ok_or_else(|| {
            EngineError::new(
                "decision",
                "decision configuration requires a decision model",
            )
        })?;
        self.planning_resources(task, context).await?;
        let facts = submission.task_facts.clone().unwrap_or(TaskFacts {
            version: "host-facts-absent-v1".into(),
            values: Default::default(),
        });
        let request = DecisionRequest {
            question_set_version: config.question_set_version.clone(),
            state: DecisionState {
                version: facts.version,
                values: facts.values,
            },
            questions: config.questions.clone(),
        };
        request.validate()?;
        let attempt = id();
        self.store
            .reserve(
                &task.task_id,
                &attempt,
                config.max_cost,
                task.max_calls,
                json!({"kind":"decision","question_set_version":request.question_set_version}),
            )
            .await?;
        let remaining = task
            .deadline_ms
            .saturating_sub(task.finalization_ms)
            .saturating_sub(now_ms());
        let result = tokio::select! {
            biased;
            _ = context.cancel.cancelled() => Err(EngineError::new("cancelled", "decision assessment cancelled")),
            result = tokio::time::timeout(Duration::from_millis(remaining), model.assess(request.clone())) =>
                result.unwrap_or_else(|_| Err(EngineError::new("deadline", "decision assessment timed out"))),
        };
        match result {
            Ok(assessment) => match assessment.validate_for(&request) {
                Ok(()) => match config.policy.apply(&request, &assessment) {
                    Ok(recommendation) => {
                        self.store
                            .settle(
                                &task.task_id,
                                &attempt,
                                assessment.actual_cost,
                                json!({"kind":"decision","assessment":assessment}),
                            )
                            .await?;
                        Ok(Some(DecisionEvaluation {
                            assessment,
                            recommendation,
                        }))
                    }
                    Err(error) => self.fail_decision(&task.task_id, &attempt, error).await,
                },
                Err(error) => self.fail_decision(&task.task_id, &attempt, error).await,
            },
            Err(error) => self.fail_decision(&task.task_id, &attempt, error).await,
        }
    }

    async fn fail_decision(
        &self,
        task: &str,
        attempt: &str,
        error: EngineError,
    ) -> Result<Option<DecisionEvaluation>> {
        self.store
            .settle(
                task,
                attempt,
                None,
                json!({"kind":"decision","error":error}),
            )
            .await?;
        Err(error)
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

fn build_assessment(
    submission: &SubmissionSpec,
    config: &crate::contracts::Config,
    _plan: &crate::contracts::EffectivePlan,
    decision: Option<&DecisionEvaluation>,
) -> Result<TaskAssessment> {
    let facts = submission.task_facts.clone().unwrap_or(TaskFacts {
        version: "host-facts-absent-v1".into(),
        values: Default::default(),
    });
    let rules = config
        .assessment_rules
        .clone()
        .unwrap_or(crate::assessment::RuleSet {
            version: "empty-v1".into(),
            rules: vec![],
        });
    let rules = evaluate_rules(&facts, &rules)?;
    Ok(merge_assessment_with_evidence(
        submission.minimum_execution_class,
        &rules,
        None,
        decision.map(|value| &value.recommendation),
        decision.map(|value| &value.assessment),
    ))
}
