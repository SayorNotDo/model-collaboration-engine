//! Read-only aggregate snapshots; ledger costs remain authoritative.
use super::{db_call, SqliteStore};
use crate::contracts::{
    now_ms, CallStatistics, EngineError, EvaluationRecord, MetricsSnapshot, QualityStatistics,
    Result, TaskStatistics,
};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::BTreeMap;

impl SqliteStore {
    pub(super) async fn read_evaluations(&self, task: &str) -> Result<Vec<EvaluationRecord>> {
        let _gate = self.gate.lock().await;
        let task = task.to_owned();
        db_call(&self.conn, move |connection| {
            let mut query =
                connection.prepare("SELECT record FROM evaluations WHERE task=? ORDER BY rowid")?;
            let mut records = Vec::new();
            for row in query.query_map([task], |row| row.get::<_, String>(0))? {
                records.push(
                    serde_json::from_str(&row?)
                        .map_err(|_| EngineError::new("storage", "invalid evaluation"))?,
                );
            }
            Ok(records)
        })
        .await
    }

    pub(super) async fn read_metrics(&self) -> Result<MetricsSnapshot> {
        let _gate = self.gate.lock().await;
        db_call(&self.conn, |connection| {
            // All components share the same SQLite read snapshot.
            let transaction = connection.transaction()?;
            let revision =
                transaction.query_row("SELECT COALESCE(MAX(id),0) FROM feedback", [], |row| {
                    row.get::<_, i64>(0)
                })? as u64;
            let result = MetricsSnapshot {
                revision,
                captured_at_ms: now_ms(),
                quality: quality(&transaction)?,
                tasks: tasks(&transaction)?,
                calls: calls(&transaction)?,
            };
            transaction.commit()?;
            Ok(result)
        })
        .await
    }
}

fn quality(connection: &Connection) -> Result<Vec<QualityStatistics>> {
    let mut query = connection.prepare(
        "SELECT key_json,kind,SUM(accepted),COUNT(*) FROM feedback
         GROUP BY key_json,kind ORDER BY key_json,kind",
    )?;
    let rows = query.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)? as u64,
            row.get::<_, i64>(3)? as u64,
        ))
    })?;
    let mut result = Vec::new();
    for row in rows {
        let (key, kind, accepted, samples) = row?;
        result.push(QualityStatistics {
            key: serde_json::from_str(&key)
                .map_err(|_| EngineError::new("storage", "invalid feedback key"))?,
            kind: serde_json::from_str(&kind)
                .map_err(|_| EngineError::new("storage", "invalid feedback kind"))?,
            accepted,
            samples,
        });
    }
    Ok(result)
}

fn tasks(connection: &Connection) -> Result<TaskStatistics> {
    let mut query = connection.prepare("SELECT status,COUNT(*) FROM tasks GROUP BY status")?;
    let rows = query.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
    })?;
    let mut result = TaskStatistics::default();
    for row in rows {
        let (status, count) = row?;
        result.total += count;
        match status.as_str() {
            "completed" => result.completed += count,
            "human_required" => result.human_required += count,
            "cancelled" => result.cancelled += count,
            "running" => result.running += count,
            _ => result.failed += count,
        }
    }
    Ok(result)
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct CallKey {
    attempt_kind: String,
    model_id: String,
    model_version: String,
    config_hash: String,
    node: String,
    tool_name: Option<String>,
}
impl CallKey {
    fn from_metadata(metadata: &Value, config_hash: String) -> Self {
        let route = &metadata["route"];
        let attempt_kind = if metadata["kind"] == "tool" {
            "tool"
        } else if route.is_object() {
            "model"
        } else {
            "unknown"
        };
        Self {
            attempt_kind: attempt_kind.into(),
            model_id: route["model_id"].as_str().unwrap_or("unknown").into(),
            model_version: route["model_version"].as_str().unwrap_or("unknown").into(),
            config_hash,
            node: if attempt_kind == "tool" {
                "tool"
            } else {
                route["node_id"].as_str().unwrap_or("unknown")
            }
            .into(),
            tool_name: metadata["request"]["call"]["name"]
                .as_str()
                .map(str::to_owned),
        }
    }
    fn empty_statistics(&self) -> CallStatistics {
        CallStatistics {
            attempt_kind: self.attempt_kind.clone(),
            model_id: self.model_id.clone(),
            model_version: self.model_version.clone(),
            config_hash: self.config_hash.clone(),
            node: self.node.clone(),
            tool_name: self.tool_name.clone(),
            attempts: 0,
            succeeded: 0,
            failed: 0,
            cancelled: 0,
            timed_out: 0,
            not_dispatched: 0,
            unknown_status: 0,
            known_cost: 0,
            unresolved_reserved: 0,
            unknown_cost_attempts: 0,
            latency_samples: 0,
            mean_latency_ms: None,
        }
    }
}

fn calls(connection: &Connection) -> Result<Vec<CallStatistics>> {
    let mut query = connection.prepare(
        "SELECT a.metadata,a.outcome,a.cost,a.amount,t.config_hash
         FROM attempts a JOIN tasks t ON t.id=a.task ORDER BY a.rowid",
    )?;
    let rows = query.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<i64>>(2)?.map(|n| n as u64),
            row.get::<_, i64>(3)? as u64,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut groups = BTreeMap::<CallKey, (CallStatistics, u128)>::new();
    for row in rows {
        let (metadata, outcome, cost, amount, config_hash) = row?;
        let metadata: Value = serde_json::from_str(&metadata)
            .map_err(|_| EngineError::new("storage", "invalid attempt metadata"))?;
        let outcome: Value = outcome
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|_| EngineError::new("storage", "invalid attempt outcome"))?
            .unwrap_or(Value::Null);
        let key = CallKey::from_metadata(&metadata, config_hash);
        let empty = key.empty_statistics();
        let (stats, latency) = groups.entry(key).or_insert((empty, 0));
        accumulate(stats, latency, &outcome, cost, amount)?;
    }
    Ok(groups
        .into_values()
        .map(|(mut stats, sum)| {
            stats.mean_latency_ms =
                (stats.latency_samples > 0).then(|| sum as f64 / stats.latency_samples as f64);
            stats
        })
        .collect())
}

fn accumulate(
    stats: &mut CallStatistics,
    latency: &mut u128,
    outcome: &Value,
    cost: Option<u64>,
    amount: u64,
) -> Result<()> {
    stats.attempts += 1;
    match outcome["call_metrics"]["status"].as_str() {
        Some("succeeded") => stats.succeeded += 1,
        Some("failed") => stats.failed += 1,
        Some("cancelled") => stats.cancelled += 1,
        Some("timed_out") => stats.timed_out += 1,
        Some("not_dispatched") => stats.not_dispatched += 1,
        _ => stats.unknown_status += 1,
    }
    let target = if cost.is_some() {
        &mut stats.known_cost
    } else {
        stats.unknown_cost_attempts += 1;
        &mut stats.unresolved_reserved
    };
    *target = target
        .checked_add(cost.unwrap_or(amount))
        .ok_or_else(|| EngineError::new("metrics", "aggregate cost overflow"))?;
    if let Some(ms) = outcome["call_metrics"]["latency_ms"].as_u64() {
        *latency += ms as u128;
        stats.latency_samples += 1;
    }
    Ok(())
}
