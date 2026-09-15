//! Versioned submission contracts; legacy TaskSpec remains unchanged.
use super::{
    Acceptance, Config, Constraints, EngineError, Evidence, Result, Strategy, TaskSpec, ToolSpec,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    #[default]
    General,
    CodeGeneration,
    CodeReview,
    InformationExtraction,
    Reasoning,
    Writing,
    ToolExecution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanningMode {
    Disabled,
    Auto,
    Required,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanChoice {
    pub task_type: TaskType,
    pub strategy: Strategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanningConfig {
    pub mode: PlanningMode,
    #[serde(default)]
    pub fallback: Option<PlanChoice>,
    #[serde(default = "one_call")]
    pub max_calls: u32,
    pub max_cost: u64,
    pub timeout_ms: u64,
}
fn one_call() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmissionSpec {
    pub schema_version: u32,
    pub task_id: String,
    pub goal: String,
    pub evidence: Vec<Evidence>,
    #[serde(default)]
    pub task_type: Option<TaskType>,
    #[serde(default)]
    pub strategy: Option<Strategy>,
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
    pub planning: PlanningConfig,
}

impl SubmissionSpec {
    /// Validate before admission. Version one deliberately permits only one planning attempt.
    pub fn validate(&self, config: &Config) -> Result<()> {
        let p = &self.planning;
        if self.schema_version != 1
            || p.max_calls != 1
            || p.max_cost > i64::MAX as u64
            || p.timeout_ms == 0
            || p.timeout_ms > 86_400_000
            || (p.mode == PlanningMode::Disabled && self.strategy.is_none())
        {
            return Err(EngineError::new(
                "configuration",
                "invalid submission version, planning bounds or disabled strategy",
            ));
        }
        if serde_json::to_vec(self)
            .expect("serializable submission")
            .len()
            > config.context_max_bytes
        {
            return Err(EngineError::new(
                "configuration",
                "submission exceeds context byte limit",
            ));
        }
        if self
            .constraints
            .required_capabilities
            .iter()
            .chain(&self.constraints.preferred_capabilities)
            .any(|c| !matches!(c.as_str(), "text" | "json" | "tools"))
        {
            return Err(EngineError::new(
                "configuration",
                "unsupported submission capability",
            ));
        }
        self.task(self.strategy.clone().unwrap_or(Strategy::Single))
            .validate(config)
    }

    pub(crate) fn needs_planning(&self) -> bool {
        match self.planning.mode {
            PlanningMode::Disabled => false,
            PlanningMode::Auto => self.strategy.is_none() || self.task_type.is_none(),
            PlanningMode::Required => true,
        }
    }

    // Provisional TaskSpec is internal only until the validated plan is applied.
    pub(crate) fn task(&self, strategy: Strategy) -> TaskSpec {
        TaskSpec {
            task_id: self.task_id.clone(),
            goal: self.goal.clone(),
            evidence: self.evidence.clone(),
            strategy,
            acceptance: self.acceptance.clone(),
            constraints: self.constraints.clone(),
            budget: self.budget,
            deadline_ms: self.deadline_ms,
            output_tokens: self.output_tokens,
            max_calls: self.max_calls,
            max_rounds: self.max_rounds,
            max_attempts: self.max_attempts,
            max_call_cost: self.max_call_cost,
            finalization_ms: self.finalization_ms,
            tools: self.tools.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannerProposal {
    pub proposal_version: u32,
    pub task_type: TaskType,
    pub classification_confidence: f64,
    pub strategy: Strategy,
    pub required_capabilities: BTreeSet<String>,
    pub suggested_acceptance: Acceptance,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleProfile {
    pub task_type: TaskType,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectivePlan {
    pub plan_version: u32,
    pub task_type: TaskType,
    pub strategy: Strategy,
    pub role_profiles: BTreeMap<String, RoleProfile>,
    pub constraints: Constraints,
    pub acceptance: Acceptance,
    pub submission_hash: String,
    pub proposal_id: Option<String>,
    pub validator_version: String,
    pub routing_profile_version: String,
}

impl EffectivePlan {
    pub(crate) fn task(&self, submission: &SubmissionSpec) -> TaskSpec {
        let mut task = submission.task(self.strategy.clone());
        task.constraints = self.constraints.clone();
        task.acceptance = self.acceptance.clone();
        task
    }
}
