//! Host-owned quality and value preferences for one task.
use super::{EngineError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionPolicy {
    pub version: String,
    pub min_quality: f64,
    pub target_quality: f64,
    pub above_target_factor: f64,
    pub cost_reference: u64,
    pub latency_reference_ms: u64,
    pub min_upgrade_gain: f64,
}

impl SelectionPolicy {
    pub fn validate(&self) -> Result<()> {
        let finite = [
            self.min_quality,
            self.target_quality,
            self.above_target_factor,
            self.min_upgrade_gain,
        ]
        .iter()
        .all(|value| value.is_finite());
        if self.version.trim().is_empty()
            || self.version.len() > 256
            || !finite
            || !(0.0..=1.0).contains(&self.min_quality)
            || !(self.min_quality..=1.0).contains(&self.target_quality)
            || !(0.0..=1.0).contains(&self.above_target_factor)
            || !(0.0 < self.min_upgrade_gain && self.min_upgrade_gain <= 1.0)
            || self.cost_reference == 0
            || self.cost_reference > i64::MAX as u64
            || !(1..=86_400_000).contains(&self.latency_reference_ms)
        {
            return Err(EngineError::new(
                "configuration",
                "invalid selection policy",
            ));
        }
        Ok(())
    }

    pub fn quality_value(&self, quality: f64) -> f64 {
        quality.min(self.target_quality)
            + self.above_target_factor * (quality - self.target_quality).max(0.0)
    }
}
