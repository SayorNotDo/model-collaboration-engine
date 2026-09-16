//! Explicit external ranking references, separate from business quality evidence.
use super::{Config, EngineError, Result, TaskType};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RankingEntry {
    pub model_id: String,
    pub model_version: String,
    pub source_model: String,
    pub source_model_version: String,
    pub task_type: TaskType,
    pub role: String,
    pub rank: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RankingConfig {
    pub schema_version: u32,
    pub version: String,
    pub source: String,
    pub source_version: String,
    pub category: String,
    pub published_at_ms: u64,
    pub expires_at_ms: u64,
    pub population: u32,
    pub weight: f64,
    pub entries: Vec<RankingEntry>,
}
impl RankingConfig {
    /// Validate imported metadata and explicit local mappings without accessing the source.
    pub fn validate(&self, config: &Config) -> Result<()> {
        let bad = |message| EngineError::new("configuration", message);
        if self.schema_version != 1
            || !identity(&self.version, 256)
            || !identity(&self.source, 2048)
            || !identity(&self.source_version, 256)
            || !identity(&self.category, 256)
            || self.published_at_ms > i64::MAX as u64
            || self.expires_at_ms > i64::MAX as u64
            || self.expires_at_ms <= self.published_at_ms
            || !(1..=1_000_000).contains(&self.population)
            || !self.weight.is_finite()
            || !(0.0..=1.0).contains(&self.weight)
            || self.entries.len() > 100_000
        {
            return Err(bad("invalid external ranking metadata or bounds"));
        }
        let models: BTreeSet<_> = config.models.iter().map(|m| m.id.as_str()).collect();
        let mut keys = BTreeSet::new();
        let mut external = BTreeMap::new();
        for entry in &self.entries {
            if [
                &entry.model_id,
                &entry.model_version,
                &entry.source_model,
                &entry.source_model_version,
                &entry.role,
            ]
            .iter()
            .any(|v| !identity(v, 256))
                || !matches!(entry.role.as_str(), "invoke" | "generator" | "critic")
                || !models.contains(entry.model_id.as_str())
                || !(1..=self.population).contains(&entry.rank)
                || !keys.insert((
                    &entry.model_id,
                    &entry.model_version,
                    entry.task_type,
                    &entry.role,
                ))
            {
                return Err(bad(
                    "invalid, unknown or duplicate external ranking mapping",
                ));
            }
            if external
                .insert(
                    (&entry.source_model, &entry.source_model_version),
                    entry.rank,
                )
                .is_some_and(|rank| rank != entry.rank)
            {
                return Err(bad("conflicting ranks for an external model version"));
            }
        }
        if external.len() > self.population as usize {
            return Err(bad("external model count exceeds ranking population"));
        }
        Ok(())
    }
}
fn identity(value: &str, bound: usize) -> bool {
    !value.trim().is_empty() && value.len() <= bound
}
