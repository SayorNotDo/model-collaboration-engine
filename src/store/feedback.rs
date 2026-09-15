//! Transactional evidence and host feedback. No inferred business success.
use super::{audit, db_call, SqliteStore};
use crate::contracts::{
    digest, EngineError, EvaluationRecord, Feedback, FeedbackKind, ProfileKey, Result,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

impl SqliteStore {
    pub(super) async fn persist_evaluation(&self, record: &EvaluationRecord) -> Result<()> {
        let _gate = self.gate.lock().await;
        let record = record.clone();
        db_call(&self.conn, move |connection| {
            let transaction = connection.transaction()?;
            validate_evaluation(&transaction, &record)?;
            transaction.execute(
                "INSERT INTO evaluations(artifact_id,task,record) VALUES(?,?,?)
                 ON CONFLICT(artifact_id) DO UPDATE SET record=excluded.record",
                params![
                    record.artifact.artifact_id,
                    record.task_id,
                    json!(record).to_string()
                ],
            )?;
            audit(
                &transaction,
                &record.task_id,
                "evaluation_saved",
                json!({
                    "artifact_id": record.artifact.artifact_id,
                    "deterministic": record.deterministic, "critic": record.critic,
                }),
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    pub(super) async fn persist_feedback(&self, feedback: &Feedback) -> Result<()> {
        feedback.validate()?;
        let _gate = self.gate.lock().await;
        let feedback = feedback.clone();
        db_call(&self.conn, move |connection| {
            let transaction = connection.transaction()?;
            let payload = json!(feedback).to_string();
            let existing: Option<String> = transaction
                .query_row(
                    "SELECT payload FROM feedback WHERE feedback_id=?",
                    [&feedback.feedback_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(existing) = existing {
                return if existing == payload {
                    Ok(())
                } else {
                    Err(EngineError::new(
                        "feedback",
                        "feedback ID conflicts with saved evidence",
                    ))
                };
            }
            let key = feedback_key(&transaction, &feedback)?;
            let kind = json!(feedback.kind).to_string();
            let duplicate: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM feedback WHERE artifact_id=? AND kind=?)",
                params![feedback.artifact_id, kind],
                |row| row.get(0),
            )?;
            if duplicate {
                return Err(EngineError::new(
                    "feedback",
                    "artifact already has feedback of this kind",
                ));
            }
            transaction.execute(
                "INSERT INTO feedback(feedback_id,task,artifact_id,kind,key_json,accepted,payload)
                 VALUES(?,?,?,?,?,?,?)",
                params![
                    feedback.feedback_id,
                    feedback.task_id,
                    feedback.artifact_id,
                    kind,
                    json!(key).to_string(),
                    feedback.accepted,
                    payload,
                ],
            )?;
            audit(
                &transaction,
                &feedback.task_id,
                "feedback_recorded",
                json!(feedback),
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }
}

fn validate_evaluation(connection: &Connection, record: &EvaluationRecord) -> Result<()> {
    if digest(&record.artifact.text) != record.artifact.checksum {
        return Err(EngineError::new("evaluation", "artifact checksum mismatch"));
    }
    let status: String = connection.query_row(
        "SELECT status FROM tasks WHERE id=?",
        [&record.task_id],
        |row| row.get(0),
    )?;
    if status != "running" {
        return Err(EngineError::new(
            "evaluation",
            "evaluation requires a running task",
        ));
    }
    profile_key(
        connection,
        &record.task_id,
        &record.artifact.attempt_id,
        false,
        "",
    )?;
    if let Some(critic) = &record.critic {
        profile_key(connection, &record.task_id, &critic.attempt_id, true, "")?;
    }
    let existing: Option<String> = connection
        .query_row(
            "SELECT record FROM evaluations WHERE artifact_id=?",
            [&record.artifact.artifact_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        let old: EvaluationRecord = serde_json::from_str(&existing)
            .map_err(|_| EngineError::new("storage", "invalid evaluation record"))?;
        if json!(old) == json!(record) {
            return Ok(());
        }
        // A saved candidate is immutable; only its previously absent critic verdict may be added.
        if old.task_id != record.task_id
            || json!(old.artifact) != json!(record.artifact)
            || json!(old.deterministic) != json!(record.deterministic)
            || old.critic.is_some()
        {
            return Err(EngineError::new(
                "evaluation",
                "conflicting evaluation evidence",
            ));
        }
    }
    Ok(())
}

fn feedback_key(connection: &Connection, feedback: &Feedback) -> Result<ProfileKey> {
    let row: Option<(String, String, String)> = connection
        .query_row(
            "SELECT e.record,t.status,t.plan FROM evaluations e
         JOIN tasks t ON t.id=e.task WHERE e.artifact_id=? AND e.task=?",
            params![feedback.artifact_id, feedback.task_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let (record, status, plan) =
        row.ok_or_else(|| EngineError::new("feedback", "unknown artifact or wrong task"))?;
    if !matches!(
        status.as_str(),
        "completed" | "human_required" | "failed" | "cancelled"
    ) {
        return Err(EngineError::new(
            "feedback",
            "feedback requires a terminal task",
        ));
    }
    let plan: Value = serde_json::from_str(&plan)
        .map_err(|_| EngineError::new("storage", "invalid saved plan"))?;
    if plan["effective_plan"]["acceptance"]["version"] != feedback.evaluator_version {
        return Err(EngineError::new(
            "feedback",
            "evaluator version does not match the task",
        ));
    }
    let record: EvaluationRecord = serde_json::from_str(&record)
        .map_err(|_| EngineError::new("storage", "invalid saved evaluation"))?;
    let critic = feedback.kind == FeedbackKind::CriticCorrectness;
    let attempt = if critic {
        &record
            .critic
            .as_ref()
            .ok_or_else(|| EngineError::new("feedback", "artifact has no valid critic verdict"))?
            .attempt_id
    } else {
        &record.artifact.attempt_id
    };
    profile_key(
        connection,
        &feedback.task_id,
        attempt,
        critic,
        &feedback.evaluator_version,
    )
}

fn profile_key(
    connection: &Connection,
    task: &str,
    attempt: &str,
    critic: bool,
    evaluator_version: &str,
) -> Result<ProfileKey> {
    let raw: Option<String> = connection
        .query_row(
            "SELECT metadata FROM attempts WHERE task=? AND id=?",
            params![task, attempt],
            |row| row.get(0),
        )
        .optional()?;
    let metadata: Value = serde_json::from_str(
        &raw.ok_or_else(|| EngineError::new("evaluation", "unknown attempt"))?,
    )
    .map_err(|_| EngineError::new("storage", "invalid attempt metadata"))?;
    let route = &metadata["route"];
    let node = route["node_id"].as_str().unwrap_or("");
    if (critic && node != "critic") || (!critic && !matches!(node, "invoke" | "generator")) {
        return Err(EngineError::new(
            "evaluation",
            "attempt role does not match feedback target",
        ));
    }
    serde_json::from_value(json!({
        "model_id": route["model_id"], "model_version": route["model_version"],
        "task_type": route["quality"]["requested"]["task_type"],
        "role": route["quality"]["requested"]["role"],
        "evaluator_version": evaluator_version,
    }))
    .map_err(|_| EngineError::new("evaluation", "attempt lacks versioned routing evidence"))
}
