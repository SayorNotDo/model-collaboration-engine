//! Bounded strategy rounds and artifact evaluation.
use super::{Engine, RunContext};
use crate::{
    contracts::{
        digest, id, Artifact, EngineError, EvaluationStatus, Result, Snapshot, Strategy,
        TaskResult, TaskSpec,
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
        self.emit(
            task,
            context,
            None,
            None,
            "task_started",
            json!({"strategy":task.strategy}),
        )
        .await?;
        let mut excluded = BTreeSet::new();
        let mut health = BTreeMap::<String, Health>::new();
        let mut artifact: Option<Artifact> = None;
        let mut feedback = Vec::new();
        let mut quality_floor = 0.0;
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
            let (output, attempt, model) = self
                .invoke(task, snapshot, &excluded, quality_floor, &health, context)
                .await?;
            let evaluation = strategy::evaluate(&output.text, &task.acceptance);
            health.entry(model.id.clone()).or_default().calls += 1;
            if evaluation.status == EvaluationStatus::Pass {
                health.entry(model.id.clone()).or_default().accepted += 1;
            }
            artifact = Some(Artifact {
                artifact_id: id(),
                version: round,
                attempt_id: attempt,
                checksum: digest(&output.text),
                text: output.text,
            });
            let mut evaluation = evaluation;
            if task.strategy == Strategy::GeneratorCritic
                && evaluation.status == EvaluationStatus::Pass
            {
                evaluation = self
                    .evaluate_with_critic(task, context, &model, artifact.as_ref(), round, &health)
                    .await?;
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
                    quality_floor = model.acceptance;
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
    ) -> Result<crate::contracts::Evaluation> {
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
        let (critic, _, _) = self
            .invoke(task, snapshot, &critic_excluded, 0.0, health, context)
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
        Ok(evaluation)
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
