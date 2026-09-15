//! Business transactions, not a generic key/value persistence interface.
mod feedback;
mod metrics;
mod planning;
mod recovery;
mod schema;

use crate::{contracts::*, events::Event};
use async_trait::async_trait;
use fs2::FileExt;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    path::Path,
    sync::Mutex,
};
use tokio_rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 2;
const APPLICATION_ID: i64 = 0x4d434531;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ledger {
    pub total: u64,
    pub settled: u64,
    pub reserved: u64,
    pub calls: u32,
}
impl Ledger {
    pub fn available(&self) -> u64 {
        self.total
            .saturating_sub(self.settled)
            .saturating_sub(self.reserved)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryRecord {
    pub task: TaskSpec,
    /// Original submission, if this task uses the versioned planning entry point.
    #[serde(default)]
    pub submission: Option<SubmissionSpec>,
    #[serde(default)]
    pub plan: Value,
    pub config_hash: String,
    pub status: String,
    pub checkpoint: Value,
    pub ledger: Ledger,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub attempts: Vec<AttemptRecord>,
}

/// Persisted evidence, including zero-cost attempts whose outcome is unknown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub attempt_id: String,
    pub amount: u64,
    pub cost: Option<u64>,
    pub state: String,
    pub metadata: Value,
    pub outcome: Option<Value>,
}

#[async_trait]
pub trait Store: Send + Sync {
    /// Atomically persist the immutable submission and initial ledger.
    async fn create_submission(&self, submission: &SubmissionSpec, config_hash: &str)
        -> Result<()>;
    /// Atomically save the plan, routing snapshot and checkpoint.
    async fn save_plan(&self, task: &str, plan: Value, checkpoint: Value) -> Result<()>;
    async fn reserve(
        &self,
        task: &str,
        attempt: &str,
        amount: u64,
        max_calls: u32,
        metadata: Value,
    ) -> Result<()>;
    async fn settle(
        &self,
        task: &str,
        attempt: &str,
        cost: Option<u64>,
        outcome: Value,
    ) -> Result<()>;
    async fn checkpoint(&self, task: &str, value: Value) -> Result<()>;
    async fn event(&self, event: &Event) -> Result<()>;
    async fn finish(&self, task: &str, status: &str, value: Value) -> Result<()>;
    async fn ledger(&self, task: &str) -> Result<Ledger>;
    async fn records(&self) -> Result<Vec<RecoveryRecord>>;
    async fn reconcile(&self, task: &str, attempt: &str, cost: u64, evidence: &str) -> Result<()>;
    async fn record_evaluation(&self, record: &EvaluationRecord) -> Result<()>;
    async fn record_feedback(&self, feedback: &Feedback) -> Result<()>;
    async fn evaluations(&self, task: &str) -> Result<Vec<EvaluationRecord>>;
    async fn metrics(&self) -> Result<MetricsSnapshot>;
    async fn close(&self) -> Result<()>;
}

pub struct SqliteStore {
    conn: Connection,
    lock: Mutex<Option<File>>,
    gate: tokio::sync::Mutex<()>,
}
fn lock_database(path: &str) -> Result<File> {
    let path = Path::new(path);
    if path.as_os_str().is_empty() || path == Path::new(":memory:") {
        return Err(EngineError::new(
            "configuration",
            "explicit database file required",
        ));
    }
    // Do not create parent directories or truncate existing files.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|_| {
            EngineError::new(
                "storage",
                "cannot open database file; parent directory must exist",
            )
        })?;
    let identity = file_id::get_file_id(path)
        .map_err(|_| EngineError::new("storage", "cannot identify database file"))?;
    // Identity-based locking also covers hard links and symlinks. Lock files are
    // intentionally persistent: unlinking a lock file creates an ownership race.
    let lock_path =
        std::env::temp_dir().join(format!("mce-{}.lock", digest(&format!("{identity:?}"))));
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|_| EngineError::new("storage", "cannot open ownership lock"))?;
    lock.try_lock_exclusive()
        .map_err(|_| EngineError::new("database_busy", "database belongs to an active engine"))?;
    drop(file);
    Ok(lock)
}
impl SqliteStore {
    pub async fn open(path: &str) -> Result<Self> {
        let path = path.to_owned();
        let path_for_lock = path.clone();
        let lock = tokio::task::spawn_blocking(move || lock_database(&path_for_lock))
            .await
            .map_err(|_| EngineError::new("storage", "ownership worker failed"))??;
        let conn = Connection::open(path)
            .await
            .map_err(|_| EngineError::new("storage", "cannot open SQLite"))?;
        db_call(&conn, schema::open).await?;
        Ok(Self {
            conn,
            lock: Mutex::new(Some(lock)),
            gate: tokio::sync::Mutex::new(()),
        })
    }
}
async fn db_call<T: Send + 'static>(
    conn: &Connection,
    f: impl FnOnce(&mut rusqlite::Connection) -> Result<T> + Send + 'static,
) -> Result<T> {
    conn.call(f).await.map_err(|e| match e {
        tokio_rusqlite::Error::Error(e) => e,
        _ => EngineError::new("storage", "database worker unavailable"),
    })
}
fn read_ledger(c: &rusqlite::Connection, task: &str) -> Result<Ledger> {
    Ok(c.query_row(
        "SELECT total,settled,reserved,calls FROM tasks WHERE id=?",
        [task],
        |r| {
            Ok(Ledger {
                total: r.get::<_, i64>(0)? as u64,
                settled: r.get::<_, i64>(1)? as u64,
                reserved: r.get::<_, i64>(2)? as u64,
                calls: r.get(3)?,
            })
        },
    )?)
}
fn audit(c: &rusqlite::Connection, task: &str, kind: &str, data: Value) -> Result<()> {
    c.execute(
        "INSERT INTO events(task,payload) VALUES(?,?)",
        params![
            task,
            json!({"kind":kind,"timestamp_ms":now_ms(),"data":data}).to_string()
        ],
    )?;
    Ok(())
}
#[async_trait]
impl Store for SqliteStore {
    async fn record_evaluation(&self, record: &EvaluationRecord) -> Result<()> {
        self.persist_evaluation(record).await
    }
    async fn record_feedback(&self, feedback: &Feedback) -> Result<()> {
        self.persist_feedback(feedback).await
    }
    async fn evaluations(&self, task: &str) -> Result<Vec<EvaluationRecord>> {
        self.read_evaluations(task).await
    }
    async fn metrics(&self) -> Result<MetricsSnapshot> {
        self.read_metrics().await
    }
    async fn create_submission(
        &self,
        submission: &SubmissionSpec,
        config_hash: &str,
    ) -> Result<()> {
        self.insert_submission(submission, config_hash).await
    }
    async fn save_plan(&self, task: &str, plan: Value, checkpoint: Value) -> Result<()> {
        self.persist_plan(task, plan, checkpoint).await
    }
    async fn reserve(
        &self,
        task: &str,
        attempt: &str,
        amount: u64,
        max_calls: u32,
        metadata: Value,
    ) -> Result<()> {
        let _gate = self.gate.lock().await;
        let task = task.to_owned();
        let attempt = attempt.to_owned();
        db_call(&self.conn, move |c| {
            let tx = c.transaction()?;
            let l = read_ledger(&tx, &task)?;
            let state: String =
                tx.query_row("SELECT status FROM tasks WHERE id=?", [&task], |r| r.get(0))?;
            if state != "running" {
                return Err(EngineError::new("cancelled", "task is not running"));
            }
            if amount > l.available() || l.calls >= max_calls {
                return Err(EngineError::new(
                    "budget",
                    "insufficient budget or total call limit reached",
                ));
            }
            tx.execute(
                "INSERT INTO attempts(id,task,amount,state,metadata) VALUES(?,?,?,'pending',?)",
                params![attempt, task, amount as i64, metadata.to_string()],
            )?;
            tx.execute(
                "UPDATE tasks SET reserved=reserved+?,calls=calls+1 WHERE id=?",
                params![amount as i64, task],
            )?;
            audit(
                &tx,
                &task,
                "attempt_reserved",
                json!({"attempt_id":attempt,"amount":amount,"metadata":metadata}),
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
    async fn settle(
        &self,
        task: &str,
        attempt: &str,
        cost: Option<u64>,
        outcome: Value,
    ) -> Result<()> {
        let _gate = self.gate.lock().await;
        let task = task.to_owned();
        let attempt = attempt.to_owned();
        db_call(&self.conn, move |c| {
            let tx = c.transaction()?;
            let (amount, old, state): (u64, Option<u64>, String) = tx.query_row(
                "SELECT amount,cost,state FROM attempts WHERE task=? AND id=?",
                params![task, attempt],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)? as u64,
                        r.get::<_, Option<i64>>(1)?.map(|v| v as u64),
                        r.get(2)?,
                    ))
                },
            )?;
            if state == "settled" {
                if old == cost {
                    return Ok(());
                }
                return Err(EngineError::new("storage", "conflicting settlement"));
            }
            if let Some(cost) = cost {
                if cost > i64::MAX as u64 {
                    return Err(EngineError::new(
                        "budget",
                        "reported cost exceeds ledger range",
                    ));
                }
                let l = read_ledger(&tx, &task)?;
                if l.settled
                    .checked_add(cost)
                    .is_none_or(|s| s > i64::MAX as u64)
                {
                    return Err(EngineError::new("budget", "ledger overflow"));
                }
                tx.execute(
                    "UPDATE tasks SET settled=settled+?,reserved=reserved-? WHERE id=?",
                    params![cost as i64, amount as i64, task],
                )?;
                tx.execute(
                    "UPDATE attempts SET cost=?,state='settled',outcome=? WHERE id=?",
                    params![cost as i64, outcome.to_string(), attempt],
                )?;
            } else {
                tx.execute(
                    "UPDATE attempts SET state='unresolved',outcome=? WHERE id=?",
                    params![outcome.to_string(), attempt],
                )?;
            }
            audit(
                &tx,
                &task,
                "attempt_settled",
                json!({"attempt_id":attempt,"actual_cost":cost,"outcome":outcome}),
            )?;
            tx.commit()?;
            if cost.is_some_and(|cost| cost > amount) {
                return Err(EngineError::new(
                    "budget",
                    "provider usage exceeded declared reservation; actual cost recorded",
                ));
            }
            Ok(())
        })
        .await
    }
    async fn checkpoint(&self, task: &str, value: Value) -> Result<()> {
        let _gate = self.gate.lock().await;
        let task = task.to_owned();
        db_call(&self.conn, move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "UPDATE tasks SET checkpoint=? WHERE id=?",
                params![value.to_string(), task],
            )?;
            audit(&tx, &task, "checkpoint", value)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
    async fn event(&self, event: &Event) -> Result<()> {
        let _gate = self.gate.lock().await;
        let event = event.clone();
        db_call(&self.conn, move |c| {
            c.execute(
                "INSERT INTO events(task,payload) VALUES(?,?)",
                params![event.task_id, serde_json::to_string(&event).unwrap()],
            )?;
            Ok(())
        })
        .await
    }
    async fn finish(&self, task: &str, status: &str, value: Value) -> Result<()> {
        let _gate = self.gate.lock().await;
        let task = task.to_owned();
        let status = status.to_owned();
        db_call(&self.conn, move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "UPDATE tasks SET status=?,result=? WHERE id=?",
                params![status, value.to_string(), task],
            )?;
            audit(
                &tx,
                &task,
                "task_state",
                json!({"status":status,"result":value}),
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
    async fn ledger(&self, task: &str) -> Result<Ledger> {
        let _gate = self.gate.lock().await;
        let task = task.to_owned();
        db_call(&self.conn, move |c| read_ledger(c, &task)).await
    }
    async fn records(&self) -> Result<Vec<RecoveryRecord>> {
        let _gate = self.gate.lock().await;
        db_call(&self.conn, recovery::read_records).await
    }
    async fn reconcile(&self, task: &str, attempt: &str, cost: u64, evidence: &str) -> Result<()> {
        if evidence.trim().is_empty() {
            return Err(EngineError::new(
                "recovery",
                "reconciliation evidence required",
            ));
        }
        self.settle(
            task,
            attempt,
            Some(cost),
            json!({"reconciliation_evidence":evidence}),
        )
        .await
    }
    async fn close(&self) -> Result<()> {
        let _gate = self.gate.lock().await;
        if self.lock.lock().unwrap().is_none() {
            return Ok(());
        }
        self.conn
            .clone()
            .close()
            .await
            .map_err(|_| EngineError::new("storage", "SQLite close failed; ownership retained"))?;
        self.lock.lock().unwrap().take();
        Ok(())
    }
}
