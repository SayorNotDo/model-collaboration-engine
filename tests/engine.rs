use async_trait::async_trait;
use model_collaboration_engine::{
    adapter::{InvokeRequest, Message, ModelAdapter, ModelOutput},
    contracts::*,
    engine::Engine,
    store::{SqliteStore, Store},
    tools::{ToolExecutor, ToolRequest},
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
    messages: Mutex<Vec<Vec<Message>>>,
    block: bool,
}
#[async_trait]
impl ModelAdapter for Fake {
    async fn invoke(&self, request: InvokeRequest) -> Result<ModelOutput> {
        self.models.lock().unwrap().push(request.model.id);
        self.messages.lock().unwrap().push(request.messages);
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
            {"id":"cheap","model":"fake","version":"1","endpoint":"chat_completions","base_url":"http://localhost/v1","api_key_env":"UNUSED","provider":"test","region":"local","local":true,"capabilities":["text","json","tools"],"context_tokens":100000,"input_price":1,"output_price":1,"price_version":"1","acceptance":0.7,"reliability":1.0,"latency_ms":1,"uncertainty":0.0},
            {"id":"strong","model":"fake","version":"1","endpoint":"responses","base_url":"http://localhost/v1","api_key_env":"UNUSED","provider":"test","region":"local","local":true,"capabilities":["text","json","tools"],"context_tokens":100000,"input_price":2,"output_price":2,"price_version":"1","acceptance":0.9,"reliability":1.0,"latency_ms":1,"uncertainty":0.0}
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
        selection: SelectionPolicy {
            version: "host-task-fit-v1".into(),
            min_quality: 0.0,
            target_quality: 0.9,
            above_target_factor: 0.1,
            cost_reference: 10_000,
            latency_reference_ms: 5_000,
            min_upgrade_gain: 0.05,
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
    setup_adjusted(outputs, block, |_| {}).await
}

async fn setup_adjusted(
    outputs: Vec<Result<ModelOutput>>,
    block: bool,
    adjust: impl FnOnce(&mut Config),
) -> (tempfile::TempDir, Engine, Arc<SqliteStore>, Arc<Fake>) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(dir.path().join("engine.db").to_str().unwrap());
    adjust(&mut config);
    let store = Arc::new(SqliteStore::open(&config.database_path).await.unwrap());
    let fake = Arc::new(Fake {
        outputs: Mutex::new(outputs.into()),
        models: Mutex::new(vec![]),
        messages: Mutex::new(vec![]),
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
    let (dir, engine, _, fake) =
        setup(vec![output("wrong", true), output("accepted", true)], false).await;
    let result = engine
        .run(task(Strategy::Cascade), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.status, "completed");
    assert_eq!(result.artifact.version, 2);
    assert_eq!(*fake.models.lock().unwrap(), vec!["cheap", "strong"]);
    assert_eq!(result.settled_cost, 45);
    let connection = rusqlite::Connection::open(dir.path().join("engine.db")).unwrap();
    let metadata: String = connection
        .query_row(
            "SELECT metadata FROM attempts WHERE task=? ORDER BY rowid DESC LIMIT 1",
            [&result.task_id],
            |row| row.get(0),
        )
        .unwrap();
    let metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
    assert_eq!(
        metadata["route_inputs"]["upgrade"],
        json!({"previous_quality": 0.7, "required_quality": 0.75})
    );
    engine.close().await.unwrap();
}

#[tokio::test]
async fn initial_route_records_the_host_minimum_quality_floor() {
    let (dir, engine, _store, fake) = setup(vec![output("accepted", true)], false).await;
    let mut task = task(Strategy::Single);
    task.selection.min_quality = 0.8;
    let result = engine.run(task, CancellationToken::new()).await.unwrap();
    assert_eq!(*fake.models.lock().unwrap(), vec!["strong"]);
    let connection = rusqlite::Connection::open(dir.path().join("engine.db")).unwrap();
    let metadata: String = connection
        .query_row(
            "SELECT metadata FROM attempts WHERE task=?",
            [&result.task_id],
            |row| row.get(0),
        )
        .unwrap();
    let metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
    assert_eq!(metadata["route_inputs"]["quality_floor"], 0.8);
    engine.close().await.unwrap();
}

#[tokio::test]
async fn cascade_stops_when_no_candidate_meets_minimum_quality_gain() {
    let (dir, engine, store, fake) = setup_adjusted(vec![output("wrong", true)], false, |config| {
        config.models[0].acceptance = 0.70;
        config.models[1].acceptance = 0.72;
    })
    .await;
    let result = engine
        .run(task(Strategy::Cascade), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.status, "human_required");
    assert_eq!(result.artifact.text, "wrong");
    assert_eq!(*fake.models.lock().unwrap(), vec!["cheap"]);
    assert_eq!(store.ledger(&result.task_id).await.unwrap().calls, 1);
    let connection = rusqlite::Connection::open(dir.path().join("engine.db")).unwrap();
    let checkpoint: String = connection
        .query_row(
            "SELECT checkpoint FROM tasks WHERE id=?",
            [&result.task_id],
            |row| row.get(0),
        )
        .unwrap();
    let checkpoint: serde_json::Value = serde_json::from_str(&checkpoint).unwrap();
    assert_eq!(
        checkpoint["upgrade_stop"]["reason"],
        "no_quality_improvement"
    );
    assert_eq!(checkpoint["upgrade_stop"]["required_quality"], 0.75);
    assert_eq!(
        checkpoint["upgrade_stop"]["requirement"],
        json!({"previous_quality": 0.7, "required_quality": 0.75})
    );
    engine.close().await.unwrap();
}

#[tokio::test]
async fn cascade_records_budget_when_only_improving_candidate_is_unaffordable() {
    let (dir, engine, store, fake) = setup_adjusted(vec![output("wrong", true)], false, |config| {
        config.models[1].input_price = 1_000_000;
        config.models[1].output_price = 1_000_000;
    })
    .await;
    let result = engine
        .run(task(Strategy::Cascade), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.status, "human_required");
    assert_eq!(*fake.models.lock().unwrap(), vec!["cheap"]);
    assert_eq!(store.ledger(&result.task_id).await.unwrap().calls, 1);
    let connection = rusqlite::Connection::open(dir.path().join("engine.db")).unwrap();
    let checkpoint: String = connection
        .query_row(
            "SELECT checkpoint FROM tasks WHERE id=?",
            [&result.task_id],
            |row| row.get(0),
        )
        .unwrap();
    let checkpoint: serde_json::Value = serde_json::from_str(&checkpoint).unwrap();
    assert_eq!(checkpoint["upgrade_stop"]["reason"], "budget");
    engine.close().await.unwrap();
}

#[tokio::test]
async fn cascade_prefers_quality_reason_when_budget_cannot_enable_candidate() {
    let (dir, engine, _, fake) = setup_adjusted(vec![output("wrong", true)], false, |config| {
        config.models[1].acceptance = 0.72;
        config.models[1].input_price = 1_000_000;
        config.models[1].output_price = 1_000_000;
    })
    .await;
    let result = engine
        .run(task(Strategy::Cascade), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(*fake.models.lock().unwrap(), vec!["cheap"]);
    let connection = rusqlite::Connection::open(dir.path().join("engine.db")).unwrap();
    let checkpoint: String = connection
        .query_row(
            "SELECT checkpoint FROM tasks WHERE id=?",
            [&result.task_id],
            |row| row.get(0),
        )
        .unwrap();
    let checkpoint: serde_json::Value = serde_json::from_str(&checkpoint).unwrap();
    assert_eq!(
        checkpoint["upgrade_stop"]["reason"],
        "no_quality_improvement"
    );
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

#[tokio::test(start_paused = true)]
async fn deadline_interrupts_a_blocked_adapter() {
    let (_dir, engine, store, _) = setup(vec![], true).await;
    let mut task = task(Strategy::Single);
    task.deadline_ms = now_ms() + 10_000;
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

struct Host {
    requests: Mutex<Vec<ToolRequest>>,
    cost: Option<u64>,
    fail: bool,
    block: bool,
}
#[async_trait]
impl ToolExecutor for Host {
    async fn execute(&self, request: ToolRequest) -> Result<ToolResult> {
        self.requests.lock().unwrap().push(request);
        if self.block {
            std::future::pending::<()>().await;
        }
        if self.fail {
            return Err(EngineError::new("tool", "host rejected request"));
        }
        Ok(ToolResult {
            output: "lookup result".into(),
            actual_cost: self.cost,
        })
    }
}
fn host(cost: Option<u64>, fail: bool, block: bool) -> Arc<Host> {
    Arc::new(Host {
        requests: Mutex::new(vec![]),
        cost,
        fail,
        block,
    })
}
fn tool_task() -> TaskSpec {
    let mut task = task(Strategy::Single);
    task.tools = vec![ToolSpec {
        name: "lookup".into(),
        description: "lookup".into(),
        parameters: json!({"type":"object"}),
        max_cost: 50,
    }];
    task
}
fn tool_output(names: &[&str]) -> Result<ModelOutput> {
    let mut output = output("", true).unwrap();
    output.tool_calls = names
        .iter()
        .enumerate()
        .map(|(i, name)| ToolCall {
            id: format!("call-{i}"),
            name: (*name).into(),
            arguments: json!({"query":"x"}),
        })
        .collect();
    Ok(output)
}

#[tokio::test]
async fn tool_roundtrip_is_accounted_and_observable() {
    let (_dir, engine, store, fake) = setup(
        vec![tool_output(&["lookup"]), output("accepted", true)],
        false,
    )
    .await;
    let host = host(Some(7), false, false);
    let task = tool_task();
    let (sender, mut receiver) = engine.event_channel();
    let collect = async {
        let mut events = vec![];
        while let Some(event) = receiver.recv().await {
            events.push(event);
        }
        events
    };
    let (result, events) = tokio::join!(
        engine.run_with_host(
            task.clone(),
            CancellationToken::new(),
            Some(host.clone()),
            Some(sender)
        ),
        collect
    );
    let result = result.unwrap();
    let metrics = engine.metrics().await.unwrap();
    let tool = metrics
        .calls
        .iter()
        .find(|c| c.attempt_kind == "tool")
        .unwrap();
    assert_eq!(tool.tool_name.as_deref(), Some("lookup"));
    assert_eq!((tool.succeeded, tool.known_cost), (1, 7));
    assert_eq!(metrics.calls.iter().map(|c| c.attempts).sum::<u64>(), 3);
    assert_eq!(result.settled_cost, 37);
    assert_eq!(result.reserved_cost, 0);
    assert_eq!(store.ledger(&task.task_id).await.unwrap().calls, 3);
    let messages = fake.messages.lock().unwrap()[1].clone();
    assert_eq!(messages[2].tool_calls[0].id, "call-0");
    assert_eq!(messages[3].role, "tool");
    assert_eq!(messages[3].content, "lookup result");
    assert_eq!(messages[3].tool_call_id.as_deref(), Some("call-0"));
    assert_eq!(host.requests.lock().unwrap()[0].max_cost, 50);
    assert!(events.iter().any(|e| e.kind == "tool_requested"));
    assert!(events.iter().any(|e| e.kind == "tool_completed"));
    assert!(events.windows(2).all(|w| w[0].sequence < w[1].sequence));
    engine.close().await.unwrap();
}

#[tokio::test]
async fn whole_tool_batch_is_validated_before_side_effects() {
    let (_dir, engine, _, _) = setup(vec![tool_output(&["lookup", "not_declared"])], false).await;
    let host = host(Some(7), false, false);
    let result = engine
        .run_with_host(
            tool_task(),
            CancellationToken::new(),
            Some(host.clone()),
            None,
        )
        .await;
    assert_eq!(result.unwrap_err().kind, "protocol");
    assert!(host.requests.lock().unwrap().is_empty());
    engine.close().await.unwrap();
}

#[tokio::test]
async fn tool_calls_obey_total_call_limit() {
    let (_dir, engine, _, fake) = setup(vec![tool_output(&["lookup"])], false).await;
    let host = host(Some(7), false, false);
    let mut task = tool_task();
    task.max_calls = 1;
    let result = engine
        .run_with_host(task, CancellationToken::new(), Some(host.clone()), None)
        .await;
    assert_eq!(result.unwrap_err().kind, "budget");
    assert!(host.requests.lock().unwrap().is_empty());
    assert_eq!(fake.models.lock().unwrap().len(), 1);
    engine.close().await.unwrap();
}

#[tokio::test]
async fn failed_tool_is_not_retried_and_retains_reservation() {
    let (_dir, engine, store, fake) = setup(vec![tool_output(&["lookup"])], false).await;
    let host = host(None, true, false);
    let task = tool_task();
    let result = engine
        .run_with_host(
            task.clone(),
            CancellationToken::new(),
            Some(host.clone()),
            None,
        )
        .await;
    assert_eq!(result.unwrap_err().kind, "tool");
    assert_eq!(host.requests.lock().unwrap().len(), 1);
    assert_eq!(fake.models.lock().unwrap().len(), 1);
    assert_eq!(store.ledger(&task.task_id).await.unwrap().reserved, 50);
    engine.close().await.unwrap();
}

#[tokio::test]
async fn tool_overrun_records_actual_cost_before_failing() {
    let (_dir, engine, store, _) = setup(vec![tool_output(&["lookup"])], false).await;
    let task = tool_task();
    let result = engine
        .run_with_host(
            task.clone(),
            CancellationToken::new(),
            Some(host(Some(75), false, false)),
            None,
        )
        .await;
    assert_eq!(result.unwrap_err().kind, "budget");
    let ledger = store.ledger(&task.task_id).await.unwrap();
    assert_eq!(ledger.settled, 90);
    assert_eq!(ledger.reserved, 0);
    engine.close().await.unwrap();
}

#[tokio::test]
async fn cancellation_during_tool_retains_unknown_cost() {
    let (_dir, engine, store, _) = setup(vec![tool_output(&["lookup"])], false).await;
    let host = host(None, false, true);
    let cancel = CancellationToken::new();
    let task = tool_task();
    let stop = async {
        while host.requests.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        cancel.cancel();
    };
    let (result, _) = tokio::join!(
        engine.run_with_host(task.clone(), cancel.clone(), Some(host.clone()), None),
        stop
    );
    assert_eq!(result.unwrap_err().kind, "cancelled");
    assert_eq!(store.ledger(&task.task_id).await.unwrap().reserved, 50);
    engine.close().await.unwrap();
}

#[tokio::test]
async fn full_event_queue_obeys_deadline_and_closed_receiver_does_not_block() {
    let (_dir, engine, _, fake) = setup(vec![output("accepted", true)], false).await;
    let (sender, _receiver) = tokio::sync::mpsc::channel(1);
    let mut task = task(Strategy::Single);
    task.deadline_ms = now_ms() + 300;
    task.finalization_ms = 50;
    let result = engine
        .run_with_host(task, CancellationToken::new(), None, Some(sender))
        .await;
    assert_eq!(result.unwrap_err().kind, "deadline");
    assert!(fake.models.lock().unwrap().is_empty());
    let (sender, receiver) = engine.event_channel();
    drop(receiver);
    assert!(engine
        .run_with_host(
            self::task(Strategy::Single),
            CancellationToken::new(),
            None,
            Some(sender)
        )
        .await
        .is_ok());
    engine.close().await.unwrap();
}

#[tokio::test]
async fn critic_cannot_execute_host_tools() {
    let (_dir, engine, _, _) = setup(
        vec![output("accepted", true), tool_output(&["lookup"])],
        false,
    )
    .await;
    let host = host(Some(0), false, false);
    let mut task = tool_task();
    task.strategy = Strategy::GeneratorCritic;
    let result = engine
        .run_with_host(task, CancellationToken::new(), Some(host.clone()), None)
        .await;
    assert_eq!(result.unwrap_err().kind, "protocol");
    assert!(host.requests.lock().unwrap().is_empty());
    engine.close().await.unwrap();
}

#[tokio::test]
async fn unknown_tool_cost_is_kept_after_success() {
    let (_dir, engine, _, _) = setup(
        vec![tool_output(&["lookup"]), output("accepted", true)],
        false,
    )
    .await;
    let result = engine
        .run_with_host(
            tool_task(),
            CancellationToken::new(),
            Some(host(None, false, false)),
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.status, "completed");
    assert_eq!(result.settled_cost, 30);
    assert_eq!(result.reserved_cost, 50);
    engine.close().await.unwrap();
}

#[tokio::test]
async fn close_cancels_active_model_and_releases_database_after_accounting() {
    let (dir, engine, _, fake) = setup_adjusted(vec![], true, |config| {
        // This scenario verifies cooperative cancellation and accounting, not the
        // separate cleanup-timeout path. Leave enough wall time under suite load.
        config.cleanup_timeout_ms = 1_000;
    })
    .await;
    let caller = CancellationToken::new();
    let spec = task(Strategy::Single);
    let shutdown = async {
        while fake.models.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        engine.close().await.unwrap();
    };
    let (result, ()) = tokio::join!(engine.run(spec, caller.clone()), shutdown);
    assert_eq!(result.unwrap_err().kind, "cancelled");
    assert!(!caller.is_cancelled());
    let reopened = SqliteStore::open(dir.path().join("engine.db").to_str().unwrap())
        .await
        .unwrap();
    let records = reopened.records().await.unwrap();
    assert_eq!(records[0].status, "cancelled");
    assert!(records[0].ledger.reserved > 0);
    reopened.close().await.unwrap();
    engine.close().await.unwrap();
    assert_eq!(
        engine
            .run(task(Strategy::Single), CancellationToken::new())
            .await
            .unwrap_err()
            .kind,
        "closed"
    );
}

struct GatedAdapter {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait]
impl ModelAdapter for GatedAdapter {
    async fn invoke(&self, _: InvokeRequest) -> Result<ModelOutput> {
        self.entered.notify_one();
        self.release.notified().await;
        output("accepted", true)
    }
}

#[tokio::test]
async fn close_allows_graceful_completion_and_rejects_queued_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path().join("engine.db").to_str().unwrap());
    cfg.close_grace_ms = 1000;
    let store = Arc::new(SqliteStore::open(&cfg.database_path).await.unwrap());
    let adapter = Arc::new(GatedAdapter {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let engine = Arc::new(Engine::with_components(cfg, store, adapter.clone()).unwrap());
    let running = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .run(task(Strategy::Single), CancellationToken::new())
                .await
        })
    };
    adapter.entered.notified().await;
    let queued = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .run(task(Strategy::Single), CancellationToken::new())
                .await
        })
    };
    tokio::task::yield_now().await;
    let closing = {
        let engine = engine.clone();
        tokio::spawn(async move { engine.close().await })
    };
    assert_eq!(queued.await.unwrap().unwrap_err().kind, "closed");
    assert!(!closing.is_finished());
    adapter.release.notify_one();
    assert_eq!(running.await.unwrap().unwrap().status, "completed");
    closing.await.unwrap().unwrap();
}

struct SlowFinish {
    inner: Arc<SqliteStore>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait]
impl Store for SlowFinish {
    async fn record_evaluation(&self, record: &EvaluationRecord) -> Result<()> {
        self.inner.record_evaluation(record).await
    }
    async fn record_feedback(&self, feedback: &Feedback) -> Result<()> {
        self.inner.record_feedback(feedback).await
    }
    async fn evaluations(&self, task: &str) -> Result<Vec<EvaluationRecord>> {
        self.inner.evaluations(task).await
    }
    async fn metrics(&self) -> Result<MetricsSnapshot> {
        self.inner.metrics().await
    }

    async fn create_submission(&self, submission: &SubmissionSpec, hash: &str) -> Result<()> {
        self.inner.create_submission(submission, hash).await
    }
    async fn save_plan(
        &self,
        task: &str,
        plan: serde_json::Value,
        checkpoint: serde_json::Value,
    ) -> Result<()> {
        self.inner.save_plan(task, plan, checkpoint).await
    }
    async fn reserve(&self, t: &str, a: &str, n: u64, m: u32, v: serde_json::Value) -> Result<()> {
        self.inner.reserve(t, a, n, m, v).await
    }
    async fn settle(&self, t: &str, a: &str, c: Option<u64>, v: serde_json::Value) -> Result<()> {
        self.inner.settle(t, a, c, v).await
    }
    async fn checkpoint(&self, t: &str, v: serde_json::Value) -> Result<()> {
        self.inner.checkpoint(t, v).await
    }
    async fn event(&self, e: &model_collaboration_engine::events::Event) -> Result<()> {
        self.inner.event(e).await
    }
    async fn finish(&self, t: &str, s: &str, v: serde_json::Value) -> Result<()> {
        self.entered.notify_one();
        self.release.notified().await;
        self.inner.finish(t, s, v).await
    }
    async fn ledger(&self, t: &str) -> Result<model_collaboration_engine::store::Ledger> {
        self.inner.ledger(t).await
    }
    async fn records(&self) -> Result<Vec<model_collaboration_engine::store::RecoveryRecord>> {
        self.inner.records().await
    }
    async fn reconcile(&self, t: &str, a: &str, c: u64, e: &str) -> Result<()> {
        self.inner.reconcile(t, a, c, e).await
    }
    async fn close(&self) -> Result<()> {
        self.inner.close().await
    }
}

#[tokio::test]
async fn cleanup_timeout_retains_ownership_until_retry() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path().join("engine.db").to_str().unwrap());
    let path = cfg.database_path.clone();
    let store = Arc::new(SlowFinish {
        inner: Arc::new(SqliteStore::open(&path).await.unwrap()),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let fake = Arc::new(Fake {
        outputs: Mutex::new(vec![output("accepted", true)].into()),
        models: Mutex::new(vec![]),
        messages: Mutex::new(vec![]),
        block: false,
    });
    let engine = Arc::new(Engine::with_components(cfg, store.clone(), fake).unwrap());
    let running = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .run(task(Strategy::Single), CancellationToken::new())
                .await
        })
    };
    store.entered.notified().await;
    assert_eq!(engine.close().await.unwrap_err().kind, "cleanup_timeout");
    assert!(matches!(SqliteStore::open(&path).await,Err(e) if e.kind=="database_busy"));
    store.release.notify_one();
    running.await.unwrap().unwrap();
    let (a, b) = tokio::join!(engine.close(), engine.close());
    a.unwrap();
    b.unwrap();
    SqliteStore::open(&path)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
}

#[tokio::test]
async fn recovery_inspection_preserves_attempt_evidence_across_reopen() {
    let (dir, engine, store, fake) = setup(vec![], false).await;
    let mut pending = task(Strategy::Single);
    pending.task_id = "pending".into();
    let mut unresolved = pending.clone();
    unresolved.task_id = "zero-cost".into();
    let mut completed = pending.clone();
    completed.task_id = "completed".into();
    for task in [&pending, &unresolved, &completed] {
        store
            .create_submission(&task.clone().into(), "original-config")
            .await
            .unwrap();
    }
    store
        .reserve(
            &pending.task_id,
            "model-attempt",
            20,
            8,
            json!({"kind":"model","model":"cheap"}),
        )
        .await
        .unwrap();
    store
        .reserve(
            &pending.task_id,
            "tool-attempt",
            5,
            8,
            json!({"kind":"tool","name":"lookup"}),
        )
        .await
        .unwrap();
    store
        .settle(
            &pending.task_id,
            "tool-attempt",
            Some(3),
            json!({"output":"known"}),
        )
        .await
        .unwrap();
    store
        .reserve(
            &unresolved.task_id,
            "zero-attempt",
            0,
            8,
            json!({"kind":"tool"}),
        )
        .await
        .unwrap();
    store
        .settle(
            &unresolved.task_id,
            "zero-attempt",
            None,
            json!({"error":"cancelled"}),
        )
        .await
        .unwrap();
    store
        .finish(
            &unresolved.task_id,
            "cancelled",
            json!({"status":"cancelled"}),
        )
        .await
        .unwrap();
    store
        .finish(&completed.task_id, "completed", json!({}))
        .await
        .unwrap();
    let before = engine.recovery_records().await.unwrap();
    assert_eq!(before.len(), 2);
    assert_eq!(before[0].task.task_id, "pending");
    assert_eq!(before[0].config_hash, "original-config");
    assert_eq!(
        before[0].checkpoint,
        json!({"version":1,"phase":"admitted"})
    );
    assert!(before[0].result.is_none());
    assert_eq!(before[0].ledger.reserved, 20);
    assert_eq!(before[0].ledger.settled, 3);
    assert_eq!(before[0].ledger.calls, 2);
    assert_eq!(before[0].attempts[0].attempt_id, "model-attempt");
    assert_eq!(before[0].attempts[0].state, "pending");
    assert_eq!(before[0].attempts[0].cost, None);
    assert_eq!(before[0].attempts[1].cost, Some(3));
    assert_eq!(before[0].attempts[1].metadata["name"], "lookup");
    assert_eq!(
        before[0].attempts[1].outcome,
        Some(json!({"output":"known"}))
    );
    assert_eq!(before[1].ledger.reserved, 0);
    assert_eq!(before[1].attempts[0].state, "unresolved");
    assert_eq!(before[1].result, Some(json!({"status":"cancelled"})));
    assert!(fake.models.lock().unwrap().is_empty());
    engine.close().await.unwrap();
    assert_eq!(engine.recovery_records().await.unwrap_err().kind, "closed");
    let reopened = SqliteStore::open(dir.path().join("engine.db").to_str().unwrap())
        .await
        .unwrap();
    let after = reopened.records().await.unwrap();
    assert_eq!(
        serde_json::to_value(&before).unwrap(),
        serde_json::to_value(&after).unwrap()
    );
    reopened
        .reconcile("zero-cost", "zero-attempt", 0, "host verified no charge")
        .await
        .unwrap();
    assert_eq!(reopened.records().await.unwrap().len(), 1);
    reopened.close().await.unwrap();
}
