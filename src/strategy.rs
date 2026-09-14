//! Strategy is deliberately independent of provider identities and transport.
use crate::contracts::*;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub plan_id: String,
    pub version: u32,
    pub strategy: Strategy,
    pub nodes: Vec<&'static str>,
    pub max_rounds: u32,
}
pub fn compile(task: &TaskSpec) -> Plan {
    let nodes = match task.strategy {
        Strategy::Single => vec!["invoke", "evaluate", "finish"],
        Strategy::Cascade => vec!["invoke", "evaluate", "bounded_upgrade", "finish"],
        Strategy::GeneratorCritic => vec![
            "generator",
            "evaluate",
            "critic",
            "bounded_revision",
            "final_evaluate",
            "finish",
        ],
    };
    Plan {
        plan_id: id(),
        version: 1,
        strategy: task.strategy.clone(),
        nodes,
        max_rounds: task.max_rounds,
    }
}
pub fn evaluate(text: &str, acceptance: &Acceptance) -> Evaluation {
    let mut defects = vec![];
    if acceptance.nonempty && text.trim().is_empty() {
        defects.push("empty output".into());
    }
    for s in &acceptance.required_substrings {
        if !text.contains(s) {
            defects.push(format!("missing required text: {s}"));
        }
    }
    if acceptance.json_object
        && !serde_json::from_str::<serde_json::Value>(text).is_ok_and(|v| v.is_object())
    {
        defects.push("output is not a JSON object".into());
    }
    Evaluation {
        status: if defects.is_empty() {
            EvaluationStatus::Pass
        } else {
            EvaluationStatus::Revise
        },
        checks: vec!["deterministic acceptance checks".into()],
        evidence: vec![],
        action: if defects.is_empty() {
            "accept"
        } else {
            "revise"
        }
        .into(),
        defects,
    }
}
