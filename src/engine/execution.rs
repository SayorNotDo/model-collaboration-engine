//! Bounded strategy rounds and artifact evaluation.
use super::{invocation::UpgradeRequirement, Engine, RunContext};
use crate::{
    contracts::{
        digest, id, Artifact, CriticVerdict, EngineError, EvaluationRecord, EvaluationStatus,
        Result, Snapshot, Strategy, TaskResult, TaskSpec,
    },
    router::Health,
    strategy,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
impl Engine {
    pub(super) async fn execute(
        &self,
        task: &TaskSpec,
        context: &RunContext,
    ) -> Result<TaskResult> {
        let mut excluded = BTreeSet::new();
        let health = BTreeMap::<String, Health>::new();
        let mut artifact: Option<Artifact> = None;
        let mut feedback = Vec::new();
        let mut quality_floor = task.selection.min_quality;
        let mut upgrade = None;
        for round in 1..=task.max_rounds {
            let node = if task.strategy == Strategy::GeneratorCritic {
                "generator"
            } else {
                "invoke"
            };
            let snapshot = Snapshot::new(
                task,
                node,
                artifact.as_ref().map(|a| a.text.clone()),
                feedback.clone(),
                round,
            );
            let invocation = self
                .invoke(
                    task,
                    snapshot,
                    &excluded,
                    quality_floor,
                    upgrade,
                    &health,
                    context,
                )
                .await;
            let (output, attempt, model, quality) = match invocation {
                Ok(value) => value,
                Err(error)
                    if task.strategy == Strategy::Cascade
                        && artifact.is_some()
                        && error.kind == "routing"
                        && error.details["category"] == "no_candidate" =>
                {
                    let reason = upgrade_stop_reason(&error);
                    self.store
                        .checkpoint(
                            &task.task_id,
                            json!({"version":1,"round":round,"artifact":artifact,
                                "upgrade_stop":{"reason":reason,
                                    "required_quality":quality_floor,
                                    "requirement":upgrade,
                                    "excluded":error.details["excluded"]}}),
                        )
                        .await?;
                    return self.result(task, "human_required", artifact.unwrap()).await;
                }
                Err(error) => return Err(error),
            };
            let evaluation = strategy::evaluate(&output.text, &task.acceptance);
            artifact = Some(Artifact {
                artifact_id: id(),
                version: round,
                attempt_id: attempt,
                checksum: digest(&output.text),
                text: output.text,
            });
            let mut record = EvaluationRecord {
                task_id: task.task_id.clone(),
                artifact: artifact.as_ref().unwrap().clone(),
                deterministic: evaluation.clone(),
                critic: None,
            };
            self.store.record_evaluation(&record).await?;
            let mut evaluation = evaluation;
            if task.strategy == Strategy::GeneratorCritic
                && evaluation.status == EvaluationStatus::Pass
            {
                let critic = self
                    .evaluate_with_critic(task, context, &model, artifact.as_ref(), round, &health)
                    .await?;
                evaluation = critic.evaluation.clone();
                record.critic = Some(critic);
                self.store.record_evaluation(&record).await?;
            }
            self.record_evaluation(task, context, node, round, &artifact, &evaluation)
                .await?;
            if evaluation.status == EvaluationStatus::Pass {
                return self.result(task, "completed", artifact.unwrap()).await;
            }
            if matches!(
                evaluation.status,
                EvaluationStatus::HumanRequired
                    | EvaluationStatus::MissingEvidence
                    | EvaluationStatus::Unacceptable
            ) {
                return self.result(task, "human_required", artifact.unwrap()).await;
            }
            feedback = evaluation.defects;
            match task.strategy {
                Strategy::Single => break,
                Strategy::Cascade => {
                    excluded.insert(model.id);
                    quality_floor = quality + task.selection.min_upgrade_gain;
                    upgrade = Some(UpgradeRequirement {
                        previous_quality: quality,
                        required_quality: quality_floor,
                    });
                }
                Strategy::GeneratorCritic => {}
            }
        }
        self.result(
            task,
            "human_required",
            artifact.expect("validated nonzero rounds"),
        )
        .await
    }

    async fn result(
        &self,
        task: &TaskSpec,
        status: &str,
        artifact: Artifact,
    ) -> Result<TaskResult> {
        let ledger = self.store.ledger(&task.task_id).await?;
        Ok(TaskResult {
            task_id: task.task_id.clone(),
            status: status.into(),
            artifact,
            settled_cost: ledger.settled,
            reserved_cost: ledger.reserved,
        })
    }
    async fn evaluate_with_critic(
        &self,
        task: &TaskSpec,
        context: &RunContext,
        model: &crate::contracts::Model,
        artifact: Option<&Artifact>,
        round: u32,
        health: &BTreeMap<String, Health>,
    ) -> Result<CriticVerdict> {
        let critic_excluded = if task.constraints.different_critic {
            BTreeSet::from([model.id.clone()])
        } else {
            BTreeSet::new()
        };
        let snapshot = Snapshot::new(
            task,
            "critic",
            artifact.map(|a| a.text.clone()),
            vec![],
            round,
        );
        let (critic, attempt_id, _, _) = self
            .invoke(task, snapshot, &critic_excluded, 0.0, None, health, context)
            .await?;
        let evaluation: crate::contracts::Evaluation =
            serde_json::from_str(&critic.text).map_err(|_| {
                EngineError::new("protocol", "critic returned an invalid Evaluation object")
            })?;
        if evaluation.status == EvaluationStatus::Pass && !evaluation.defects.is_empty() {
            return Err(EngineError::new(
                "protocol",
                "critic passed an artifact with unresolved defects",
            ));
        }
        Ok(CriticVerdict {
            attempt_id,
            evaluation,
        })
    }
    async fn record_evaluation(
        &self,
        task: &TaskSpec,
        context: &RunContext,
        node: &str,
        round: u32,
        artifact: &Option<Artifact>,
        evaluation: &crate::contracts::Evaluation,
    ) -> Result<()> {
        self.store
            .checkpoint(
                &task.task_id,
                json!({"version":1,"round":round,"artifact":artifact,"evaluation":evaluation}),
            )
            .await?;
        self.emit(
            task,
            context,
            Some(node),
            None,
            "evaluation",
            json!({
                "round": round,
                "status": evaluation.status,
                "defect_count": evaluation.defects.len(),
            }),
        )
        .await?;
        Ok(())
    }
}

fn upgrade_stop_reason(error: &EngineError) -> &'static str {
    let candidates = error.details["excluded"]
        .as_object()
        .into_iter()
        .flat_map(|models| models.values())
        .filter_map(serde_json::Value::as_array)
        .map(|reasons| {
            reasons
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
        })
        .filter(|reasons| !reasons.contains(&"attempt_history_or_diversity"))
        .collect::<Vec<_>>();
    if candidates.is_empty()
        || candidates
            .iter()
            .all(|reasons| reasons.contains(&"quality_floor"))
    {
        "no_quality_improvement"
    } else if candidates.iter().any(|reasons| {
        reasons.contains(&"budget") && reasons.iter().all(|reason| *reason == "budget")
    }) {
        "budget"
    } else {
        "constraints"
    }
}
