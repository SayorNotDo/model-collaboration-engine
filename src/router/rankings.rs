//! Resolve once at admission; execution never refreshes external ranking evidence.
use crate::contracts::{Model, RankingConfig, RankingEntry, RoleProfile};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RankingStatus {
    Applied,
    Disabled,
    NotYetEffective,
    Expired,
    Missing,
    ModelVersionMismatch,
    TaskTypeMismatch,
    RoleMismatch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedRanking {
    pub status: RankingStatus,
    pub entry: Option<RankingEntry>,
    pub normalized_rank: Option<f64>,
    pub contribution: f64,
}

pub(super) fn resolve(
    config: Option<&RankingConfig>,
    model: &Model,
    role: &RoleProfile,
    at_ms: u64,
) -> ResolvedRanking {
    let mut resolved = ResolvedRanking {
        status: RankingStatus::Disabled,
        entry: None,
        normalized_rank: None,
        contribution: 0.0,
    };
    let Some(config) = config else {
        return resolved;
    };
    let candidates: Vec<_> = config
        .entries
        .iter()
        .filter(|e| e.model_id == model.id)
        .collect();
    let exact = candidates.iter().find(|e| {
        e.model_version == model.version && e.task_type == role.task_type && e.role == role.role
    });
    resolved.entry = exact.map(|e| (*e).clone());
    resolved.status = if config.weight == 0.0 {
        RankingStatus::Disabled
    } else if at_ms < config.published_at_ms {
        RankingStatus::NotYetEffective
    } else if at_ms >= config.expires_at_ms {
        RankingStatus::Expired
    } else if candidates.is_empty() {
        RankingStatus::Missing
    } else if !candidates.iter().any(|e| e.model_version == model.version) {
        RankingStatus::ModelVersionMismatch
    } else if !candidates
        .iter()
        .any(|e| e.model_version == model.version && e.task_type == role.task_type)
    {
        RankingStatus::TaskTypeMismatch
    } else if exact.is_none() {
        RankingStatus::RoleMismatch
    } else {
        RankingStatus::Applied
    };
    if resolved.status == RankingStatus::Applied {
        if let Some(entry) = &resolved.entry {
            let normalized = if config.population == 1 {
                1.0
            } else {
                f64::from(config.population - entry.rank) / f64::from(config.population - 1)
            };
            resolved.normalized_rank = Some(normalized);
            resolved.contribution = config.weight * normalized;
        }
    }
    resolved
}
