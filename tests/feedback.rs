//! Durable host feedback must never be inferred from execution success.
#[path = "planning/support.rs"]
mod support;
use model_collaboration_engine::{contracts::*, engine::Engine};
use serde_json::json;
use support::*;
use tokio_util::sync::CancellationToken;

fn explicit() -> SubmissionSpec {
    let mut s = submission();
    s.strategy = Some(Strategy::Single);
    s.task_type = Some(TaskType::Writing);
    s
}
fn feedback(result: &TaskResult) -> Feedback {
    Feedback {
        feedback_id: id(),
        task_id: result.task_id.clone(),
        artifact_id: result.artifact.artifact_id.clone(),
        kind: FeedbackKind::BusinessAcceptance,
        evaluator_version: "1".into(),
        accepted: false,
        reason: "Host rejected greeting".into(),
    }
}
#[tokio::test]
async fn feedback_is_idempotent_persistent_and_only_changes_future_snapshots() {
    let f = setup(vec![response("{}", true), response("{}", true)], |_| {}).await;
    let result = f
        .engine
        .run(explicit(), CancellationToken::new())
        .await
        .unwrap();
    let original = saved(&f, &result.task_id).1;
    let before = f.engine.metrics().await.unwrap();
    assert!(before.quality.is_empty());
    assert_eq!(before.tasks.completed, 1);
    assert_eq!(before.calls[0].succeeded, 1);
    assert_eq!(
        before.calls[0].known_cost,
        ledger(&f, &result.task_id).await.settled
    );
    assert!(before.calls[0].mean_latency_ms.is_some());
    let fb = feedback(&result);
    let (a, b) = tokio::join!(f.engine.record_feedback(&fb), f.engine.record_feedback(&fb));
    a.unwrap();
    b.unwrap();
    let metrics = f.engine.metrics().await.unwrap();
    assert_eq!(metrics.revision, 1);
    assert_eq!(metrics.quality[0].samples, 1);
    assert_eq!(metrics.quality[0].accepted, 0);
    assert_eq!(metrics.quality[0].key.model_id, "local");
    assert_eq!(metrics.quality[0].key.task_type, TaskType::Writing);
    assert_eq!(saved(&f, &result.task_id).1, original);
    let next = f
        .engine
        .run(explicit(), CancellationToken::new())
        .await
        .unwrap();
    let snapshot = &saved(&f, &next.task_id).1["routing_snapshot"];
    assert_eq!(snapshot["feedback_revision"], 1);
    assert_eq!(
        snapshot["nodes"]["invoke"]["qualities"]["local"]["evidence"]["samples"],
        1
    );
    let mut conflict = fb.clone();
    conflict.accepted = true;
    assert_eq!(
        f.engine.record_feedback(&conflict).await.unwrap_err().kind,
        "feedback"
    );
    conflict.feedback_id = id();
    assert_eq!(
        f.engine.record_feedback(&conflict).await.unwrap_err().kind,
        "feedback"
    );
    assert_eq!(f.engine.metrics().await.unwrap().revision, 1);
    let records = f.engine.evaluations(&result.task_id).await.unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].critic.is_none());
    f.engine.close().await.unwrap();
    assert_eq!(f.engine.metrics().await.unwrap_err().kind, "closed");
    let reopened = Engine::open(f.config.clone()).await.unwrap();
    assert_eq!(
        json!(reopened.evaluations(&result.task_id).await.unwrap()),
        json!(records)
    );
    assert_eq!(reopened.metrics().await.unwrap().revision, 1);
    reopened.close().await.unwrap();
}
#[tokio::test]
async fn critic_verdict_and_business_acceptance_are_independent() {
    let f = setup(
        vec![
            response("{}", true),
            response(r#"{"status":"revise","checks":[],"evidence":[],"defects":["wrong"],"action":"revise"}"#, true),
        ],
        |_| {},
    )
    .await;
    let mut s = explicit();
    s.strategy = Some(Strategy::GeneratorCritic);
    s.max_rounds = 1;
    let result = f.engine.run(s, CancellationToken::new()).await.unwrap();
    assert_eq!(result.status, "human_required");
    let records = f.engine.evaluations(&result.task_id).await.unwrap();
    assert_eq!(records[0].deterministic.status, EvaluationStatus::Pass);
    assert_eq!(
        records[0].critic.as_ref().unwrap().evaluation.status,
        EvaluationStatus::Revise
    );
    assert!(f.engine.metrics().await.unwrap().quality.is_empty());
    let business = feedback(&result);
    f.engine.record_feedback(&business).await.unwrap();
    let mut critic = business.clone();
    critic.feedback_id = id();
    critic.kind = FeedbackKind::CriticCorrectness;
    critic.accepted = true;
    f.engine.record_feedback(&critic).await.unwrap();
    let m = f.engine.metrics().await.unwrap();
    assert_eq!(m.quality.len(), 2);
    assert!(m
        .quality
        .iter()
        .any(|s| s.kind == FeedbackKind::CriticCorrectness
            && s.accepted == 1
            && s.key.role == "critic"));
    assert!(m
        .quality
        .iter()
        .any(|s| s.kind == FeedbackKind::BusinessAcceptance
            && s.accepted == 0
            && s.key.role == "generator"));
    f.engine.close().await.unwrap();
}
#[tokio::test]
async fn invalid_feedback_does_not_write_and_versions_are_isolated() {
    let f = setup(vec![response("{}", true), response("{}", true)], |_| {}).await;
    let result = f
        .engine
        .run(explicit(), CancellationToken::new())
        .await
        .unwrap();
    let fb = feedback(&result);
    for field in ["task", "artifact", "version", "critic"] {
        let mut invalid = fb.clone();
        match field {
            "task" => invalid.task_id = id(),
            "artifact" => invalid.artifact_id = id(),
            "version" => invalid.evaluator_version = "2".into(),
            _ => invalid.kind = FeedbackKind::CriticCorrectness,
        }
        assert_eq!(
            f.engine.record_feedback(&invalid).await.unwrap_err().kind,
            "feedback"
        );
    }
    assert_eq!(f.engine.metrics().await.unwrap().revision, 0);
    f.engine.record_feedback(&fb).await.unwrap();
    let mut next = explicit();
    next.acceptance.version = "2".into();
    let result = f.engine.run(next, CancellationToken::new()).await.unwrap();
    assert!(
        saved(&f, &result.task_id).1["routing_snapshot"]["nodes"]["invoke"]["qualities"]["local"]
            ["evidence"]
            .is_null()
    );
    f.engine.close().await.unwrap();
}
#[tokio::test]
async fn planner_and_unknown_cost_calls_are_accounted_without_quality_samples() {
    let f = setup(
        vec![
            response(&proposal().to_string(), false),
            response("{}", true),
        ],
        |_| {},
    )
    .await;
    let result = f
        .engine
        .run(submission(), CancellationToken::new())
        .await
        .unwrap();
    let metrics = f.engine.metrics().await.unwrap();
    assert_eq!(metrics.calls.len(), 2);
    assert_eq!(metrics.calls.iter().map(|s| s.attempts).sum::<u64>(), 2);
    assert_eq!(
        metrics
            .calls
            .iter()
            .map(|s| s.unresolved_reserved)
            .sum::<u64>(),
        result.reserved_cost
    );
    assert_eq!(
        metrics.calls.iter().map(|s| s.known_cost).sum::<u64>(),
        result.settled_cost
    );
    assert!(metrics.quality.is_empty());
    f.engine.close().await.unwrap();
}
#[tokio::test]
async fn cancelled_critic_retains_candidate_but_no_invented_critic_verdict() {
    let f = setup(vec![response("{}", true), Action::Block], |_| {}).await;
    let mut s = explicit();
    s.strategy = Some(Strategy::GeneratorCritic);
    let task_id = s.task_id.clone();
    let engine = f.engine.clone();
    let cancel = CancellationToken::new();
    let worker = cancel.clone();
    let pending = tokio::spawn(async move { engine.run(s, worker).await });
    loop {
        tokio::time::timeout(std::time::Duration::from_secs(3), f.fake.started.notified())
            .await
            .unwrap();
        if f.fake.requests.lock().unwrap().len() == 2 {
            break;
        }
    }
    let records = f.engine.evaluations(&task_id).await.unwrap();
    assert_eq!(records.len(), 1);
    let fb = Feedback {
        feedback_id: id(),
        task_id: task_id.clone(),
        artifact_id: records[0].artifact.artifact_id.clone(),
        kind: FeedbackKind::BusinessAcceptance,
        evaluator_version: "1".into(),
        accepted: false,
        reason: "cancelled".into(),
    };
    assert_eq!(
        f.engine.record_feedback(&fb).await.unwrap_err().kind,
        "feedback"
    );
    cancel.cancel();
    assert_eq!(pending.await.unwrap().unwrap_err().kind, "cancelled");
    let metrics = f.engine.metrics().await.unwrap();
    assert_eq!(metrics.calls.iter().map(|c| c.cancelled).sum::<u64>(), 1);
    assert_eq!(metrics.tasks.cancelled, 1);
    assert!(f.engine.evaluations(&task_id).await.unwrap()[0]
        .critic
        .is_none());
    f.engine.record_feedback(&fb).await.unwrap();
    f.engine.close().await.unwrap();
}

#[tokio::test]
async fn live_feedback_changes_selection_but_model_versions_do_not_mix() {
    let f = setup(vec![response("{}", true), response("{}", true)], |c| {
        c.weights.quality = 1.0;
        c.weights.capability = 0.0;
        c.weights.reliability = 0.0;
        c.weights.cost = 0.0;
        c.weights.latency = 0.0;
        c.weights.uncertainty = 0.0;
        let mut other = c.models[0].clone();
        other.id = "other".into();
        c.models.push(other);
    })
    .await;
    let first = f
        .engine
        .run(explicit(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(f.fake.requests.lock().unwrap()[0]["model"], "local");
    f.engine.record_feedback(&feedback(&first)).await.unwrap();
    f.engine
        .run(explicit(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(f.fake.requests.lock().unwrap()[1]["model"], "other");
    let metrics = f.engine.metrics().await.unwrap();
    let mut config = f.config.clone();
    config.models[0].version = "new-version".into();
    let mut plan: EffectivePlan =
        serde_json::from_value(saved(&f, &first.task_id).1["effective_plan"].clone()).unwrap();
    let snapshot = model_collaboration_engine::router::RoutingSnapshot::capture_with_metrics(
        &config,
        &mut plan,
        now_ms(),
        &metrics,
    );
    assert!(snapshot.nodes["invoke"].qualities["local"]
        .evidence
        .is_none());
    f.engine.close().await.unwrap();
}

#[tokio::test]
async fn failed_model_attempt_keeps_metrics_and_reservation() {
    let f = setup(
        vec![Action::Reply(Err(EngineError::new(
            "model",
            "network failed",
        )))],
        |_| {},
    )
    .await;
    let mut s = explicit();
    s.max_attempts = 1;
    let task_id = s.task_id.clone();
    assert!(f.engine.run(s, CancellationToken::new()).await.is_err());
    let metrics = f.engine.metrics().await.unwrap();
    assert_eq!(metrics.tasks.failed, 1);
    assert_eq!(metrics.calls[0].failed, 1);
    assert_eq!(
        metrics.calls[0].unresolved_reserved,
        ledger(&f, &task_id).await.reserved
    );
    assert!(metrics.quality.is_empty());
    assert!(f.engine.evaluations(&task_id).await.unwrap().is_empty());
    f.engine.close().await.unwrap();
}

#[tokio::test]
async fn zero_reservation_does_not_hide_unknown_usage() {
    let f = setup(vec![response("{}", false)], |config| {
        config.models[0].input_price = 0;
        config.models[0].output_price = 0;
    })
    .await;
    let result = f
        .engine
        .run(explicit(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.reserved_cost, 0);
    let metrics = f.engine.metrics().await.unwrap();
    assert_eq!(metrics.calls[0].unknown_cost_attempts, 1);
    assert_eq!(metrics.calls[0].unresolved_reserved, 0);
    f.engine.close().await.unwrap();
}
