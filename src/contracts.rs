use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

mod planning;
pub use planning::{
    EffectivePlan, PlanChoice, PlannerProposal, PlanningConfig, PlanningMode, RoleProfile,
    SubmissionSpec, TaskType,
};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub id: String,
    pub model: String,
    pub version: String,
    pub endpoint: Endpoint,
    pub base_url: String,
    pub api_key_env: String,
    pub provider: String,
    pub region: String,
    pub local: bool,
    pub capabilities: BTreeSet<String>,
    pub context_tokens: u64,
    /// Integer microcredits per token; no floating point money.
    pub input_price: u64,
    pub output_price: u64,
    pub price_version: String,
    pub acceptance: f64,
    pub reliability: f64,
    pub latency_ms: u64,
    pub uncertainty: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Weights {
    pub quality: f64,
    pub capability: f64,
    pub reliability: f64,
    pub cost: f64,
    pub latency: f64,
    pub uncertainty: f64,
    pub version: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub database_path: String,
    pub models: Vec<Model>,
    /// Explicit candidate IDs for planning. Empty disables model-based planning.
    #[serde(default)]
    pub planner_models: BTreeSet<String>,
    pub weights: Weights,
    pub max_concurrency: usize,
    pub event_capacity: usize,
    pub event_max_bytes: usize,
    pub stream_frame_max_bytes: usize,
    pub context_max_bytes: usize,
    pub close_grace_ms: u64,
    pub cleanup_timeout_ms: u64,
}
impl Config {
    pub fn validate(&self, sqlite: bool) -> Result<()> {
        let bad = |s| EngineError::new("configuration", s);
        if sqlite && (self.database_path.trim().is_empty() || self.database_path == ":memory:") {
            return Err(bad("database_path must name a file"));
        }
        if self.models.is_empty() {
            return Err(bad("models must not be empty"));
        }
        if self.close_grace_ms > 86_400_000 || self.cleanup_timeout_ms > 86_400_000 {
            return Err(bad("shutdown timeouts must be between zero and one day"));
        }
        if !(1..=1024).contains(&self.max_concurrency)
            || !(1..=65536).contains(&self.event_capacity)
            || !(1024..=1048576).contains(&self.event_max_bytes)
            || !(1024..=4194304).contains(&self.stream_frame_max_bytes)
            || !(1024..=16777216).contains(&self.context_max_bytes)
        {
            return Err(bad("invalid concurrency or buffer bounds"));
        }
        let w = &self.weights;
        if w.version.is_empty()
            || [
                w.quality,
                w.capability,
                w.reliability,
                w.cost,
                w.latency,
                w.uncertainty,
            ]
            .iter()
            .any(|x| !x.is_finite() || *x < 0.0)
            || w.quality + w.capability + w.reliability + w.cost + w.latency + w.uncertainty == 0.0
        {
            return Err(bad("weights must be finite, nonnegative and versioned"));
        }
        let mut ids = BTreeSet::new();
        for m in &self.models {
            if m.id.is_empty()
                || !ids.insert(&m.id)
                || m.model.is_empty()
                || m.version.is_empty()
                || m.price_version.is_empty()
                || m.api_key_env.is_empty()
            {
                return Err(bad("model identifiers, credential references and versions must be nonempty and unique"));
            }
            let url =
                reqwest::Url::parse(&m.base_url).map_err(|_| bad("invalid model base_url"))?;
            if !matches!(url.scheme(), "https" | "http")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(bad(
                    "base_url must be HTTP(S) without credentials, query or fragment",
                ));
            }
            if m.context_tokens == 0
                || m.context_tokens > 100_000_000
                || m.input_price > 1_000_000_000
                || m.output_price > 1_000_000_000
                || [m.acceptance, m.reliability, m.uncertainty]
                    .iter()
                    .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            {
                return Err(bad("invalid model capacity, price or metrics"));
            }
            if m.capabilities
                .iter()
                .any(|c| !matches!(c.as_str(), "text" | "tools" | "json"))
            {
                return Err(bad(
                    "phase one supports text, tools and json capabilities only",
                ));
            }
        }
        if self.planner_models.iter().any(|id| !ids.contains(id)) {
            return Err(bad("planner_models must reference configured model IDs"));
        }
        Ok(())
    }
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
