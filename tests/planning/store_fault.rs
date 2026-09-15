//! A store fault must terminate planning even when the host supplied a fallback.
use async_trait::async_trait;
use model_collaboration_engine::{
    contracts::*,
    events::Event,
    store::{Ledger, RecoveryRecord, Store},
};
use serde_json::Value;
use std::sync::Arc;

pub struct FaultStore {
    pub inner: Arc<dyn Store>,
    pub fail_settlement: bool,
    pub reservation_gate: Option<Arc<ReservationGate>>,
}

#[derive(Default)]
pub struct ReservationGate {
    pub reached: tokio::sync::Notify,
    pub release: tokio::sync::Notify,
}
#[async_trait]
impl Store for FaultStore {
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

    async fn create_submission(&self, s: &SubmissionSpec, hash: &str) -> Result<()> {
        self.inner.create_submission(s, hash).await
    }
    async fn save_plan(&self, task: &str, plan: Value, checkpoint: Value) -> Result<()> {
        self.inner.save_plan(task, plan, checkpoint).await
    }
    async fn reserve(
        &self,
        task: &str,
        attempt: &str,
        amount: u64,
        max_calls: u32,
        metadata: Value,
    ) -> Result<()> {
        let planner = metadata["role"] == "planner";
        self.inner
            .reserve(task, attempt, amount, max_calls, metadata)
            .await?;
        if let Some(gate) = self.reservation_gate.as_ref().filter(|_| planner) {
            gate.reached.notify_one();
            gate.release.notified().await;
        }
        Ok(())
    }
    async fn settle(
        &self,
        task: &str,
        attempt: &str,
        cost: Option<u64>,
        outcome: Value,
    ) -> Result<()> {
        if self.fail_settlement {
            Err(EngineError::new("storage", "injected settlement failure"))
        } else {
            self.inner.settle(task, attempt, cost, outcome).await
        }
    }
    async fn checkpoint(&self, task: &str, value: Value) -> Result<()> {
        self.inner.checkpoint(task, value).await
    }
    async fn event(&self, event: &Event) -> Result<()> {
        self.inner.event(event).await
    }
    async fn finish(&self, task: &str, status: &str, value: Value) -> Result<()> {
        self.inner.finish(task, status, value).await
    }
    async fn ledger(&self, task: &str) -> Result<Ledger> {
        self.inner.ledger(task).await
    }
    async fn records(&self) -> Result<Vec<RecoveryRecord>> {
        self.inner.records().await
    }
    async fn reconcile(&self, task: &str, attempt: &str, cost: u64, evidence: &str) -> Result<()> {
        self.inner.reconcile(task, attempt, cost, evidence).await
    }
    async fn close(&self) -> Result<()> {
        self.inner.close().await
    }
}
