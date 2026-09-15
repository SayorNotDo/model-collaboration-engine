use model_collaboration_engine::{
    contracts::*,
    store::{SqliteStore, Store},
};
use serde_json::json;

#[tokio::test]
async fn reconciliation_preserves_outcome_metrics_and_is_idempotent() {
    for (status, cost) in [("succeeded", 15), ("cancelled", 0), ("timed_out", 150)] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("engine.db");
        let store = SqliteStore::open(path.to_str().unwrap()).await.unwrap();
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../examples/task.json")).unwrap();
        value["deadline_ms"] = json!(now_ms() + 30_000);
        let submission: SubmissionSpec = serde_json::from_value(value).unwrap();
        let task = &submission.task_id;
        store
            .create_submission(&submission, "test-config")
            .await
            .unwrap();
        store
            .reserve(
                task,
                "attempt",
                100,
                5,
                json!({"route":{"model_id":"local","model_version":"1","node_id":"invoke"}}),
            )
            .await
            .unwrap();
        let original = json!({
            "request_id":"provider-request", "planning_output":{"text":"evidence"},
            "error":{"kind":"test"}, "call_metrics":{"status":status,"latency_ms":42},
        });
        store
            .settle(task, "attempt", None, original.clone())
            .await
            .unwrap();
        let before = store.metrics().await.unwrap().calls.remove(0);
        let result = store
            .reconcile(task, "attempt", cost, "verified invoice")
            .await;
        if cost > 100 {
            assert_eq!(result.unwrap_err().kind, "budget");
        } else {
            result.unwrap();
        }
        store
            .reconcile(task, "attempt", cost, "verified invoice")
            .await
            .unwrap();
        assert_eq!(
            store
                .reconcile(task, "attempt", cost + 1, "conflict")
                .await
                .unwrap_err()
                .kind,
            "storage"
        );
        let after = store.metrics().await.unwrap().calls.remove(0);
        assert_eq!(after.succeeded, before.succeeded);
        assert_eq!(after.cancelled, before.cancelled);
        assert_eq!(after.timed_out, before.timed_out);
        assert_eq!(after.mean_latency_ms, Some(42.0));
        assert_eq!(after.latency_samples, 1);
        assert_eq!(after.known_cost, cost);
        assert_eq!(after.unknown_cost_attempts, 0);
        let ledger = store.ledger(task).await.unwrap();
        assert_eq!(
            (ledger.settled, ledger.reserved, ledger.calls),
            (cost, 0, 1)
        );
        store.close().await.unwrap();
        let reopened = SqliteStore::open(path.to_str().unwrap()).await.unwrap();
        let records = reopened.records().await.unwrap();
        let outcome = records[0].attempts[0].outcome.as_ref().unwrap();
        for (key, value) in original.as_object().unwrap() {
            assert_eq!(&outcome[key], value);
        }
        assert_eq!(outcome["reconciliation_evidence"], "verified invoice");
        reopened.close().await.unwrap();
    }
}
