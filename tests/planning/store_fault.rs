//! A store fault must terminate planning even when the host supplied a fallback.
use async_trait::async_trait;
use model_collaboration_engine::{
    contracts::*,
    events::Event,
    store::{Ledger, RecoveryRecord, Store},
};
use serde_json::Value;
use std::sync::Arc;

pub struct FailSettlement(pub Arc<dyn Store>);
#[async_trait]
impl Store for FailSettlement {
    async fn record_evaluation(&self, record: &EvaluationRecord) -> Result<()> {
        self.0.record_evaluation(record).await
    }
    async fn record_feedback(&self, feedback: &Feedback) -> Result<()> {
        self.0.record_feedback(feedback).await
    }
    async fn evaluations(&self, task: &str) -> Result<Vec<EvaluationRecord>> {
        self.0.evaluations(task).await
    }
    async fn metrics(&self) -> Result<MetricsSnapshot> {
        self.0.metrics().await
    }

    async fn create_submission(&self, s: &SubmissionSpec, hash: &str) -> Result<()> {
        self.0.create_submission(s, hash).await
    }
    async fn save_plan(&self, task: &str, plan: Value, checkpoint: Value) -> Result<()> {
        self.0.save_plan(task, plan, checkpoint).await
    }
    async fn reserve(
        &self,
        task: &str,
        attempt: &str,
        amount: u64,
        max_calls: u32,
        metadata: Value,
    ) -> Result<()> {
        self.0
            .reserve(task, attempt, amount, max_calls, metadata)
            .await
    }
    async fn settle(&self, _: &str, _: &str, _: Option<u64>, _: Value) -> Result<()> {
        Err(EngineError::new("storage", "injected settlement failure"))
    }
    async fn checkpoint(&self, task: &str, value: Value) -> Result<()> {
        self.0.checkpoint(task, value).await
    }
    async fn event(&self, event: &Event) -> Result<()> {
        self.0.event(event).await
    }
    async fn finish(&self, task: &str, status: &str, value: Value) -> Result<()> {
        self.0.finish(task, status, value).await
    }
    async fn ledger(&self, task: &str) -> Result<Ledger> {
        self.0.ledger(task).await
    }
    async fn records(&self) -> Result<Vec<RecoveryRecord>> {
        self.0.records().await
    }
    async fn reconcile(&self, task: &str, attempt: &str, cost: u64, evidence: &str) -> Result<()> {
        self.0.reconcile(task, attempt, cost, evidence).await
    }
    async fn close(&self) -> Result<()> {
        self.0.close().await
    }
}
