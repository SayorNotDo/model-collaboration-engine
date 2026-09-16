use model_collaboration_engine::{
    contracts::{Config, EffectivePlan, QualityProfile, RoutingProfiles, TaskSpec, TaskType},
    router::{route_profiled, RouteRequest, RoutingDecision, RoutingSnapshot},
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

pub fn config() -> Config {
    let mut config: Config =
        serde_json::from_str(include_str!("../../examples/config.json")).unwrap();
    config.weights = serde_json::from_value(json!({"quality":1,"capability":0,
        "reliability":0,"cost":0,"latency":0,"uncertainty":0,"version":"quality-only"}))
    .unwrap();
    let mut other = config.models[0].clone();
    other.id = "other".into();
    config.models.push(other);
    config.routing_profiles = Some(
        serde_json::from_value(json!({
            "schema_version":1,"version":"test-static-v1"
        }))
        .unwrap(),
    );
    config
}

pub fn profile(model: &str, kind: TaskType, role: &str, prior: f64) -> QualityProfile {
    QualityProfile {
        model_id: model.into(),
        model_version: "1".into(),
        task_type: kind,
        role: role.into(),
        evaluator_version: "1".into(),
        prior,
        prior_weight: 10.0,
        accepted: 0,
        samples: 0,
    }
}

pub fn profiles(config: &mut Config) -> &mut RoutingProfiles {
    config.routing_profiles.as_mut().unwrap()
}

pub fn plan(kind: TaskType) -> EffectivePlan {
    serde_json::from_value(json!({
        "plan_version":1,"task_type":kind,"strategy":"single",
        "role_profiles":{"invoke":{"task_type":kind,"role":"invoke"}},
        "constraints":model_collaboration_engine::contracts::Constraints::default(),"acceptance":{"version":"1","nonempty":true,
        "json_object":false,"required_substrings":[]},
        "selection":{"version":"host-task-fit-v1","min_quality":0.0,"target_quality":0.9,
        "above_target_factor":0.1,"cost_reference":10000,"latency_reference_ms":5000,
        "min_upgrade_gain":0.05},"submission_hash":"test",
        "proposal_id":null,"validator_version":"1","routing_profile_version":"global-prior-v1"
    }))
    .unwrap()
}

pub fn task() -> TaskSpec {
    let mut task: TaskSpec =
        serde_json::from_str(include_str!("../../examples/task.json")).unwrap();
    task.deadline_ms = 60_000;
    task
}

pub fn snapshot(config: &Config, kind: TaskType) -> RoutingSnapshot {
    config.validate(false).unwrap();
    RoutingSnapshot::capture(config, &mut plan(kind), 10_000)
}

pub fn choose(snapshot: &RoutingSnapshot, task: &TaskSpec, floor: f64) -> RoutingDecision {
    route_profiled(
        snapshot,
        RouteRequest {
            task,
            node: "invoke",
            input_tokens: 100,
            available: 100_000,
            excluded: &BTreeSet::new(),
            quality_floor: floor,
            health: &BTreeMap::new(),
        },
    )
    .unwrap()
}
