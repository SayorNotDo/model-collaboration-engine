//! Host acceptance is distinct from execution and critic verdicts.
use super::{Artifact, EngineError, Evaluation, Result, TaskType};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackKind {
    BusinessAcceptance,
    CriticCorrectness,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Feedback {
    pub feedback_id: String,
    pub task_id: String,
    pub artifact_id: String,
    pub kind: FeedbackKind,
    pub evaluator_version: String,
    pub accepted: bool,
    pub reason: String,
}
impl Feedback {
    pub fn validate(&self) -> Result<()> {
        if [
            &self.feedback_id,
            &self.task_id,
            &self.artifact_id,
            &self.evaluator_version,
        ]
        .iter()
        .any(|v| v.trim().is_empty() || v.len() > 256)
            || self.reason.len() > 4096
        {
            return Err(EngineError::new(
                "feedback",
                "invalid feedback identifiers or reason size",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriticVerdict {
    pub attempt_id: String,
    pub evaluation: Evaluation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationRecord {
    pub task_id: String,
    pub artifact: Artifact,
    pub deterministic: Evaluation,
    pub critic: Option<CriticVerdict>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileKey {
    pub model_id: String,
    pub model_version: String,
    pub task_type: TaskType,
    pub role: String,
    pub evaluator_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityStatistics {
    pub key: ProfileKey,
    pub kind: FeedbackKind,
    pub accepted: u64,
    pub samples: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallStatistics {
    /// model, tool, or unknown for historical evidence without attribution.
    pub attempt_kind: String,
    pub tool_name: Option<String>,
    pub model_id: String,
    pub model_version: String,
    pub config_hash: String,
    pub node: String,
    pub attempts: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub timed_out: u64,
    pub not_dispatched: u64,
    pub unknown_status: u64,
    pub known_cost: u64,
    pub unresolved_reserved: u64,
    pub unknown_cost_attempts: u64,
    pub latency_samples: u64,
    pub mean_latency_ms: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskStatistics {
    pub total: u64,
    pub completed: u64,
    pub human_required: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub running: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub revision: u64,
    pub captured_at_ms: u64,
    pub quality: Vec<QualityStatistics>,
    pub calls: Vec<CallStatistics>,
    pub tasks: TaskStatistics,
}
