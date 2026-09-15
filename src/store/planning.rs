//! Versioned submission payloads and immutable plans within the current database schema.
use super::{audit, db_call, SqliteStore};
use crate::contracts::{EngineError, Result, SubmissionSpec};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

impl SqliteStore {
    pub(super) async fn insert_submission(
        &self,
        submission: &SubmissionSpec,
        config_hash: &str,
    ) -> Result<()> {
        let _gate = self.gate.lock().await;
        let submission = submission.clone();
        let hash = config_hash.to_owned();
        db_call(&self.conn, move |connection| {
            let transaction = connection.transaction()?;
            if transaction.query_row("SELECT 1 FROM tasks WHERE id=?", [&submission.task_id], |_| Ok(())).optional()?.is_some() {
                return Err(EngineError::new("configuration", "task_id already exists"));
            }
            let payload = json!({"payload_version": 1, "submission": submission});
            transaction.execute(
                "INSERT INTO tasks(id,spec,config_hash,plan,status,total,checkpoint) VALUES(?,?,?,'null','running',?,?)",
                params![submission.task_id, payload.to_string(), hash, submission.budget as i64,
                    json!({"version":1,"phase":"admitted"}).to_string()],
            )?;
            audit(&transaction, &submission.task_id, "submission_created", json!({"payload_version":1}))?;
            transaction.commit()?;
            Ok(())
        }).await
    }

    pub(super) async fn persist_plan(
        &self,
        task: &str,
        plan: Value,
        checkpoint: Value,
    ) -> Result<()> {
        let _gate = self.gate.lock().await;
        let task = task.to_owned();
        db_call(&self.conn, move |connection| {
            let transaction = connection.transaction()?;
            let changed = transaction.execute(
                "UPDATE tasks SET plan=?,checkpoint=? WHERE id=? AND status='running'",
                params![plan.to_string(), checkpoint.to_string(), task],
            )?;
            if changed != 1 {
                return Err(EngineError::new("storage", "plan requires a running task"));
            }
            audit(
                &transaction,
                &task,
                "plan_saved",
                json!({"plan":plan,"checkpoint":checkpoint}),
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }
}
