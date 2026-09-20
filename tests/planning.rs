#[path = "planning/support.rs"]
mod support;
use model_collaboration_engine::{
    assessment::{AssessmentRule, ExecutionClass, RuleOperator, RuleSet, TaskFactValue, TaskFacts},
    contracts::*,
    store::Store,
};
use serde_json::{json, Value};
use support::*;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn planning_and_execution_share_accounting_and_preserve_host_acceptance() {
    let f = setup(
        vec![
            response(&proposal().to_string(), true),
            response(r#"{"greeting":"hello"}"#, true),
        ],
        |_| {},
    )
    .await;
    let sub = submission();
    let (sender, mut events) = f.engine.event_channel();
    let result = f
        .engine
        .run_with_host(sub.clone(), CancellationToken::new(), None, Some(sender))
        .await
        .unwrap();
    assert_eq!(result.settled_cost, 30);
    assert_eq!(ledger(&f, &sub.task_id).await.calls, 2);
    {
        let requests = f.fake.requests.lock().unwrap();
        assert_eq!(requests[0]["node"], "planner");
        assert_eq!(requests[0]["tools"], json!([]));
        assert_eq!(requests[0]["json_object"], true);
    }
    let (raw, plan, _, status) = saved(&f, &sub.task_id);
    assert_eq!(raw["submission"], json!(sub));
    assert_eq!(status, "completed");
    assert_eq!(plan["effective_plan"]["acceptance"]["json_object"], true);
    assert_eq!(plan["effective_plan"]["acceptance"]["nonempty"], true);
    assert_eq!(plan["effective_plan"]["submission_hash"], digest(&sub));
    assert_eq!(
        plan["effective_plan"]["selection"]["version"],
        "host-task-fit-v1"
    );
    assert_eq!(
        plan["effective_plan"]["selection"]["cost_reference"],
        10_000
    );
    let mut kinds = vec![];
    while let Some(event) = events.recv().await {
        kinds.push(event.kind);
        assert_eq!(event.sequence, kinds.len() as u64);
    }
    assert_eq!(
        &kinds[..5],
        &[
            "task_started",
            "planning_started",
            "model_started",
            "planning_completed",
            "plan_validated"
        ]
    );
    f.engine.close().await.unwrap();
}

#[tokio::test]
async fn schema_three_facts_freeze_the_assessed_execution_class() {
    let f = setup(vec![response("{}", true)], |config| {
        for model in &mut config.models {
            model.execution_class = ExecutionClass::Hard;
        }
        config.assessment_rules = Some(RuleSet {
            version: "coding-demand-v1".into(),
            rules: vec![AssessmentRule {
                id: "many-files".into(),
                fact: "file_count".into(),
                op: RuleOperator::Gt,
                value: TaskFactValue::Integer(5),
                minimum_execution_class: ExecutionClass::Hard,
            }],
        });
    })
    .await;
    let mut sub = submission();
    sub.schema_version = 3;
    sub.strategy = Some(Strategy::Single);
    sub.task_type = Some(TaskType::Writing);
    sub.task_facts = Some(TaskFacts {
        version: "coding-facts-v1".into(),
        values: [("file_count".into(), TaskFactValue::Integer(7))]
            .into_iter()
            .collect(),
    });
    let result = f
        .engine
        .run(sub.clone(), CancellationToken::new())
        .await
        .unwrap();
    let (_, plan, _, _) = saved(&f, &sub.task_id);
    assert_eq!(
        plan["effective_plan"]["assessment"]["execution_class"],
        "hard"
    );
    assert_eq!(plan["routing_snapshot"]["minimum_execution_class"], "hard");
    assert_eq!(result.status, "completed");
    f.engine.close().await.unwrap();
}

#[tokio::test]
async fn explicit_disabled_and_complete_auto_skip_planning_without_a_pool() {
    for mode in [PlanningMode::Disabled, PlanningMode::Auto] {
        let f = setup(vec![response("{}", true)], |c| c.planner_models.clear()).await;
        let mut sub = submission();
        sub.planning.mode = mode;
        sub.task_type = Some(TaskType::Writing);
        sub.strategy = Some(Strategy::Single);
        f.engine
            .run(sub.clone(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(ledger(&f, &sub.task_id).await.calls, 1);
        assert_eq!(saved(&f, &sub.task_id).1["planning"]["skipped"], true);
        f.engine.close().await.unwrap();
    }
}

#[tokio::test]
async fn required_plans_even_with_host_choices_and_rejects_conflicts() {
    for conflict in [false, true] {
        let f = setup(
            vec![
                response(&proposal().to_string(), true),
                response("{}", true),
            ],
            |_| {},
        )
        .await;
        let mut sub = submission();
        sub.planning.mode = PlanningMode::Required;
        sub.strategy = Some(Strategy::Single);
        sub.task_type = Some(if conflict {
            TaskType::Reasoning
        } else {
            TaskType::Writing
        });
        let result = f.engine.run(sub.clone(), CancellationToken::new()).await;
        if conflict {
            assert_eq!(result.unwrap_err().kind, "plan_validation");
        } else {
            assert_eq!(result.unwrap().status, "completed");
        }
        assert_eq!(
            ledger(&f, &sub.task_id).await.calls,
            if conflict { 1 } else { 2 }
        );
        f.engine.close().await.unwrap();
    }
}

#[tokio::test]
async fn invalid_proposals_are_settled_before_rejection() {
    let mut unknown = proposal();
    unknown["selection"] = json!({"version":"planner-must-not-change-host-policy"});
    let mut capability = proposal();
    capability["required_capabilities"] = json!(["shell"]);
    let mut confidence = proposal();
    confidence["classification_confidence"] = json!(1.1);
    let mut version = proposal();
    version["suggested_acceptance"]["version"] = json!("unregistered");
    let mut strategy = proposal();
    strategy["strategy"] = json!("arbitrary_graph");
    for text in [
        "not json".into(),
        unknown.to_string(),
        capability.to_string(),
        confidence.to_string(),
        version.to_string(),
        strategy.to_string(),
    ] {
        let f = setup(vec![response(&text, true)], |_| {}).await;
        let sub = submission();
        assert_eq!(
            f.engine
                .run(sub.clone(), CancellationToken::new())
                .await
                .unwrap_err()
                .kind,
            "plan_validation"
        );
        assert_eq!(ledger(&f, &sub.task_id).await.settled, 15);
        assert_eq!(ledger(&f, &sub.task_id).await.calls, 1);
        let connection = rusqlite::Connection::open(&f.config.database_path).unwrap();
        let outcome: String = connection
            .query_row("SELECT outcome FROM attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&outcome).unwrap()["planning_output"]["text"],
            text
        );
        f.engine.close().await.unwrap();
    }
}

#[tokio::test]
async fn explicit_fallback_keeps_unknown_planning_cost_and_recovery_evidence() {
    let f = setup(vec![response("bad", false), response("{}", true)], |_| {}).await;
    let mut sub = submission();
    sub.planning.fallback = Some(PlanChoice {
        task_type: TaskType::General,
        strategy: Strategy::Single,
    });
    let result = f
        .engine
        .run(sub.clone(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.settled_cost, 15);
    assert!(result.reserved_cost > 0);
    let records = f.engine.recovery_records().await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].submission.as_ref().unwrap().task_id, sub.task_id);
    assert_eq!(records[0].attempts[0].state, "unresolved");
    assert_eq!(
        records[0].plan["planning"]["error"]["kind"],
        "plan_validation"
    );
    assert!(records[0].plan["effective_plan"]["proposal_id"].is_null());
    f.engine.close().await.unwrap();
    let reopened = model_collaboration_engine::engine::Engine::open(f.config.clone())
        .await
        .unwrap();
    assert_eq!(
        json!(reopened.recovery_records().await.unwrap()),
        json!(records)
    );
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn recovery_projects_legacy_submission_without_panicking_or_rewriting_evidence() {
    let f = setup(vec![response("bad", false), response("{}", true)], |_| {}).await;
    let mut sub = submission();
    sub.planning.fallback = Some(PlanChoice {
        task_type: TaskType::General,
        strategy: Strategy::Single,
    });
    f.engine
        .run(sub.clone(), CancellationToken::new())
        .await
        .unwrap();

    let connection = rusqlite::Connection::open(&f.config.database_path).unwrap();
    let (spec, plan): (String, String) = connection
        .query_row(
            "SELECT spec,plan FROM tasks WHERE id=?",
            [&sub.task_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let mut spec: Value = serde_json::from_str(&spec).unwrap();
    spec["submission"]["schema_version"] = json!(1);
    spec["submission"]
        .as_object_mut()
        .unwrap()
        .remove("selection");
    let mut plan: Value = serde_json::from_str(&plan).unwrap();
    plan["effective_plan"]
        .as_object_mut()
        .unwrap()
        .remove("selection");
    connection
        .execute(
            "UPDATE tasks SET spec=?,plan=? WHERE id=?",
            rusqlite::params![spec.to_string(), plan.to_string(), sub.task_id],
        )
        .unwrap();
    drop(connection);

    let records = f.engine.recovery_records().await.unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].submission.as_ref().unwrap().selection.is_none());
    assert_eq!(
        records[0].task.selection.version,
        "legacy-recovery-evidence-only-v1"
    );
    f.engine.close().await.unwrap();
}

#[tokio::test]
async fn planning_pool_cannot_bypass_host_data_constraints() {
    for restriction in [
        "pool", "local", "provider", "region", "model", "json", "context",
    ] {
        let f = setup(vec![], |c| match restriction {
            "pool" => c.planner_models.clear(),
            "local" => c.models[0].local = false,
            "json" => {
                c.models[0].capabilities.remove("json");
            }
            "context" => c.models[0].context_tokens = 10,
            _ => {}
        })
        .await;
        let mut sub = submission();
        match restriction {
            "provider" => {
                sub.constraints.allowed_providers.insert("other".into());
            }
            "region" => {
                sub.constraints.allowed_regions.insert("other".into());
            }
            "model" => {
                sub.constraints.allowed_models.insert("other".into());
            }
            _ => {}
        }
        assert_eq!(
            f.engine
                .run(sub.clone(), CancellationToken::new())
                .await
                .unwrap_err()
                .kind,
            "planning"
        );
        assert_eq!(ledger(&f, &sub.task_id).await.calls, 0);
        f.engine.close().await.unwrap();
    }
}

#[tokio::test]
async fn total_call_exhaustion_and_usage_overrun_never_fall_back() {
    for overrun in [false, true] {
        let action = if overrun {
            Action::Reply(Ok(model_collaboration_engine::adapter::ModelOutput {
                text: proposal().to_string(),
                tool_calls: vec![],
                usage: Some(Usage {
                    input_tokens: 50_000,
                    output_tokens: 1,
                }),
                request_id: None,
                complete: true,
            }))
        } else {
            response(&proposal().to_string(), true)
        };
        let f = setup(vec![action], |_| {}).await;
        let mut sub = submission();
        sub.max_calls = 1;
        sub.planning.fallback = Some(PlanChoice {
            task_type: TaskType::General,
            strategy: Strategy::Single,
        });
        assert_eq!(
            f.engine
                .run(sub.clone(), CancellationToken::new())
                .await
                .unwrap_err()
                .kind,
            "budget"
        );
        assert_eq!(ledger(&f, &sub.task_id).await.calls, 1);
        assert_eq!(
            ledger(&f, &sub.task_id).await.settled,
            if overrun { 50_001 } else { 15 }
        );
        f.engine.close().await.unwrap();
    }
}

#[path = "planning/timeouts.rs"]
mod timeouts;

#[tokio::test]
async fn cancellation_and_close_finish_planning_accounting_without_fallback() {
    for shutdown in [false, true] {
        let f = setup(vec![Action::Block], |_| {}).await;
        let mut sub = submission();
        sub.planning.fallback = Some(PlanChoice {
            task_type: TaskType::General,
            strategy: Strategy::Single,
        });
        let cancel = CancellationToken::new();
        let pending = tokio::spawn({
            let engine = f.engine.clone();
            let sub = sub.clone();
            let cancel = cancel.clone();
            async move { engine.run(sub, cancel).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), f.fake.started.notified())
            .await
            .unwrap();
        if shutdown {
            f.engine.close().await.unwrap();
        } else {
            cancel.cancel();
        }
        assert_eq!(pending.await.unwrap().unwrap_err().kind, "cancelled");
        assert_eq!(f.fake.requests.lock().unwrap().len(), 1);
        assert_eq!(saved(&f, &sub.task_id).3, "cancelled");
        if !shutdown {
            assert_eq!(
                f.store.records().await.unwrap()[0].attempts[0].state,
                "unresolved"
            );
            f.engine.close().await.unwrap();
        }
        let reopened = model_collaboration_engine::engine::Engine::open(f.config.clone())
            .await
            .unwrap();
        assert_eq!(
            reopened.recovery_records().await.unwrap()[0].attempts[0].state,
            "unresolved"
        );
        reopened.close().await.unwrap();
    }
}

#[tokio::test]
async fn version_and_mode_errors_reject_before_admission() {
    let f = setup(vec![], |_| {}).await;
    for variant in 0..4 {
        let mut sub = submission();
        match variant {
            0 => sub.schema_version = 1,
            1 => sub.planning.mode = PlanningMode::Disabled,
            2 => sub.planning.max_calls = 2,
            _ => sub.planning.timeout_ms = 0,
        }
        assert_eq!(
            f.engine
                .run(sub, CancellationToken::new())
                .await
                .unwrap_err()
                .kind,
            "configuration"
        );
    }
    assert!(f.store.records().await.unwrap().is_empty());
    assert!(f.fake.requests.lock().unwrap().is_empty());
    f.engine.close().await.unwrap();
}

#[path = "planning/store_fault.rs"]
mod store_fault;

#[tokio::test]
async fn settlement_storage_failure_stops_before_fallback_or_execution() {
    let f = setup(vec![response(&proposal().to_string(), true)], |_| {}).await;
    let engine = model_collaboration_engine::engine::Engine::with_components(
        f.config.clone(),
        std::sync::Arc::new(store_fault::FaultStore {
            inner: f.store.clone(),
            fail_settlement: true,
            reservation_gate: None,
        }),
        f.fake.clone(),
    )
    .unwrap();
    let mut sub = submission();
    sub.planning.fallback = Some(PlanChoice {
        task_type: TaskType::General,
        strategy: Strategy::Single,
    });
    assert_eq!(
        engine
            .run(sub.clone(), CancellationToken::new())
            .await
            .unwrap_err()
            .kind,
        "storage"
    );
    assert_eq!(ledger(&f, &sub.task_id).await.calls, 1);
    assert_eq!(
        f.store.records().await.unwrap()[0].attempts[0].state,
        "pending"
    );
    assert_eq!(saved(&f, &sub.task_id).3, "failed");
    engine.close().await.unwrap();
}

#[tokio::test]
async fn planner_selection_failure_uses_only_explicit_validated_fallback() {
    for no_pool in [false, true] {
        let f = setup(vec![response("{}", true)], |c| {
            if no_pool {
                c.planner_models.clear();
            }
        })
        .await;
        let mut sub = submission();
        sub.planning.max_cost = 0;
        sub.planning.fallback = Some(PlanChoice {
            task_type: TaskType::General,
            strategy: Strategy::Single,
        });
        f.engine
            .run(sub.clone(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(ledger(&f, &sub.task_id).await.calls, 1);
        assert_eq!(f.fake.requests.lock().unwrap()[0]["node"], "invoke");
        assert_eq!(
            saved(&f, &sub.task_id).1["planning"]["error"]["kind"],
            "planning"
        );
        f.engine.close().await.unwrap();
    }
}

#[tokio::test]
async fn planned_critic_preserves_diversity_and_records_role_mapping() {
    let mut proposal = proposal();
    proposal["task_type"] = json!("code_generation");
    proposal["strategy"] = json!("generator_critic");
    let f = setup(
        vec![
            response(&proposal.to_string(), true),
            response("{}", true),
            response(
                r#"{"status":"pass","checks":[],"evidence":[],"defects":[],"action":"accept"}"#,
                true,
            ),
        ],
        |c| {
            let mut critic = c.models[0].clone();
            critic.id = "reviewer".into();
            c.models.push(critic);
        },
    )
    .await;
    let mut sub = submission();
    sub.constraints.different_critic = true;
    f.engine
        .run(sub.clone(), CancellationToken::new())
        .await
        .unwrap();
    let plan = saved(&f, &sub.task_id).1;
    assert_eq!(
        plan["effective_plan"]["role_profiles"]["critic"]["task_type"],
        "code_review"
    );
    {
        let requests = f.fake.requests.lock().unwrap();
        assert_ne!(requests[1]["model"], requests[2]["model"]);
        assert_eq!(requests[2]["tools"], json!([]));
    }
    assert_eq!(ledger(&f, &sub.task_id).await.calls, 3);
    f.engine.close().await.unwrap();
}

#[tokio::test]
async fn current_schema_preserves_existing_task_evidence() {
    // Existing records within the current schema remain readable without a migration.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    let legacy: Value = serde_json::from_str(include_str!("../examples/task.json")).unwrap();
    let initialized = model_collaboration_engine::store::SqliteStore::open(path.to_str().unwrap())
        .await
        .unwrap();
    initialized.close().await.unwrap();
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute("INSERT INTO tasks(id,spec,config_hash,plan,status,total,checkpoint) VALUES(?,?,'old','{}','human_required',100000,'{\"version\":1}')",
            rusqlite::params![legacy["task_id"].as_str().unwrap(),legacy.to_string()]).unwrap();
    }
    let store = model_collaboration_engine::store::SqliteStore::open(path.to_str().unwrap())
        .await
        .unwrap();
    let records = store.records().await.unwrap();
    assert_eq!(json!(records[0].task), legacy);
    assert!(records[0].submission.is_none());
    assert_eq!(records[0].plan, json!({}));
    store.close().await.unwrap();
}
