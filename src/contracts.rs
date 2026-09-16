use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

mod configuration;
pub use configuration::{Config, Model, Weights};

mod feedback;
mod planning;
mod profiles;
mod rankings;
mod selection;
pub use feedback::{
    CallStatistics, CriticVerdict, EvaluationRecord, Feedback, FeedbackKind, MetricsSnapshot,
    ProfileKey, QualityStatistics, TaskStatistics,
};
pub use planning::{
    EffectivePlan, PlanChoice, PlannerProposal, PlanningConfig, PlanningMode, RoleProfile,
    SubmissionSpec, TaskType,
};
pub use profiles::{QualityProfile, RoleMapping, RoutingProfiles};
pub use rankings::{RankingConfig, RankingEntry};
pub use selection::SelectionPolicy;

pub type Result<T> = std::result::Result<T, EngineError>;

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{kind}: {message}")]
pub struct EngineError {
    pub kind: String,
    pub message: String,
    pub task_id: Option<String>,
    pub details: Value,
}
impl EngineError {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            task_id: None,
            details: Value::Null,
        }
    }
    pub fn task(mut self, id: &str) -> Self {
        self.task_id = Some(id.into());
        self
    }
    pub fn details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }
}
impl From<rusqlite::Error> for EngineError {
    fn from(_: rusqlite::Error) -> Self {
        Self::new("storage", "database operation failed")
    }
}
pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn digest(value: &impl Serialize) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("serializable contract"))
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Endpoint {
    ChatCompletions,
    Responses,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    Single,
    Cascade,
    GeneratorCritic,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Constraints {
    pub allowed_models: BTreeSet<String>,
    pub allowed_providers: BTreeSet<String>,
    pub allowed_regions: BTreeSet<String>,
    pub local_only: bool,
    pub required_capabilities: BTreeSet<String>,
    pub preferred_capabilities: BTreeSet<String>,
    pub different_critic: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acceptance {
    pub version: String,
    pub nonempty: bool,
    pub required_substrings: Vec<String>,
    pub json_object: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub max_cost: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub source: String,
    pub text: String,
    pub trusted: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    pub task_id: String,
    pub goal: String,
    pub evidence: Vec<Evidence>,
    pub strategy: Strategy,
    pub acceptance: Acceptance,
    pub selection: SelectionPolicy,
    pub constraints: Constraints,
    pub budget: u64,
    pub deadline_ms: u64,
    pub output_tokens: u64,
    pub max_calls: u32,
    pub max_rounds: u32,
    pub max_attempts: u32,
    pub max_call_cost: u64,
    pub finalization_ms: u64,
    pub tools: Vec<ToolSpec>,
}
impl TaskSpec {
    pub fn validate(&self, config: &Config) -> Result<()> {
        self.selection.validate()?;
        if self.task_id.is_empty()
            || self.goal.trim().is_empty()
            || self.acceptance.version.is_empty()
            || self.budget > i64::MAX as u64
            || self.max_call_cost > i64::MAX as u64
            || self.max_calls == 0
            || self.max_calls > 1000
            || self.max_rounds == 0
            || self.max_rounds > 100
            || self.max_attempts == 0
            || self.max_attempts > 100
            || self.output_tokens == 0
            || self.output_tokens > 1_000_000
            || self.deadline_ms <= now_ms().saturating_add(self.finalization_ms)
        {
            return Err(EngineError::new(
                "configuration",
                "invalid task identifiers, resource bounds or deadline",
            ));
        }
        if serde_json::to_vec(self).unwrap().len() > config.context_max_bytes {
            return Err(EngineError::new(
                "configuration",
                "task exceeds context byte limit",
            ));
        }
        let mut names = BTreeSet::new();
        for t in &self.tools {
            if t.name.is_empty()
                || !names.insert(&t.name)
                || !t.parameters.is_object()
                || t.max_cost > self.max_call_cost
            {
                return Err(EngineError::new(
                    "configuration",
                    "invalid tool declaration",
                ));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub version: u32,
    pub goal: String,
    pub acceptance: Acceptance,
    pub evidence: Vec<Evidence>,
    pub role: String,
    pub prior_artifact: Option<String>,
    pub feedback: Vec<String>,
    pub checksum: String,
}
impl Snapshot {
    pub fn new(
        task: &TaskSpec,
        role: &str,
        prior_artifact: Option<String>,
        feedback: Vec<String>,
        version: u32,
    ) -> Self {
        let mut s = Self {
            id: id(),
            version,
            goal: task.goal.clone(),
            acceptance: task.acceptance.clone(),
            evidence: task.evidence.clone(),
            role: role.into(),
            prior_artifact,
            feedback,
            checksum: String::new(),
        };
        s.checksum = digest(&s);
        s
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub artifact_id: String,
    pub version: u32,
    pub attempt_id: String,
    pub text: String,
    pub checksum: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationStatus {
    Pass,
    Revise,
    MissingEvidence,
    Unacceptable,
    HumanRequired,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evaluation {
    pub status: EvaluationStatus,
    pub checks: Vec<String>,
    pub evidence: Vec<String>,
    pub defects: Vec<String>,
    pub action: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: String,
    pub status: String,
    pub artifact: Artifact,
    pub settled_cost: u64,
    pub reserved_cost: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolResult {
    pub output: String,
    pub actual_cost: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}
