#[allow(dead_code)]
#[path = "routing/support.rs"]
mod support;
use model_collaboration_engine::{contracts::*, router::RoutingSnapshot};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use support::{choose, config, plan, task};

fn rankings() -> Value {
    json!({"schema_version":1,"version":"r1","source":"synthetic","source_version":"s1",
        "category":"reasoning","published_at_ms":0,"expires_at_ms":20000,
        "population":10,"weight":0.5,"entries":[{"model_id":"other","model_version":"1",
        "source_model":"external","source_model_version":"v1","task_type":"reasoning",
        "role":"invoke","rank":1}]})
}
fn configured(value: Value) -> Config {
    let mut c = json!(config());
    c["rankings"] = value;
    serde_json::from_value(c).unwrap()
}
#[test]
fn external_rank_changes_selection_without_changing_quality() {
    let c = configured(rankings());
    c.validate(false).unwrap();
    let s = RoutingSnapshot::capture(&c, &mut plan(TaskType::Reasoning), 10000);
    let d = choose(&s, &task(), 0.0);
    assert_eq!(d.model_id, "other");
    assert_eq!(d.breakdown["ranking"], 0.5);
    assert_eq!(d.quality.unwrap().quality, 0.8);
}

#[test]
fn external_rank_cannot_override_unequal_task_fit_value() {
    let mut c = configured(rankings());
    c.models[0].acceptance = 0.81;
    c.models[1].acceptance = 0.80;
    let s = RoutingSnapshot::capture(&c, &mut plan(TaskType::Reasoning), 10000);
    let d = choose(&s, &task(), 0.0);
    assert_eq!(d.model_id, "local");
    assert_eq!(d.breakdown["ranking"], 0.0);
}

#[test]
fn critic_uses_legacy_role_score_but_task_fit_ranking_tie_break() {
    let mut ranking = rankings();
    ranking["entries"][0]["role"] = json!("critic");
    let mut config = configured(ranking);
    config.models[0].acceptance = 0.8;
    config.models[1].acceptance = 0.7;
    let mut plan = plan(TaskType::Reasoning);
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
    let decision = model_collaboration_engine::router::route_profiled(
        &snapshot,
        model_collaboration_engine::router::RouteRequest {
            task: &task(),
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

#[test]
fn schema_three_keeps_legacy_total_score_then_model_id_ordering() {
    let mut c = configured(rankings());
    c.models[0].id = "z-local".into();
    c.models[0].acceptance = 0.5;
    c.models[1].acceptance = 0.0;
    c.models[1].input_price = c.models[0].input_price + 1;
    let mut snapshot = support::snapshot(&c, TaskType::Reasoning);
    snapshot.schema_version = 3;
    snapshot.selection = None;
    snapshot.algorithm_version = None;
    let decision = choose(&snapshot, &task(), 0.0);
    assert_eq!(decision.model_id, "other");
    assert_eq!(decision.score, 0.5);
}

#[test]
fn ranking_cannot_override_hard_constraints_or_quality_floor() {
    let mut c = configured(rankings());
    c.models[1].acceptance = 0.4;
    let s = support::snapshot(&c, TaskType::Reasoning);
    let d = choose(&s, &task(), 0.7);
    assert_eq!(d.model_id, "local");
    assert_eq!(d.excluded["other"], ["quality_floor"]);
    for reason in [
        "allowed_models",
        "provider",
        "region",
        "capabilities",
        "context",
        "budget",
    ] {
        let mut c = configured(rankings());
        let mut t = task();
        match reason {
            "allowed_models" => {
                t.constraints.allowed_models.insert("local".into());
            }
            "provider" => {
                c.models[1].provider = "blocked".into();
                t.constraints
                    .allowed_providers
                    .insert(c.models[0].provider.clone());
            }
            "region" => {
                c.models[1].region = "blocked".into();
                t.constraints
                    .allowed_regions
                    .insert(c.models[0].region.clone());
            }
            "capabilities" => {
                c.models[1].capabilities.clear();
            }
            "context" => {
                c.models[1].context_tokens = 1;
            }
            "budget" => {
                c.models[1].input_price = 1_000_000;
            }
            _ => unreachable!(),
        }
        let d = choose(&support::snapshot(&c, TaskType::Reasoning), &t, 0.0);
        assert_eq!(d.model_id, "local");
        assert!(d.excluded["other"].iter().any(|v| v == reason));
    }
}

#[test]
fn unknown_and_inactive_evidence_has_explicit_status() {
    for (field, value, status) in [
        ("expires_at_ms", json!(10000), "expired"),
        ("published_at_ms", json!(10001), "not_yet_effective"),
        ("weight", json!(0), "disabled"),
        ("model_version", json!("old"), "model_version_mismatch"),
        ("task_type", json!("writing"), "task_type_mismatch"),
        ("role", json!("critic"), "role_mismatch"),
    ] {
        let mut v = rankings();
        if ["model_version", "task_type", "role"].contains(&field) {
            v["entries"][0][field] = value;
        } else {
            v[field] = value;
        }
        let s = support::snapshot(&configured(v), TaskType::Reasoning);
        let r = &json!(s)["nodes"]["invoke"]["rankings"]["other"];
        assert_eq!(r["status"], status);
        assert_eq!(r["contribution"], 0.0);
        assert_eq!(r["normalized_rank"], Value::Null);
    }
    let s = support::snapshot(&configured(rankings()), TaskType::Reasoning);
    assert_eq!(
        json!(s)["nodes"]["invoke"]["rankings"]["local"]["status"],
        "missing"
    );
    let s = support::snapshot(&config(), TaskType::Reasoning);
    assert_eq!(
        json!(s)["nodes"]["invoke"]["rankings"]["local"]["status"],
        "disabled"
    );
}

#[test]
fn snapshot_roundtrip_freezes_reference_and_old_snapshots_remain_routable() {
    let mut c = configured(rankings());
    let s = support::snapshot(&c, TaskType::Reasoning);
    let before = choose(&s, &task(), 0.0);
    c.rankings.as_mut().unwrap().entries[0].rank = 10;
    let after: RoutingSnapshot = serde_json::from_value(json!(s)).unwrap();
    assert_eq!(digest(&s), digest(&after));
    let d = choose(&after, &task(), 0.0);
    assert_eq!(d.score, before.score);
    assert_eq!(d.model_id, before.model_id);
    assert_eq!(after.ranking_config.as_ref().unwrap().entries[0].rank, 1);
    assert_eq!(
        after.ranking_config_hash,
        Some(digest(s.ranking_config.as_ref().unwrap()))
    );
    assert_eq!(
        choose(&support::snapshot(&c, TaskType::Reasoning), &task(), 0.0).model_id,
        "local"
    );
    for version in [1, 2] {
        let mut historical = json!(s);
        historical["schema_version"] = json!(version);
        historical.as_object_mut().unwrap().remove("ranking_config");
        historical
            .as_object_mut()
            .unwrap()
            .remove("ranking_config_hash");
        historical["nodes"]["invoke"]
            .as_object_mut()
            .unwrap()
            .remove("rankings");
        let restored: RoutingSnapshot = serde_json::from_value(historical.clone()).unwrap();
        assert_eq!(json!(restored), historical);
        let d = choose(&restored, &task(), 0.0);
        assert_eq!(d.model_id, "local");
        assert!(d.ranking.is_none());
        assert!(!d.breakdown.contains_key("ranking"));
    }
}

#[test]
fn normalization_uses_full_population_and_allows_ties_and_singletons() {
    for (rank, population, expected) in [(1, 1, 0.5), (2, 10, 4.0 / 9.0), (10, 10, 0.0)] {
        let mut v = rankings();
        v["population"] = json!(population);
        v["entries"][0]["rank"] = json!(rank);
        let mut tied = v["entries"][0].clone();
        tied["model_id"] = json!("local");
        v["entries"].as_array_mut().unwrap().push(tied);
        let s = support::snapshot(&configured(v), TaskType::Reasoning);
        assert!((choose(&s, &task(), 0.0).breakdown["ranking"] - expected).abs() < 1e-12);
    }
}

#[test]
fn rejects_invalid_metadata_mappings_and_unknown_fields() {
    for (field, value) in [
        ("schema_version", json!(2)),
        ("population", json!(0)),
        ("population", json!(1_000_001)),
        ("weight", json!(-0.1)),
        ("weight", json!(1.1)),
        ("version", json!(" ")),
        ("category", json!("")),
        ("source", json!("x".repeat(2049))),
        ("source_version", json!("x".repeat(257))),
        ("expires_at_ms", json!(0)),
        ("published_at_ms", json!(u64::MAX)),
    ] {
        let mut v = rankings();
        v[field] = value;
        assert!(configured(v).validate(false).is_err(), "{field}");
    }
    for (field, value) in [
        ("model_id", json!("unknown")),
        ("model_version", json!("")),
        ("source_model_version", json!("")),
        ("role", json!("planner")),
        ("rank", json!(0)),
        ("rank", json!(11)),
    ] {
        let mut v = rankings();
        v["entries"][0][field] = value;
        assert!(configured(v).validate(false).is_err(), "{field}");
    }
    let mut v = rankings();
    let duplicate = v["entries"][0].clone();
    v["entries"].as_array_mut().unwrap().push(duplicate);
    assert!(configured(v).validate(false).is_err());
    let mut v = rankings();
    let mut conflict = v["entries"][0].clone();
    conflict["model_id"] = json!("local");
    conflict["rank"] = json!(2);
    v["entries"].as_array_mut().unwrap().push(conflict);
    assert!(configured(v).validate(false).is_err());
    let mut v = rankings();
    v["extra"] = json!(1);
    assert!(serde_json::from_value::<RankingConfig>(v).is_err());
    let mut v = rankings();
    v["entries"][0]["extra"] = json!(1);
    assert!(serde_json::from_value::<RankingConfig>(v).is_err());
}

#[test]
fn global_planner_route_does_not_use_external_rankings() {
    use model_collaboration_engine::router::{route, RouteRequest};
    use std::collections::{BTreeMap, BTreeSet};
    let c = configured(rankings());
    let decision = route(
        &c,
        RouteRequest {
            task: &task(),
            node: "planner",
            input_tokens: 100,
            available: 100_000,
            excluded: &BTreeSet::new(),
            quality_floor: 0.0,
            health: &BTreeMap::new(),
        },
    )
    .unwrap();
    assert_eq!(decision.model_id, "local");
    assert!(decision.ranking.is_none());
    assert!(!decision.breakdown.contains_key("ranking"));
}
