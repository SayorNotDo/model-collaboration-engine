#[path = "routing/support.rs"]
mod routing;
#[path = "planning/support.rs"]
mod support;
use model_collaboration_engine::{
    assessment::ExecutionClass,
    contracts::*,
    router::{
        self, route_profiled_with_minimum_class, FallbackTier, RouteRequest, RoutingSnapshot,
    },
};
use routing::{choose, config, plan, profile, profiles, snapshot, task};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use support::{ledger, proposal, response, saved, setup, submission, Action};
use tokio_util::sync::CancellationToken;

#[test]
fn task_type_changes_ranking_without_changing_capabilities() {
    let mut c = config();
    profiles(&mut c).profiles = vec![
        profile("local", TaskType::Writing, "invoke", 0.95),
        profile("other", TaskType::Writing, "invoke", 0.3),
        profile("local", TaskType::CodeGeneration, "invoke", 0.2),
        profile("other", TaskType::CodeGeneration, "invoke", 0.9),
    ];
    assert_eq!(
        choose(&snapshot(&c, TaskType::Writing), &task(), 0.0).model_id,
        "local"
    );
    assert_eq!(
        choose(&snapshot(&c, TaskType::CodeGeneration), &task(), 0.0).model_id,
        "other"
    );
}

#[test]
fn fallback_walks_parents_general_then_global_without_inventing_zero() {
    let mut c = config();
    profiles(&mut c)
        .parents
        .insert(TaskType::CodeGeneration, TaskType::Reasoning);
    profiles(&mut c).profiles = vec![
        profile("local", TaskType::CodeGeneration, "invoke", 0.4),
        profile("local", TaskType::Reasoning, "invoke", 0.5),
        profile("local", TaskType::General, "invoke", 0.6),
    ];
    for (tier, quality) in [
        (FallbackTier::Exact, 0.4),
        (FallbackTier::Parent, 0.5),
        (FallbackTier::General, 0.6),
        (FallbackTier::GlobalPrior, 0.8),
    ] {
        let s = snapshot(&c, TaskType::CodeGeneration);
        let q = &s.nodes["invoke"].qualities["local"];
        assert_eq!(q.fallback, tier);
        assert!((q.quality - quality).abs() < 1e-12);
        if !profiles(&mut c).profiles.is_empty() {
            profiles(&mut c).profiles.remove(0);
        }
    }
}

#[test]
fn version_and_role_are_part_of_the_quality_key() {
    let mut c = config();
    let exact = profile("local", TaskType::Writing, "invoke", 0.95);
    for field in ["model_version", "evaluator_version", "role"] {
        let mut wrong = json!(exact);
        wrong[field] = json!(if field == "role" { "critic" } else { "old" });
        profiles(&mut c).profiles = vec![serde_json::from_value(wrong).unwrap()];
        let s = snapshot(&c, TaskType::Writing);
        assert_eq!(
            s.nodes["invoke"].qualities["local"].fallback,
            FallbackTier::GlobalPrior
        );
    }
    profiles(&mut c).profiles.push(exact);
    let s = snapshot(&c, TaskType::Writing);
    assert_eq!(
        s.nodes["invoke"].qualities["local"].fallback,
        FallbackTier::Exact
    );
}

#[test]
fn posterior_and_cascade_floor_use_identical_quality() {
    let mut c = config();
    let mut p = profile("local", TaskType::Writing, "invoke", 0.9);
    p.samples = 10;
    p.accepted = 3; // (9 + 3) / (10 + 10) = 0.6, not global 0.8 or prior 0.9.
    profiles(&mut c).profiles = vec![p, profile("other", TaskType::Writing, "invoke", 0.5)];
    let s = snapshot(&c, TaskType::Writing);
    let d = choose(&s, &task(), 0.6);
    assert_eq!(d.model_id, "local");
    assert!((d.breakdown["quality"] - 0.6).abs() < 1e-12);
    assert_eq!(d.excluded["other"], vec!["quality_floor"]);
    let mut cold = profile("local", TaskType::Writing, "invoke", 0.5);
    cold.prior_weight = f64::from_bits(1);
    assert_eq!(cold.quality(), 0.5);
}

#[test]
fn typed_weights_and_parent_weights_change_ranking() {
    let mut c = config();
    c.models[0].acceptance = 0.99;
    c.models[0].input_price = 10;
    c.models[1].acceptance = 0.4;
    let mut cheap = c.weights.clone();
    cheap.quality = 0.0;
    cheap.cost = 1.0;
    cheap.version = "cheap-v1".into();
    profiles(&mut c).weights.insert(TaskType::Writing, cheap);
    profiles(&mut c)
        .parents
        .insert(TaskType::CodeGeneration, TaskType::Writing);
    for kind in [TaskType::Writing, TaskType::CodeGeneration] {
        let d = choose(&snapshot(&c, kind), &task(), 0.0);
        assert_eq!(d.model_id, "other");
        assert_eq!(d.weights_version, "cheap-v1");
    }
    assert_eq!(
        choose(&snapshot(&c, TaskType::Reasoning), &task(), 0.0).model_id,
        "local"
    );
}

#[test]
fn frozen_snapshot_replays_after_config_change_and_ignores_local_accept_counts() {
    let mut c = config();
    profiles(&mut c)
        .profiles
        .push(profile("local", TaskType::Writing, "invoke", 0.95));
    let s = snapshot(&c, TaskType::Writing);
    let first = choose(&s, &task(), 0.0);
    profiles(&mut c).profiles[0].prior = 0.1;
    c.models[0].input_price = 1000;
    c.weights.quality = 0.0;
    let restored: RoutingSnapshot = serde_json::from_value(json!(s)).unwrap();
    let again = choose(&restored, &task(), 0.0);
    assert_eq!(first.score, again.score);
    assert_eq!(first.model_id, again.model_id);
    assert_eq!(first.routing_snapshot, again.routing_snapshot);
    let health = BTreeMap::from([(
        "local".into(),
        router::Health {
            calls: 100,
            accepted: 0,
            failures: 0,
            unavailable_until: 0,
        },
    )]);
    let d = router::route_profiled(
        &s,
        RouteRequest {
            task: &task(),
            node: "invoke",
            input_tokens: 100,
            available: 100_000,
            excluded: &BTreeSet::new(),
            quality_floor: 0.0,
            health: &health,
        },
    )
    .unwrap();
    assert_eq!(d.breakdown["quality"], first.breakdown["quality"]);
}

#[test]
fn hard_constraints_still_exclude_the_highest_quality_model() {
    let mut c = config();
    c.models[0].acceptance = 1.0;
    c.models[0].local = false;
    let d = choose(&snapshot(&c, TaskType::Writing), &task(), 0.0);
    assert_eq!(d.model_id, "other");
    assert!(d.excluded["local"].contains(&"local_only".into()));
}

#[test]
fn minimum_execution_class_filters_lower_class_candidates() {
    let mut c = config();
    c.models[0].execution_class = ExecutionClass::Medium;
    c.models[1].execution_class = ExecutionClass::Simple;
    let s = snapshot(&c, TaskType::Writing);
    let task = task();
    let decision = route_profiled_with_minimum_class(
        &s,
        RouteRequest {
            task: &task,
            node: "invoke",
            input_tokens: 100,
            available: 100_000,
            excluded: &BTreeSet::new(),
            quality_floor: 0.0,
            health: &BTreeMap::new(),
        },
        ExecutionClass::Medium,
    )
    .unwrap();
    assert_eq!(decision.model_id, "local");
    assert_eq!(decision.excluded["other"], vec!["execution_class"]);
}

#[test]
fn invalid_profiles_fail_configuration_before_admission() {
    let base = json!(config());
    let mutations = [
        json!({"schema_version":2}),
        json!({"version":" "}),
        json!({"parents":{"general":"writing"}}),
        json!({"parents":{"writing":"reasoning","reasoning":"writing"}}),
        json!({"role_mappings":[{"task_type":"writing","node":"planner","profile":{"task_type":"general","role":"invoke"}}]}),
    ];
    for mutation in mutations {
        let mut value = base.clone();
        for (key, entry) in mutation.as_object().unwrap() {
            value["routing_profiles"][key] = entry.clone();
        }
        assert!(serde_json::from_value::<Config>(value)
            .unwrap()
            .validate(false)
            .is_err());
    }
    for (field, value) in [
        ("model_id", json!("missing")),
        ("prior", json!(1.1)),
        ("prior_weight", json!(0)),
        ("accepted", json!(1)),
        ("samples", json!(u64::MAX)),
    ] {
        let mut p = json!(profile("local", TaskType::Writing, "invoke", 0.5));
        p[field] = value;
        let mut value = base.clone();
        value["routing_profiles"]["profiles"] = json!([p]);
        assert!(serde_json::from_value::<Config>(value)
            .unwrap()
            .validate(false)
            .is_err());
    }
    let mut c = config();
    let p = profile("local", TaskType::Writing, "invoke", 0.5);
    profiles(&mut c).profiles = vec![p.clone(), p];
    assert!(c.validate(false).is_err());
    profiles(&mut c).profiles.clear();
    let mut w = c.weights.clone();
    w.quality = f64::MAX;
    w.cost = f64::MAX;
    profiles(&mut c).weights.insert(TaskType::Writing, w);
    assert!(c.validate(false).is_err());
}

fn typed_config(c: &mut Config) {
    let typed = config();
    c.models = typed.models;
    c.weights = typed.weights;
    c.routing_profiles = typed.routing_profiles;
}

#[tokio::test]
async fn cascade_uses_frozen_quality_and_persists_replay_inputs() {
    let f = setup(
        vec![
            response("{}", true),
            response(r#"{"answer":"hello"}"#, true),
        ],
        |c| {
            typed_config(c);
            c.models[0].acceptance = 0.99;
            c.models[1].acceptance = 0.1;
            c.models[0].input_price = 0;
            c.models[0].output_price = 0;
            c.weights.cost = 100.0;
            profiles(c).profiles = vec![
                profile("local", TaskType::Writing, "invoke", 0.6),
                profile("other", TaskType::Writing, "invoke", 0.9),
            ];
        },
    )
    .await;
    let mut sub = submission();
    sub.task_type = Some(TaskType::Writing);
    sub.strategy = Some(Strategy::Cascade);
    sub.acceptance.required_substrings = vec!["hello".into()];
    let result = f
        .engine
        .run(sub.clone(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.status, "completed");
    assert_eq!(ledger(&f, &sub.task_id).await.calls, 2);
    let requests = f.fake.requests.lock().unwrap().clone();
    assert_eq!(requests[0]["model"], "local");
    assert_eq!(requests[1]["model"], "other");
    let saved_plan = saved(&f, &sub.task_id).1;
    let s: RoutingSnapshot =
        serde_json::from_value(saved_plan["routing_snapshot"].clone()).unwrap();
    assert_eq!(
        saved_plan["effective_plan"]["routing_profile_version"],
        s.profile_version
    );
    let db = rusqlite::Connection::open(&f.config.database_path).unwrap();
    let rows: Vec<String> = db
        .prepare("SELECT metadata FROM attempts WHERE task=? ORDER BY rowid")
        .unwrap()
        .query_map([&sub.task_id], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let mut task_value = json!(sub);
    for key in ["schema_version", "task_type", "planning"] {
        task_value.as_object_mut().unwrap().remove(key);
    }
    let effective_task: TaskSpec = serde_json::from_value(task_value).unwrap();
    for row in rows {
        let metadata: Value = serde_json::from_str(&row).unwrap();
        let stored: router::RoutingDecision =
            serde_json::from_value(metadata["route"].clone()).unwrap();
        let inputs = &metadata["route_inputs"];
        let d = router::route_profiled(
            &s,
            RouteRequest {
                task: &effective_task,
                node: &stored.node_id,
                input_tokens: stored.estimated_input,
                available: inputs["available"].as_u64().unwrap(),
                excluded: &serde_json::from_value(inputs["excluded"].clone()).unwrap(),
                quality_floor: inputs["quality_floor"].as_f64().unwrap(),
                health: &serde_json::from_value(inputs["health"].clone()).unwrap(),
            },
        )
        .unwrap();
        assert_eq!(stored.model_id, d.model_id);
        assert_eq!(stored.score, d.score);
        assert_eq!(stored.routing_snapshot, Some(digest(&s)));
    }
    f.engine.close().await.unwrap();
}

#[tokio::test]
async fn critic_uses_code_review_profile_and_host_mapping_is_versioned() {
    for override_mapping in [false, true] {
        let f = setup(
            vec![
                response("{}", true),
                response(
                    r#"{"status":"pass","checks":[],"evidence":[],"defects":[],"action":"accept"}"#,
                    true,
                ),
            ],
            |c| {
                typed_config(c);
                profiles(c).profiles = vec![
                    profile("local", TaskType::CodeGeneration, "generator", 0.99),
                    profile("other", TaskType::CodeReview, "critic", 0.99),
                    profile("local", TaskType::Reasoning, "invoke", 0.99),
                ];
                if override_mapping {
                    profiles(c).role_mappings.push(RoleMapping {
                        task_type: TaskType::CodeGeneration,
                        node: "critic".into(),
                        profile: RoleProfile {
                            task_type: TaskType::Reasoning,
                            role: "invoke".into(),
                        },
                    });
                }
            },
        )
        .await;
        let mut sub = submission();
        sub.task_type = Some(TaskType::CodeGeneration);
        sub.strategy = Some(Strategy::GeneratorCritic);
        sub.constraints.different_critic = false;
        assert_eq!(
            f.engine
                .run(sub.clone(), CancellationToken::new())
                .await
                .unwrap()
                .status,
            "completed"
        );
        let requests = f.fake.requests.lock().unwrap().clone();
        assert_eq!(requests[0]["model"], "local");
        assert_eq!(
            requests[1]["model"],
            if override_mapping { "local" } else { "other" }
        );
        assert_eq!(requests[1]["tools"], json!([]));
        assert_eq!(
            saved(&f, &sub.task_id).1["effective_plan"]["role_profiles"]["critic"]["task_type"],
            if override_mapping {
                "reasoning"
            } else {
                "code_review"
            }
        );
        f.engine.close().await.unwrap();
    }
}

#[tokio::test]
async fn planner_stays_global_while_execution_is_typed() {
    let f = setup(
        vec![
            response(&proposal().to_string(), true),
            response("{}", true),
        ],
        |c| {
            typed_config(c);
            profiles(c)
                .profiles
                .push(profile("other", TaskType::Writing, "invoke", 0.99));
        },
    )
    .await;
    f.engine
        .run(submission(), CancellationToken::new())
        .await
        .unwrap();
    let requests = f.fake.requests.lock().unwrap().clone();
    assert_eq!(requests[0]["node"], "planner");
    assert_eq!(requests[0]["model"], "local");
    assert_eq!(requests[1]["model"], "other");
    f.engine.close().await.unwrap();
}

#[tokio::test]
async fn typed_cancellation_keeps_snapshot_and_unresolved_reservation() {
    let f = setup(vec![Action::Block], typed_config).await;
    let mut sub = submission();
    sub.task_type = Some(TaskType::Writing);
    sub.strategy = Some(Strategy::Single);
    let cancel = CancellationToken::new();
    let engine = f.engine.clone();
    let token = cancel.clone();
    let input = sub.clone();
    let handle = tokio::spawn(async move { engine.run(input, token).await });
    tokio::time::timeout(std::time::Duration::from_secs(3), f.fake.started.notified())
        .await
        .unwrap();
    cancel.cancel();
    assert_eq!(handle.await.unwrap().unwrap_err().kind, "cancelled");
    assert!(ledger(&f, &sub.task_id).await.reserved > 0);
    let records = f.engine.recovery_records().await.unwrap();
    assert!(records
        .iter()
        .any(|r| r.plan["routing_snapshot"].is_object()));
    f.engine.close().await.unwrap();
}

#[test]
fn no_profile_config_still_captures_global_prior_snapshot() {
    let mut c = config();
    c.routing_profiles = None;
    let s = RoutingSnapshot::capture(&c, &mut plan(TaskType::General), 0);
    assert_eq!(s.profile_version, "global-prior-v1");
    assert_eq!(
        s.nodes["invoke"].qualities["local"].fallback,
        FallbackTier::GlobalPrior
    );
}

#[test]
fn snapshot_json_roundtrip_preserves_float_bits_and_hash() {
    let mut c = config();
    let mut state = 42_u64;
    for _ in 0..256 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let prior = state as f64 / u64::MAX as f64;
        profiles(&mut c).profiles = vec![profile("local", TaskType::Writing, "invoke", prior)];
        let original = snapshot(&c, TaskType::Writing);
        let restored: RoutingSnapshot =
            serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
        assert_eq!(digest(&original), digest(&restored), "prior {prior}");
        assert_eq!(
            choose(&original, &task(), 0.0).score,
            choose(&restored, &task(), 0.0).score
        );
    }
}

#[test]
fn legacy_decision_json_remains_readable() {
    let s = snapshot(&config(), TaskType::General);
    let mut old = json!(choose(&s, &task(), 0.0));
    old.as_object_mut().unwrap().remove("quality");
    old.as_object_mut().unwrap().remove("routing_snapshot");
    let decoded: router::RoutingDecision = serde_json::from_value(old).unwrap();
    assert!(decoded.quality.is_none());
    assert!(decoded.routing_snapshot.is_none());
}

#[tokio::test]
async fn explicit_task_uses_general_profiles_and_persists_a_plan() {
    let f = setup(vec![response("{}", true)], |c| {
        typed_config(c);
        profiles(c)
            .profiles
            .push(profile("other", TaskType::General, "invoke", 0.99));
    })
    .await;
    let mut input: TaskSpec = serde_json::from_str(include_str!("../examples/task.json")).unwrap();
    input.task_id = id();
    input.deadline_ms = now_ms() + 30_000;
    f.engine
        .run(input.clone(), CancellationToken::new())
        .await
        .unwrap();
    let requests = f.fake.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["model"], "other");
    let (raw, plan, _, _) = saved(&f, &input.task_id);
    assert_eq!(raw["payload_version"], 1);
    assert_eq!(plan["planning"]["skipped"], true);
    assert_eq!(plan["effective_plan"]["task_type"], "general");
    assert_eq!(
        plan["routing_snapshot"]["nodes"]["invoke"]["qualities"]["other"]["fallback"],
        "exact"
    );
    f.engine.close().await.unwrap();
}
