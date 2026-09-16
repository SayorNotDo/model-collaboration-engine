use async_trait::async_trait;
use model_collaboration_engine::{
    adapter::{InvokeRequest, ModelAdapter, ModelOutput},
    contracts::*,
    engine::Engine,
    store::{SqliteStore, Store},
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;

pub enum Action {
    Reply(Result<ModelOutput>),
    Block,
}
pub struct Fake {
    pub requests: Mutex<Vec<Value>>,
    pub started: Notify,
    actions: Mutex<VecDeque<Action>>,
}
#[async_trait]
impl ModelAdapter for Fake {
    async fn invoke(&self, request: InvokeRequest) -> Result<ModelOutput> {
        self.requests.lock().unwrap().push(json!({
            "node":request.node,"model":request.model.id,"tools":request.tools,
            "messages":request.messages,"json_object":request.json_object,
        }));
        let action = self
            .actions
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected request");
        self.started.notify_one();
        match action {
            Action::Reply(output) => output,
            Action::Block => std::future::pending().await,
        }
    }
}
pub fn response(text: &str, usage: bool) -> Action {
    Action::Reply(Ok(ModelOutput {
        text: text.into(),
        tool_calls: vec![],
        usage: usage.then_some(Usage {
            input_tokens: 10,
            output_tokens: 5,
        }),
        request_id: Some("test-request".into()),
        complete: true,
    }))
}
pub fn proposal() -> Value {
    json!({"proposal_version":1,"task_type":"writing","classification_confidence":0.8,
        "strategy":"single","required_capabilities":["text"],
        "suggested_acceptance":{"version":"1","nonempty":false,"required_substrings":[],"json_object":false},
        "reason":"A simple greeting"})
}
pub fn submission() -> SubmissionSpec {
    let mut task: Value = serde_json::from_str(include_str!("../../examples/task.json")).unwrap();
    task["task_id"] = json!(id());
    task["deadline_ms"] = json!(now_ms() + 30_000);
    task["strategy"] = Value::Null;
    task["schema_version"] = json!(2);
    task["selection"] = json!({"version":"host-task-fit-v1","min_quality":0.0,
        "target_quality":0.9,"above_target_factor":0.1,"cost_reference":10000,
        "latency_reference_ms":5000,"min_upgrade_gain":0.05});
    task["planning"] = json!({"mode":"auto","max_cost":20_000,"timeout_ms":5000});
    serde_json::from_value(task).unwrap()
}
pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub config: Config,
    pub engine: Arc<Engine>,
    pub store: Arc<SqliteStore>,
    pub fake: Arc<Fake>,
}
pub async fn setup(actions: Vec<Action>, adjust: impl FnOnce(&mut Config)) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let mut config: Config =
        serde_json::from_str(include_str!("../../examples/config.json")).unwrap();
    config.database_path = dir.path().join("engine.db").to_str().unwrap().into();
    config.planner_models.insert("local".into());
    config.close_grace_ms = 0;
    adjust(&mut config);
    let store = Arc::new(SqliteStore::open(&config.database_path).await.unwrap());
    let fake = Arc::new(Fake {
        requests: Mutex::new(vec![]),
        started: Notify::new(),
        actions: Mutex::new(actions.into()),
    });
    let engine =
        Arc::new(Engine::with_components(config.clone(), store.clone(), fake.clone()).unwrap());
    Fixture {
        dir,
        config,
        engine,
        store,
        fake,
    }
}
pub fn saved(f: &Fixture, id: &str) -> (Value, Value, Value, String) {
    let connection = rusqlite::Connection::open(f.dir.path().join("engine.db")).unwrap();
    connection
        .query_row(
            "SELECT spec,plan,checkpoint,status FROM tasks WHERE id=?",
            [id],
            |row| {
                Ok((
                    serde_json::from_str(&row.get::<_, String>(0)?).unwrap(),
                    serde_json::from_str(&row.get::<_, String>(1)?).unwrap(),
                    serde_json::from_str(&row.get::<_, String>(2)?).unwrap(),
                    row.get(3)?,
                ))
            },
        )
        .unwrap()
}
pub async fn ledger(f: &Fixture, id: &str) -> model_collaboration_engine::store::Ledger {
    f.store.ledger(id).await.unwrap()
}
