//! Settlement and reconciliation preserve call evidence in the same ledger transaction.
use super::{audit, db_call, read_ledger, SqliteStore};
use crate::contracts::{EngineError, Result};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub(super) enum SettlementEvidence {
    Invocation(Value),
    Reconciliation(String),
}

impl SettlementEvidence {
    fn resolve(self, connection: &Connection, task: &str, attempt: &str) -> Result<Value> {
        match self {
            Self::Invocation(outcome) => Ok(outcome),
            Self::Reconciliation(evidence) => {
                let saved: Option<String> = connection.query_row(
                    "SELECT outcome FROM attempts WHERE task=? AND id=?",
                    params![task, attempt],
                    |row| row.get(0),
                )?;
                let mut outcome = match saved {
                    Some(saved) => serde_json::from_str::<Value>(&saved)
                        .map_err(|_| EngineError::new("storage", "invalid saved call outcome"))?,
                    None => json!({}),
                };
                // Legacy null outcomes contain no call evidence; never invent metrics.
                if outcome.is_null() {
                    outcome = json!({});
                }
                let object = outcome.as_object_mut().ok_or_else(|| {
                    EngineError::new("storage", "saved call outcome must be an object")
                })?;
                object.insert("reconciliation_evidence".into(), json!(evidence));
                Ok(outcome)
            }
        }
    }
}

impl SqliteStore {
    pub(super) async fn settle_attempt(
        &self,
        task: &str,
        attempt: &str,
        cost: Option<u64>,
        evidence: SettlementEvidence,
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
            let outcome = evidence.resolve(&tx, &task, &attempt)?;
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
}
