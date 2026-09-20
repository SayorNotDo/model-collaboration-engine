//! Bounded execution; the host retains ownership of cancellation and tool permissions.
use crate::contracts::{EvaluationRecord, Feedback, MetricsSnapshot};
mod execution;
mod invocation;
mod planning;
mod tools;

use crate::{
    adapter::{ModelAdapter, OpenAIAdapter},
    contracts::{
        digest, id, now_ms, Config, EngineError, Result, SubmissionSpec, TaskResult, TaskSpec,
    },
    decision::DecisionModel,
    events::{Event, EventSink},
    store::{RecoveryRecord, SqliteStore, Store},
    tools::ToolExecutor,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{mpsc, Notify, Semaphore};
use tokio_util::sync::CancellationToken;

pub struct Engine {
    config: Config,
    store: Arc<dyn Store>,
    adapter: Arc<dyn ModelAdapter>,
    decision_model: Option<Arc<dyn DecisionModel>>,
    slots: Semaphore,
    lifecycle: Mutex<Lifecycle>,
    drained: Notify,
    close_gate: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct Lifecycle {
    closing: bool,
    closed: bool,
    active: BTreeMap<String, CancellationToken>,
}

struct ActiveRun<'a> {
    engine: &'a Engine,
    registration: String,
}
impl Drop for ActiveRun<'_> {
    fn drop(&mut self) {
        self.engine
            .lifecycle
            .lock()
            .unwrap()
            .active
            .remove(&self.registration);
        self.engine.drained.notify_one();
    }
}

struct RunContext {
    cancel: CancellationToken,
    sink: EventSink,
    sequence: Arc<AtomicU64>,
    tools: Option<Arc<dyn ToolExecutor>>,
    routing: Option<crate::router::RoutingSnapshot>,
}

impl Engine {
    pub async fn open(config: Config) -> Result<Self> {
        config.validate(true)?;
        let adapter = Arc::new(OpenAIAdapter::new(&config)?);
        let store = Arc::new(SqliteStore::open(&config.database_path).await?);
        Self::with_components(config, store, adapter)
    }

    pub fn with_components(
        config: Config,
        store: Arc<dyn Store>,
        adapter: Arc<dyn ModelAdapter>,
    ) -> Result<Self> {
        Self::with_components_and_decision(config, store, adapter, None)
    }

    pub fn with_components_and_decision(
        config: Config,
        store: Arc<dyn Store>,
        adapter: Arc<dyn ModelAdapter>,
        decision_model: Option<Arc<dyn DecisionModel>>,
    ) -> Result<Self> {
        config.validate(false)?;
        if config.decision.is_some() && decision_model.is_none() {
            return Err(EngineError::new(
                "decision",
                "decision configuration requires an injected decision model",
            ));
        }
        Ok(Self {
            slots: Semaphore::new(config.max_concurrency),
            config,
            store,
            adapter,
            decision_model,
            lifecycle: Mutex::new(Lifecycle::default()),
            drained: Notify::new(),
            close_gate: tokio::sync::Mutex::new(()),
        })
    }

    pub async fn run(
        &self,
        task: impl Into<SubmissionSpec>,
        cancel: CancellationToken,
    ) -> Result<TaskResult> {
        self.run_with_host(task, cancel, None, None).await
    }

    /// Read persisted recovery evidence without replaying or changing task state.
    pub async fn recovery_records(&self) -> Result<Vec<RecoveryRecord>> {
        let _close = self.close_gate.lock().await;
        if self.lifecycle.lock().unwrap().closing {
            return Err(EngineError::new("closed", "engine is closing or closed"));
        }
        self.store.records().await
    }

    /// Record host acceptance for a terminal artifact; identical IDs are idempotent.
    /// Cancellation of this await does not guarantee rollback: retry the same payload.
    pub async fn record_feedback(&self, feedback: &Feedback) -> Result<()> {
        let _close = self.close_gate.lock().await;
        if self.lifecycle.lock().unwrap().closing {
            return Err(EngineError::new("closed", "engine is closing or closed"));
        }
        self.store.record_feedback(feedback).await
    }

    /// Read saved candidates and separate deterministic/critic verdicts.
    /// Unknown task IDs return an empty collection; closing engines reject inspection.
    pub async fn evaluations(&self, task: &str) -> Result<Vec<EvaluationRecord>> {
        let _close = self.close_gate.lock().await;
        if self.lifecycle.lock().unwrap().closing {
            return Err(EngineError::new("closed", "engine is closing or closed"));
        }
        self.store.evaluations(task).await
    }

    /// Read consistent all-history aggregates; execution completion is not acceptance.
    pub async fn metrics(&self) -> Result<MetricsSnapshot> {
        let _close = self.close_gate.lock().await;
        if self.lifecycle.lock().unwrap().closing {
            return Err(EngineError::new("closed", "engine is closing or closed"));
        }
        self.store.metrics().await
    }

    pub fn event_channel(&self) -> (mpsc::Sender<Event>, mpsc::Receiver<Event>) {
        mpsc::channel(self.config.event_capacity)
    }

    /// All inputs normalize to one submission, plan, snapshot and accounting path.
    pub async fn run_with_host(
        &self,
        submission: impl Into<SubmissionSpec>,
        cancel: CancellationToken,
        tools: Option<Arc<dyn ToolExecutor>>,
        events: Option<mpsc::Sender<Event>>,
    ) -> Result<TaskResult> {
        let submission = submission.into();
        submission.validate(&self.config)?;
        let task = submission.task(
            submission
                .strategy
                .clone()
                .unwrap_or(crate::contracts::Strategy::Single),
        );
        self.run_input(task, submission, cancel, tools, events)
            .await
    }

    async fn run_input(
        &self,
        task: TaskSpec,
        submission: SubmissionSpec,
        cancel: CancellationToken,
        tools: Option<Arc<dyn ToolExecutor>>,
        events: Option<mpsc::Sender<Event>>,
    ) -> Result<TaskResult> {
        task.validate(&self.config)?;
        if !task.tools.is_empty() && tools.is_none() {
            return Err(EngineError::new(
                "configuration",
                "tool declarations require an explicit host executor",
            ));
        }
        // Registration and closing share one lock, including tasks waiting for a slot.
        // A child token lets shutdown cancel this run without cancelling caller siblings.
        let cancel = cancel.child_token();
        let _active = {
            let mut state = self.lifecycle.lock().unwrap();
            if state.closing {
                return Err(
                    EngineError::new("closed", "engine is closing or closed").task(&task.task_id)
                );
            }
            let registration = id();
            state.active.insert(registration.clone(), cancel.clone());
            ActiveRun {
                engine: self,
                registration,
            }
        };
        let remaining = task
            .deadline_ms
            .saturating_sub(task.finalization_ms)
            .saturating_sub(now_ms());
        let _permit = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(EngineError::new("cancelled", "task cancelled while queued").task(&task.task_id)),
            permit = tokio::time::timeout(Duration::from_millis(remaining), self.slots.acquire()) =>
                permit.map_err(|_| EngineError::new("deadline", "deadline reached while queued"))?
                    .map_err(|_| EngineError::new("closed", "engine is closed"))?,
        };
        task.validate(&self.config)?;
        self.store
            .create_submission(&submission, &digest(&self.config))
            .await?;
        let mut context = RunContext {
            sink: EventSink {
                sender: events,
                max_bytes: self.config.event_max_bytes,
                cancel: cancel.clone(),
            },
            cancel,
            tools,
            sequence: Arc::new(AtomicU64::new(1)),
            routing: None,
        };
        let result = async {
            self.emit(
                &task,
                &context,
                None,
                None,
                "task_started",
                json!({
                    "strategy": submission.strategy
                }),
            )
            .await?;
            let (effective_task, routing) = self
                .prepare_submission(&submission, &task, &context)
                .await?;
            context.routing = Some(routing);
            self.execute(&effective_task, &context).await
        }
        .await;
        match result {
            Ok(result) => {
                self.store
                    .finish(&task.task_id, &result.status, json!(result))
                    .await?;
                Ok(result)
            }
            Err(error) => {
                let error = error.task(&task.task_id);
                let status = if error.kind == "cancelled" {
                    "cancelled"
                } else {
                    "failed"
                };
                self.store
                    .finish(&task.task_id, status, json!(error))
                    .await?;
                Err(error)
            }
        }
    }

    async fn emit(
        &self,
        task: &TaskSpec,
        context: &RunContext,
        node: Option<&str>,
        attempt: Option<&str>,
        kind: &str,
        data: serde_json::Value,
    ) -> Result<()> {
        let event = Event {
            task_id: task.task_id.clone(),
            node_id: node.map(String::from),
            attempt_id: attempt.map(String::from),
            sequence: context.sequence.fetch_add(1, Ordering::Relaxed),
            timestamp_ms: now_ms(),
            kind: kind.into(),
            data,
        };
        self.store.event(&event).await?;
        let remaining = task
            .deadline_ms
            .saturating_sub(task.finalization_ms)
            .saturating_sub(now_ms());
        tokio::time::timeout(Duration::from_millis(remaining), context.sink.send(event))
            .await
            .map_err(|_| {
                EngineError::new("deadline", "event consumer exceeded execution deadline")
            })?
    }

    async fn wait_drained(&self) {
        loop {
            let notified = self.drained.notified();
            if self.lifecycle.lock().unwrap().active.is_empty() {
                return;
            }
            notified.await;
        }
    }

    /// Stop admission, allow a grace period, then cancel and wait for accounting.
    /// On cleanup timeout the store remains owned; callers may retry close.
    pub async fn close(&self) -> Result<()> {
        let _gate = self.close_gate.lock().await;
        {
            let mut state = self.lifecycle.lock().unwrap();
            if state.closed {
                return Ok(());
            }
            state.closing = true;
        }
        self.slots.close();
        if tokio::time::timeout(
            Duration::from_millis(self.config.close_grace_ms),
            self.wait_drained(),
        )
        .await
        .is_err()
        {
            let tokens: Vec<_> = self
                .lifecycle
                .lock()
                .unwrap()
                .active
                .values()
                .cloned()
                .collect();
            for token in tokens {
                token.cancel();
            }
            if tokio::time::timeout(
                Duration::from_millis(self.config.cleanup_timeout_ms),
                self.wait_drained(),
            )
            .await
            .is_err()
            {
                return Err(EngineError::new(
                    "cleanup_timeout",
                    "active tasks have not finished accounting; database ownership retained",
                ));
            }
        }
        self.store.close().await?;
        self.lifecycle.lock().unwrap().closed = true;
        Ok(())
    }
}
