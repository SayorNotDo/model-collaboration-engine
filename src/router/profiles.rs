//! Pure resolution and serializable, fixed per-submission routing inputs.
use crate::contracts::{
    digest, Config, EffectivePlan, Model, QualityProfile, RoleProfile, RoutingProfiles, TaskType,
    Weights,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackTier {
    Exact,
    Parent,
    General,
    GlobalPrior,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedQuality {
    pub quality: f64,
    pub fallback: FallbackTier,
    pub requested: RoleProfile,
    pub matched_type: Option<TaskType>,
    pub evidence: Option<QualityProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeProfile {
    pub weights: Weights,
    pub weights_task_type: Option<TaskType>,
    pub qualities: BTreeMap<String, ResolvedQuality>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingSnapshot {
    pub schema_version: u32,
    pub profile_version: String,
    pub profile_config_hash: String,
    pub evaluator_version: String,
    /// Fixes latency normalization. Live deadlines still gate every dispatch.
    pub captured_at_ms: u64,
    pub models: Vec<Model>,
    pub nodes: BTreeMap<String, NodeProfile>,
}

impl RoutingSnapshot {
    /// Capture only after Config validation and deterministic plan validation.
    /// The returned snapshot is never refreshed during execution.
    pub fn capture(config: &Config, plan: &mut EffectivePlan, at_ms: u64) -> Self {
        let defaults = RoutingProfiles::default();
        let profiles = config.routing_profiles.as_ref().unwrap_or(&defaults);
        for mapping in &profiles.role_mappings {
            if mapping.task_type == plan.task_type {
                if let Some(role) = plan.role_profiles.get_mut(&mapping.node) {
                    *role = mapping.profile.clone();
                }
            }
        }
        plan.routing_profile_version = profiles.version.clone();
        let nodes = plan
            .role_profiles
            .iter()
            .map(|(node, role)| {
                let lineage = lineage(profiles, role.task_type);
                let selected = lineage
                    .iter()
                    .find_map(|(kind, _)| profiles.weights.get(kind).map(|w| (*kind, w)));
                let qualities = config
                    .models
                    .iter()
                    .map(|model| {
                        (
                            model.id.clone(),
                            resolve(profiles, model, role, &plan.acceptance.version),
                        )
                    })
                    .collect();
                (
                    node.clone(),
                    NodeProfile {
                        weights: selected.map(|(_, w)| w).unwrap_or(&config.weights).clone(),
                        weights_task_type: selected.map(|(kind, _)| kind),
                        qualities,
                    },
                )
            })
            .collect();
        Self {
            schema_version: 1,
            profile_version: profiles.version.clone(),
            profile_config_hash: digest(profiles),
            evaluator_version: plan.acceptance.version.clone(),
            captured_at_ms: at_ms,
            models: config.models.clone(),
            nodes,
        }
    }
}

fn lineage(profiles: &RoutingProfiles, kind: TaskType) -> Vec<(TaskType, FallbackTier)> {
    let mut chain = vec![(kind, FallbackTier::Exact)];
    let mut current = kind;
    while let Some(parent) = profiles.parents.get(&current) {
        // Defensive bound for callers using capture without Config::validate.
        if chain.iter().any(|(seen, _)| seen == parent) {
            break;
        }
        chain.push((
            *parent,
            if *parent == TaskType::General {
                FallbackTier::General
            } else {
                FallbackTier::Parent
            },
        ));
        current = *parent;
    }
    if !chain.iter().any(|(kind, _)| *kind == TaskType::General) {
        chain.push((TaskType::General, FallbackTier::General));
    }
    chain
}

fn resolve(
    profiles: &RoutingProfiles,
    model: &Model,
    role: &RoleProfile,
    evaluator: &str,
) -> ResolvedQuality {
    for (kind, tier) in lineage(profiles, role.task_type) {
        if let Some(profile) = profiles.profiles.iter().find(|p| {
            p.model_id == model.id
                && p.model_version == model.version
                && p.task_type == kind
                && p.role == role.role
                && p.evaluator_version == evaluator
        }) {
            return ResolvedQuality {
                quality: profile.quality(),
                fallback: tier,
                requested: role.clone(),
                matched_type: Some(kind),
                evidence: Some(profile.clone()),
            };
        }
    }
    ResolvedQuality {
        quality: model.acceptance,
        fallback: FallbackTier::GlobalPrior,
        requested: role.clone(),
        matched_type: None,
        evidence: None,
    }
}
