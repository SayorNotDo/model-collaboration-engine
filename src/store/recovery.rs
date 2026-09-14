//! Consistent, read-only task and attempt evidence.
use super::{read_ledger, AttemptRecord, RecoveryRecord};
use crate::contracts::{EngineError, Result};
use serde_json::Value;

pub(super) fn read_records(connection: &mut rusqlite::Connection) -> Result<Vec<RecoveryRecord>> {
    // Keep task, ledger and attempt evidence in one read transaction.
    let transaction = connection.transaction()?;
    let records = {
        let mut statement = transaction.prepare(
            "SELECT id,spec,config_hash,status,checkpoint,result FROM tasks
             WHERE status IN ('running','human_required') OR reserved>0
             OR EXISTS (SELECT 1 FROM attempts WHERE task=tasks.id AND state IN ('pending','unresolved'))
             ORDER BY id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut records = Vec::with_capacity(rows.len());
        for (id, spec, config_hash, status, checkpoint, result) in rows {
            let task = serde_json::from_str(&spec)
                .map_err(|_| EngineError::new("storage", "invalid saved task"))?;
            records.push(RecoveryRecord {
                task,
                config_hash,
                status,
                checkpoint: saved_json(&checkpoint)?,
                ledger: read_ledger(&transaction, &id)?,
                result: result.as_deref().map(saved_json).transpose()?,
                attempts: read_attempts(&transaction, &id)?,
            });
        }
        records
    };
    transaction.commit()?;
    Ok(records)
}

fn read_attempts(connection: &rusqlite::Connection, task_id: &str) -> Result<Vec<AttemptRecord>> {
    let mut statement = connection.prepare(
        "SELECT id,amount,cost,state,metadata,outcome FROM attempts WHERE task=? ORDER BY rowid",
    )?;
    let rows = statement.query_map([task_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    rows.map(|row| {
        let (attempt_id, amount, cost, state, metadata, outcome) = row?;
        Ok(AttemptRecord {
            attempt_id,
            amount: saved_cost(amount)?,
            cost: cost.map(saved_cost).transpose()?,
            state,
            metadata: saved_json(&metadata)?,
            outcome: outcome.as_deref().map(saved_json).transpose()?,
        })
    })
    .collect()
}

fn saved_cost(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| EngineError::new("storage", "negative saved cost"))
}
fn saved_json(value: &str) -> Result<Value> {
    serde_json::from_str(value)
        .map_err(|_| EngineError::new("storage", "invalid saved JSON evidence"))
}
