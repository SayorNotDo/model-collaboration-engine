//! Host-owned static quality evidence. No feedback collection or I/O lives here.
use super::{Config, EngineError, Result, RoleProfile, TaskType, Weights};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityProfile {
    pub model_id: String,
    pub model_version: String,
    pub task_type: TaskType,
    pub role: String,
    /// Same evaluation definition as the effective acceptance.version.
    pub evaluator_version: String,
    pub prior: f64,
    pub prior_weight: f64,
    pub accepted: u64,
    pub samples: u64,
}
impl QualityProfile {
    /// Posterior under validated bounds; an empty sample preserves the prior exactly.
    pub fn quality(&self) -> f64 {
        if self.samples == 0 {
            return self.prior;
        }
        (self.prior_weight * self.prior + self.accepted as f64)
            / (self.prior_weight + self.samples as f64)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleMapping {
    pub task_type: TaskType,
    pub node: String,
    pub profile: RoleProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingProfiles {
    pub schema_version: u32,
    /// Versions profiles, parent relationships, role mappings and weights together.
    pub version: String,
    #[serde(default)]
    pub parents: BTreeMap<TaskType, TaskType>,
    #[serde(default)]
    pub role_mappings: Vec<RoleMapping>,
    #[serde(default)]
    pub weights: BTreeMap<TaskType, Weights>,
    #[serde(default)]
    pub profiles: Vec<QualityProfile>,
}
impl Default for RoutingProfiles {
    fn default() -> Self {
        Self {
            schema_version: 1,
            version: "global-prior-v1".into(),
            parents: BTreeMap::new(),
            role_mappings: vec![],
            weights: BTreeMap::new(),
            profiles: vec![],
        }
    }
}
impl RoutingProfiles {
    pub fn validate(&self, config: &Config) -> Result<()> {
        let bad = |s| EngineError::new("configuration", s);
        if self.schema_version != 1 || self.version.trim().is_empty() {
            return Err(bad(
                "routing profiles require schema_version 1 and a version",
            ));
        }
        if self.profiles.len() > 100_000 || self.role_mappings.len() > 21 {
            return Err(bad("routing profile or mapping count exceeds bounds"));
        }
        if self.parents.contains_key(&TaskType::General) {
            return Err(bad("general cannot have a parent"));
        }
        for start in self.parents.keys() {
            let mut seen = BTreeSet::from([*start]);
            let mut current = start;
            while let Some(parent) = self.parents.get(current) {
                if !seen.insert(*parent) {
                    return Err(bad("routing parent inheritance contains a cycle"));
                }
                current = parent;
            }
        }
        let mut mappings = BTreeSet::new();
        for mapping in &self.role_mappings {
            if !valid_role(&mapping.node)
                || !valid_role(&mapping.profile.role)
                || !mappings.insert((mapping.task_type, &mapping.node))
            {
                return Err(bad("invalid or duplicate routing role mapping"));
            }
        }
        for weights in self.weights.values() {
            validate_weights(weights)?;
        }
        self.validate_profiles(config)
    }

    fn validate_profiles(&self, config: &Config) -> Result<()> {
        let bad = |s| EngineError::new("configuration", s);
        let mut keys = BTreeSet::new();
        for p in &self.profiles {
            if !config.models.iter().any(|m| m.id == p.model_id)
                || p.model_version.trim().is_empty()
                || p.evaluator_version.trim().is_empty()
                || !valid_role(&p.role)
                || !keys.insert((
                    &p.model_id,
                    &p.model_version,
                    p.task_type,
                    &p.role,
                    &p.evaluator_version,
                ))
            {
                return Err(bad(
                    "unknown model, invalid key or duplicate quality profile",
                ));
            }
            // Keep integer counts exactly representable and the posterior finite.
            if !p.prior.is_finite()
                || !(0.0..=1.0).contains(&p.prior)
                || !p.prior_weight.is_finite()
                || p.prior_weight <= 0.0
                || p.prior_weight > (1_u64 << 53) as f64
                || p.samples > (1_u64 << 53)
                || p.accepted > p.samples
            {
                return Err(bad("invalid quality prior, prior weight or sample counts"));
            }
        }
        Ok(())
    }
}

fn valid_role(role: &str) -> bool {
    matches!(role, "invoke" | "generator" | "critic")
}

pub(super) fn validate_weights(w: &Weights) -> Result<()> {
    let values = [
        w.quality,
        w.capability,
        w.reliability,
        w.cost,
        w.latency,
        w.uncertainty,
    ];
    let sum: f64 = values.iter().sum();
    if w.version.trim().is_empty()
        || !sum.is_finite()
        || sum <= 0.0
        || values.iter().any(|v| !v.is_finite() || *v < 0.0)
    {
        return Err(EngineError::new(
            "configuration",
            "weights must be finite, nonnegative and versioned",
        ));
    }
    Ok(())
}
