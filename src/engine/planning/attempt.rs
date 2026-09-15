//! One planning attempt, with a stricter phase cap and protected settlement.
use super::super::{Engine, RunContext};
use crate::{
    contracts::{
        id, now_ms, EngineError, PlannerProposal, Result, Snapshot, SubmissionSpec, TaskSpec,
    },
    planning::messages,
    router::{self, RouteRequest},
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

impl Engine {
    pub(super) async fn planning_attempt(
        &self,
        submission: &SubmissionSpec,
        task: &TaskSpec,
        context: &RunContext,
        stage_deadline: u64,
    ) -> Result<(Option<String>, Result<PlannerProposal>)> {
        let mut phase = task.clone();
        phase.tools.clear();
        // Execution capabilities do not describe the planner. Data restrictions remain intact.
        phase.constraints.required_capabilities = BTreeSet::from(["text".into(), "json".into()]);
        phase.constraints.preferred_capabilities.clear();
        phase.acceptance.json_object = true;
        phase.max_call_cost = phase.max_call_cost.min(submission.planning.max_cost);
        phase.deadline_ms = stage_deadline;
        phase.finalization_ms = 0;
        let messages = messages(submission);
        let bytes = serde_json::to_vec(&json!({"messages":messages,"tools":[]}))
            .expect("serializable messages")
            .len();
        if bytes > self.config.context_max_bytes {
            return Ok((
                None,
                Err(EngineError::new(
                    "planning",
                    "planning context exceeds byte limit",
                )),
            ));
        }
        let excluded = self
            .config
            .models
            .iter()
            .filter(|model| !self.config.planner_models.contains(&model.id))
            .map(|model| model.id.clone())
            .collect();
        let ledger = self.store.ledger(&task.task_id).await?;
        let decision = match router::route(
            &self.config,
            RouteRequest {
                task: &phase,
                node: "planner",
                input_tokens: bytes as u64 + 256,
                available: ledger.available().min(submission.planning.max_cost),
                excluded: &excluded,
                quality_floor: 0.0,
                health: &BTreeMap::new(),
            },
        ) {
            Ok(decision) => decision,
            Err(error) => {
                return Ok((
                    None,
                    Err(
                        EngineError::new("planning", "no permitted planning candidate")
                            .details(error.details),
                    ),
                ))
            }
        };
        let model = self
            .config
            .models
            .iter()
            .find(|m| m.id == decision.model_id)
            .expect("router returns a configured model");
        let attempt = id();
        self.emit(
            task,
            context,
            Some("planner"),
            Some(&attempt),
            "model_started",
            json!({"model_id":model.id,"estimated_cost":decision.estimated_cost}),
        )
        .await?;
        self.store
            .reserve(
                &task.task_id,
                &attempt,
                decision.estimated_cost,
                task.max_calls,
                json!({"role":"planner","route":decision,"proposal_version":1}),
            )
            .await?;
        let snapshot = Snapshot::new(&phase, "planner", None, vec![], 1);
        let output = self
            .dispatch_model(&phase, context, &snapshot, model, &attempt, &messages, &[])
            .await?;
        let proposal = match output {
            Ok(output)
                if !output.complete
                    || !output.tool_calls.is_empty()
                    || output.text.len() > self.config.context_max_bytes =>
            {
                Err(EngineError::new(
                    "plan_validation",
                    "planner output incomplete, oversized or requests tools",
                ))
            }
            Ok(output) => serde_json::from_str(&output.text).map_err(|_| {
                EngineError::new("plan_validation", "planner returned invalid proposal JSON")
            }),
            Err(error)
                if error.kind == "deadline"
                    && now_ms() < task.deadline_ms.saturating_sub(task.finalization_ms) =>
            {
                Err(EngineError::new("planning", "planning stage timed out"))
            }
            Err(error) => Err(error),
        };
        Ok((Some(attempt), proposal))
    }
}
