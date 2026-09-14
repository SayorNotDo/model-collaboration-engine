use async_trait::async_trait;
use model_collaboration_engine::{
    adapter::{InvokeRequest, ModelAdapter, ModelOutput},
    contracts::*,
    engine::Engine,
    store::{SqliteStore, Store},
};
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

struct Fake {
    outputs: Mutex<VecDeque<Result<ModelOutput>>>,
    models: Mutex<Vec<String>>,
    block: bool,
}
#[async_trait]
impl ModelAdapter for Fake {
    async fn invoke(&self, request: InvokeRequest) -> Result<ModelOutput> {
        self.models.lock().unwrap().push(request.model.id);
        if self.block {
            std::future::pending::<()>().await;
        }
        self.outputs
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected invocation")
    }
}
fn output(text: &str, usage: bool) -> Result<ModelOutput> {
    Ok(ModelOutput {
        text: text.into(),
        tool_calls: vec![],
        usage: usage.then_some(Usage {
            input_tokens: 10,
            output_tokens: 5,
        }),
        request_id: None,
        complete: true,
    })
}
fn config(path: &str) -> Config {
    serde_json::from_value(json!({
        "database_path":path,"models":[
            {"id":"cheap","model":"fake","version":"1","endpoint":"chat_completions","base_url":"http://localhost/v1","api_key_env":"UNUSED","provider":"test","region":"local","local":true,"capabilities":["text","json"],"context_tokens":100000,"input_price":1,"output_price":1,"price_version":"1","acceptance":0.7,"reliability":1.0,"latency_ms":1,"uncertainty":0.0},
            {"id":"strong","model":"fake","version":"1","endpoint":"responses","base_url":"http://localhost/v1","api_key_env":"UNUSED","provider":"test","region":"local","local":true,"capabilities":["text","json"],"context_tokens":100000,"input_price":2,"output_price":2,"price_version":"1","acceptance":0.9,"reliability":1.0,"latency_ms":1,"uncertainty":0.0}
        ],"weights":{"quality":0.0,"capability":0.0,"reliability":0.0,"cost":1.0,"latency":0.0,"uncertainty":0.0,"version":"1"},
        "max_concurrency":1,"event_capacity":8,"event_max_bytes":4096,"stream_frame_max_bytes":4096,"context_max_bytes":65536,"close_grace_ms":100,"cleanup_timeout_ms":100
    })).unwrap()
}
fn task(strategy: Strategy) -> TaskSpec {
    TaskSpec {
        task_id: id(),
        goal: "Say accepted".into(),
        evidence: vec![],
        strategy,
        acceptance: Acceptance {
            version: "1".into(),
            nonempty: true,
            required_substrings: vec!["accepted".into()],
            json_object: false,
        },
        constraints: Constraints {
            different_critic: true,
            ..Default::default()
        },
        budget: 100000,
        deadline_ms: now_ms() + 10000,
        output_tokens: 100,
        max_calls: 8,
        max_rounds: 3,
        max_attempts: 2,
        max_call_cost: 30000,
        finalization_ms: 100,
        tools: vec![],
    }
}
async fn setup(
    outputs: Vec<Result<ModelOutput>>,
    block: bool,
) -> (tempfile::TempDir, Engine, Arc<SqliteStore>, Arc<Fake>) {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path().join("engine.db").to_str().unwrap());
    let store = Arc::new(SqliteStore::open(&config.database_path).await.unwrap());
    let fake = Arc::new(Fake {
        outputs: Mutex::new(outputs.into()),
        models: Mutex::new(vec![]),
        block,
    });
    let engine = Engine::with_components(config, store.clone(), fake.clone()).unwrap();
    (dir, engine, store, fake)
}

#[tokio::test]
async fn single_settles_and_rejects_duplicate_id() {
    let (_dir, engine, store, fake) = setup(vec![output("accepted", true)], false).await;
    let task = task(Strategy::Single);
    let result = engine
        .run(task.clone(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.status, "completed");
    assert_eq!(result.settled_cost, 15);
    assert_eq!(result.reserved_cost, 0);
    assert_eq!(store.ledger(&task.task_id).await.unwrap().calls, 1);
    assert!(engine.run(task, CancellationToken::new()).await.is_err());
    assert_eq!(fake.models.lock().unwrap().len(), 1);
    engine.close().await.unwrap();
}

#[tokio::test]
async fn cascade_upgrades_after_failed_acceptance() {
    let (_dir, engine, _, fake) =
        setup(vec![output("wrong", true), output("accepted", true)], false).await;
    let result = engine
        .run(task(Strategy::Cascade), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.status, "completed");
    assert_eq!(result.artifact.version, 2);
    assert_eq!(*fake.models.lock().unwrap(), vec!["cheap", "strong"]);
    assert_eq!(result.settled_cost, 45);
    engine.close().await.unwrap();
}

#[tokio::test]
async fn critic_requires_revision_and_uses_other_model() {
    let revise = json!({"status":"revise","checks":[],"evidence":[],"defects":["add detail"],"action":"revise"}).to_string();
    let pass = json!({"status":"pass","checks":[],"evidence":[],"defects":[],"action":"accept"})
        .to_string();
    let (_dir, engine, _, fake) = setup(
        vec![
            output("accepted", true),
            output(&revise, true),
            output("accepted with detail", true),
            output(&pass, true),
        ],
        false,
    )
    .await;
    let result = engine
        .run(task(Strategy::GeneratorCritic), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.status, "completed");
    assert_eq!(result.artifact.version, 2);
    assert_eq!(
        *fake.models.lock().unwrap(),
        vec!["cheap", "strong", "cheap", "strong"]
    );
    engine.close().await.unwrap();
}

#[tokio::test]
async fn missing_usage_remains_reserved() {
    let (_dir, engine, store, _) = setup(vec![output("accepted", false)], false).await;
    let result = engine
        .run(task(Strategy::Single), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.settled_cost, 0);
    assert!(result.reserved_cost > 0);
    assert_eq!(store.records().await.unwrap().len(), 1);
    engine.close().await.unwrap();
}

#[tokio::test]
async fn cancellation_keeps_unknown_cost_and_finishes_task() {
    let (_dir, engine, store, fake) = setup(vec![], true).await;
    let cancel = CancellationToken::new();
    let task = task(Strategy::Single);
    let run = engine.run(task.clone(), cancel.clone());
    let stop = async {
        while fake.models.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        cancel.cancel();
    };
    let (result, _) = tokio::join!(run, stop);
    assert_eq!(result.unwrap_err().kind, "cancelled");
    let records = store.records().await.unwrap();
    assert_eq!(records[0].status, "cancelled");
    assert!(records[0].ledger.reserved > 0);
    engine.close().await.unwrap();
}

#[tokio::test]
async fn budget_and_call_limit_prevent_additional_dispatch() {
    let (_dir, engine, _, fake) = setup(vec![output("wrong", true)], false).await;
    let mut task = task(Strategy::Cascade);
    task.max_calls = 1;
    assert_eq!(
        engine
            .run(task, CancellationToken::new())
            .await
            .unwrap_err()
            .kind,
        "budget"
    );
    assert_eq!(fake.models.lock().unwrap().len(), 1);
    engine.close().await.unwrap();
}

#[tokio::test]
async fn deadline_interrupts_a_blocked_adapter() {
    let (_dir, engine, store, _) = setup(vec![], true).await;
    let mut task = task(Strategy::Single);
    task.deadline_ms = now_ms() + 300;
    task.finalization_ms = 50;
    assert_eq!(
        engine
            .run(task, CancellationToken::new())
            .await
            .unwrap_err()
            .kind,
        "deadline"
    );
    assert!(store.records().await.unwrap()[0].ledger.reserved > 0);
    engine.close().await.unwrap();
}
