use super::{
    store_fault::{FaultStore, ReservationGate},
    support::*,
};
use model_collaboration_engine::{contracts::*, engine::Engine, store::Store};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

fn fallback_submission() -> SubmissionSpec {
    let mut sub = submission();
    sub.planning.fallback = Some(PlanChoice {
        task_type: TaskType::General,
        strategy: Strategy::Single,
    });
    sub
}

#[tokio::test]
async fn dispatched_planning_timeout_falls_back_and_retains_unknown_cost() {
    let f = setup(vec![Action::Block, response("{}", true)], |_| {}).await;
    let sub = fallback_submission();
    let task_id = sub.task_id.clone();
    let engine = f.engine.clone();
    let run = tokio::spawn(async move { engine.run(sub, CancellationToken::new()).await });
    tokio::time::timeout(Duration::from_secs(5), f.fake.started.notified())
        .await
        .unwrap();
    assert_eq!(f.fake.requests.lock().unwrap()[0]["node"], "planner");
    // Advance only after the planner is dispatched. Resume before SQLite work can
    // cause automatic virtual-time jumps while waiting for its worker thread.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(6)).await;
    tokio::time::resume();
    let result = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.status, "completed");
    let records = f.store.records().await.unwrap();
    let planner = &records[0].attempts[0];
    assert_eq!(planner.state, "unresolved");
    assert_eq!(planner.cost, None);
    assert!(planner.amount > 0);
    assert_eq!(
        planner.outcome.as_ref().unwrap()["call_metrics"]["status"],
        "timed_out"
    );
    assert_eq!(ledger(&f, &task_id).await.calls, 2);
    assert_eq!(f.fake.requests.lock().unwrap()[1]["node"], "invoke");
    f.engine.close().await.unwrap();
}

#[tokio::test]
async fn undispatched_timeout_settles_zero_and_global_deadline_never_falls_back() {
    for global in [false, true] {
        let f = setup(vec![response("{}", true)], |_| {}).await;
        let gate = Arc::new(ReservationGate::default());
        let engine = Arc::new(
            Engine::with_components(
                f.config.clone(),
                Arc::new(FaultStore {
                    inner: f.store.clone(),
                    fail_settlement: false,
                    reservation_gate: Some(gate.clone()),
                }),
                f.fake.clone(),
            )
            .unwrap(),
        );
        let mut sub = fallback_submission();
        sub.planning.timeout_ms = 100;
        if global {
            sub.deadline_ms = now_ms() + 1500;
            sub.finalization_ms = 10;
        }
        let task_id = sub.task_id.clone();
        let global_deadline = sub.deadline_ms - sub.finalization_ms;
        let worker = engine.clone();
        let run = tokio::spawn(async move { worker.run(sub, CancellationToken::new()).await });
        tokio::time::timeout(Duration::from_secs(5), gate.reached.notified())
            .await
            .unwrap();
        assert!(f.fake.requests.lock().unwrap().is_empty());
        // Hold the committed reservation until the selected deadline has actually
        // expired. This waits for a boundary, never guesses whether dispatch ran.
        let expiry = if global {
            global_deadline
        } else {
            now_ms() + 101
        };
        while now_ms() <= expiry {
            tokio::time::sleep(Duration::from_millis(expiry.saturating_sub(now_ms()) + 1)).await;
        }
        gate.release.notify_one();
        let result = tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .unwrap()
            .unwrap();
        if global {
            assert_eq!(result.unwrap_err().kind, "deadline");
            assert!(f.fake.requests.lock().unwrap().is_empty());
        } else {
            assert_eq!(result.unwrap().status, "completed");
            assert_eq!(f.fake.requests.lock().unwrap()[0]["node"], "invoke");
        }
        let conn = rusqlite::Connection::open(&f.config.database_path).unwrap();
        let (cost, outcome): (i64, String) = conn
            .query_row(
                "SELECT cost,outcome FROM attempts WHERE task=? ORDER BY rowid LIMIT 1",
                [&task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(cost, 0);
        let outcome: serde_json::Value = serde_json::from_str(&outcome).unwrap();
        assert_eq!(outcome["not_dispatched"], true);
        let ledger = ledger(&f, &task_id).await;
        assert_eq!(ledger.calls, if global { 1 } else { 2 });
        assert_eq!(ledger.reserved, 0);
        engine.close().await.unwrap();
    }
}
