use model_collaboration_engine::contracts::{Config, SelectionPolicy, SubmissionSpec};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[path = "routing/support.rs"]
#[allow(dead_code)]
mod support;

fn policy() -> Value {
    json!({
        "version":"host-task-fit-v1",
        "min_quality":0.75,
        "target_quality":0.90,
        "above_target_factor":0.10,
        "cost_reference":10000,
        "latency_reference_ms":5000,
        "min_upgrade_gain":0.05
    })
}

#[test]
fn selection_policy_accepts_valid_bounds() {
    let valid: SelectionPolicy = serde_json::from_value(policy()).unwrap();
    valid.validate().unwrap();
}

#[test]
fn selection_policy_rejects_invalid_bounds_and_unknown_fields() {
    for (field, value) in [
        ("min_quality", json!(0.91)),
        ("target_quality", json!(1.01)),
        ("above_target_factor", json!(-0.01)),
        ("cost_reference", json!(0)),
        ("latency_reference_ms", json!(0)),
        ("min_upgrade_gain", json!(0.0)),
    ] {
        let mut candidate = policy();
        candidate[field] = value;
        let parsed: SelectionPolicy = serde_json::from_value(candidate).unwrap();
        assert!(parsed.validate().is_err(), "{field}");
    }
    let mut unknown = policy();
    unknown["extra"] = json!(true);
    assert!(serde_json::from_value::<SelectionPolicy>(unknown).is_err());
}

#[test]
fn selection_policy_uses_utf8_byte_limit_for_version() {
    let mut candidate = policy();
    candidate["version"] = json!("模".repeat(86));
    let parsed: SelectionPolicy = serde_json::from_value(candidate).unwrap();
    assert!(parsed.validate().is_err());
}

#[test]
fn submission_schema_two_requires_and_preserves_selection_policy() {
    let config: Config = serde_json::from_str(include_str!("../examples/config.json")).unwrap();
    let mut task: Value = serde_json::from_str(include_str!("../examples/task.json")).unwrap();
    task["schema_version"] = json!(2);
    task["selection"] = policy();
    task["deadline_ms"] = json!(model_collaboration_engine::contracts::now_ms() + 30_000);
    let submission: SubmissionSpec = serde_json::from_value(task.clone()).unwrap();
    submission.validate(&config).unwrap();
    assert_eq!(
        submission.selection.as_ref().unwrap().version,
        "host-task-fit-v1"
    );

    task.as_object_mut().unwrap().remove("selection");
    let missing: SubmissionSpec = serde_json::from_value(task).unwrap();
    assert!(missing.validate(&config).is_err());
}

#[test]
fn snapshot_schema_four_freezes_selection_and_algorithm() {
    let snapshot = support::snapshot(
        &support::config(),
        model_collaboration_engine::contracts::TaskType::Reasoning,
    );
    assert_eq!(snapshot.schema_version, 4);
    assert_eq!(
        snapshot.selection.as_ref().unwrap().version,
        "host-task-fit-v1"
    );
    assert_eq!(snapshot.algorithm_version.as_deref(), Some("task-fit-v1"));
}

#[test]
fn schema_four_requires_task_fit_inputs_while_schema_three_remains_routable() {
    let snapshot = support::snapshot(
        &support::config(),
        model_collaboration_engine::contracts::TaskType::Reasoning,
    );
    let mut missing = snapshot.clone();
    missing.selection = None;
    let result = model_collaboration_engine::router::route_profiled(
        &missing,
        model_collaboration_engine::router::RouteRequest {
            task: &support::task(),
            node: "invoke",
            input_tokens: 100,
            available: 100_000,
            excluded: &std::collections::BTreeSet::new(),
            quality_floor: 0.0,
            health: &std::collections::BTreeMap::new(),
        },
    );
    assert_eq!(result.unwrap_err().kind, "routing");

    let mut historical = snapshot;
    historical.schema_version = 3;
    historical.selection = None;
    historical.algorithm_version = None;
    assert_eq!(
        support::choose(&historical, &support::task(), 0.0).model_id,
        "local"
    );
}

#[test]
fn schema_three_ignores_task_fit_fields_and_uses_legacy_scoring() {
    let mut config = support::config();
    config.models.truncate(1);
    config.weights = serde_json::from_value(json!({"quality":1.0,"capability":0.0,
        "reliability":0.0,"cost":1.0,"latency":0.0,"uncertainty":0.0,
        "version":"legacy-check"}))
    .unwrap();
    let mut snapshot = support::snapshot(
        &config,
        model_collaboration_engine::contracts::TaskType::Reasoning,
    );
    snapshot.schema_version = 3;
    let task = support::task();
    let decision = support::choose(&snapshot, &task, 0.0);
    let expected_cost =
        100 * config.models[0].input_price + task.output_tokens * config.models[0].output_price;
    let expected = 0.8 - expected_cost as f64 / task.max_call_cost as f64;
    assert!((decision.score - expected).abs() < 1e-12);
}

#[test]
fn critic_keeps_role_scoring_without_generation_selection_floor() {
    use model_collaboration_engine::{
        contracts::{RoleProfile, Strategy, TaskType},
        router::{route_profiled, RouteRequest, RoutingSnapshot},
    };

    let config = support::config();
    let mut plan = support::plan(TaskType::Reasoning);
    plan.strategy = Strategy::GeneratorCritic;
    plan.selection.as_mut().unwrap().min_quality = 0.95;
    plan.role_profiles.insert(
        "critic".into(),
        RoleProfile {
            task_type: TaskType::Reasoning,
            role: "critic".into(),
        },
    );
    let snapshot = RoutingSnapshot::capture(&config, &mut plan, 10_000);
    let decision = route_profiled(
        &snapshot,
        RouteRequest {
            task: &support::task(),
            node: "critic",
            input_tokens: 100,
            available: 100_000,
            excluded: &BTreeSet::new(),
            quality_floor: 0.0,
            health: &BTreeMap::new(),
        },
    )
    .unwrap();
    assert_eq!(decision.model_id, "local");
    assert_eq!(decision.score, 0.8);
}

fn choose_case(
    economic_quality: f64,
    strong_quality: f64,
    budget: u64,
) -> model_collaboration_engine::router::RoutingDecision {
    let mut config = support::config();
    config.models[0].id = "economic".into();
    config.models[0].acceptance = economic_quality;
    config.models[0].input_price = 10;
    config.models[0].output_price = 0;
    config.models[1].id = "strong".into();
    config.models[1].acceptance = strong_quality;
    config.models[1].input_price = 40;
    config.models[1].output_price = 0;
    config.weights = serde_json::from_value(json!({"quality":1.0,"capability":0.0,
        "reliability":0.0,"cost":0.1,"latency":0.0,"uncertainty":0.0,
        "version":"task-fit-test"}))
    .unwrap();
    let snapshot = support::snapshot(
        &config,
        model_collaboration_engine::contracts::TaskType::Reasoning,
    );
    let mut task = support::task();
    task.budget = budget;
    task.max_call_cost = budget;
    support::choose(&snapshot, &task, 0.0)
}

#[test]
fn task_fit_value_selects_economic_or_strong_by_expected_gain() {
    let simple = choose_case(0.90, 0.95, 10_000);
    assert_eq!(simple.model_id, "economic");
    assert!((simple.score - 0.890).abs() < 1e-12);

    let difficult = choose_case(0.76, 0.95, 10_000);
    assert_eq!(difficult.model_id, "strong");
    assert!((difficult.score - 0.865).abs() < 1e-12);
}

#[test]
fn task_fit_value_is_independent_of_budget_when_eligibility_is_unchanged() {
    assert_eq!(
        choose_case(0.90, 0.95, 10_000).model_id,
        choose_case(0.90, 0.95, 100_000).model_id
    );
}
