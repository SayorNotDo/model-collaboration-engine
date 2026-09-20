//! Pure resolution and serializable, fixed per-submission routing inputs.
use super::rankings::{self, ResolvedRanking};
use crate::assessment::ExecutionClass;
use crate::contracts::{
    digest, Config, EffectivePlan, FeedbackKind, MetricsSnapshot, Model, QualityProfile,
    RankingConfig, RoleProfile, RoutingProfiles, SelectionPolicy, TaskType, Weights,
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rankings: BTreeMap<String, ResolvedRanking>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingSnapshot {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SelectionPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algorithm_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranking_config: Option<RankingConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranking_config_hash: Option<String>,
    #[serde(default)]
    pub feedback_revision: u64,
    #[serde(default)]
    pub feedback_hash: String,
    pub profile_version: String,
    pub profile_config_hash: String,
    pub evaluator_version: String,
    /// Fixes latency normalization. Live deadlines still gate every dispatch.
    pub captured_at_ms: u64,
    pub models: Vec<Model>,
    pub nodes: BTreeMap<String, NodeProfile>,
    #[serde(default)]
    pub minimum_execution_class: ExecutionClass,
}

impl RoutingSnapshot {
    /// Capture only after Config validation and deterministic plan validation.
    /// The returned snapshot is never refreshed during execution.
    pub fn capture(config: &Config, plan: &mut EffectivePlan, at_ms: u64) -> Self {
        Self::capture_with_metrics(config, plan, at_ms, &MetricsSnapshot::default())
    }
    pub fn capture_with_metrics(
        config: &Config,
        plan: &mut EffectivePlan,
        at_ms: u64,
        metrics: &MetricsSnapshot,
    ) -> Self {
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
                            resolve(
                                profiles,
                                model,
                                role,
                                &plan.acceptance.version,
                                metrics,
                                if node == "critic" {
                                    FeedbackKind::CriticCorrectness
                                } else {
                                    FeedbackKind::BusinessAcceptance
                                },
                            ),
                        )
                    })
                    .collect();
                (
                    node.clone(),
                    NodeProfile {
                        weights: selected.map(|(_, w)| w).unwrap_or(&config.weights).clone(),
                        weights_task_type: selected.map(|(kind, _)| kind),
                        qualities,
                        rankings: config
                            .models
                            .iter()
                            .map(|model| {
                                (
                                    model.id.clone(),
                                    rankings::resolve(config.rankings.as_ref(), model, role, at_ms),
                                )
                            })
                            .collect(),
                    },
                )
            })
            .collect();
        Self {
            schema_version: 4,
            selection: plan.selection.clone(),
            algorithm_version: Some("task-fit-v1".into()),
            ranking_config: config.rankings.clone(),
            ranking_config_hash: config.rankings.as_ref().map(digest),
            feedback_revision: metrics.revision,
            feedback_hash: digest(&metrics.quality),
            profile_version: profiles.version.clone(),
            profile_config_hash: digest(profiles),
            evaluator_version: plan.acceptance.version.clone(),
            captured_at_ms: at_ms,
            models: config.models.clone(),
            nodes,
            minimum_execution_class: plan
                .assessment
                .as_ref()
                .map_or(ExecutionClass::Simple, |assessment| {
                    assessment.execution_class
                }),
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
    metrics: &MetricsSnapshot,
    feedback_kind: FeedbackKind,
) -> ResolvedQuality {
    for (kind, tier) in lineage(profiles, role.task_type) {
        let configured = profiles.profiles.iter().find(|p| {
            p.model_id == model.id
                && p.model_version == model.version
                && p.task_type == kind
                && p.role == role.role
                && p.evaluator_version == evaluator
        });
        let live = metrics.quality.iter().find(|s| {
            s.kind == feedback_kind
                && s.key.model_id == model.id
                && s.key.model_version == model.version
                && s.key.task_type == kind
                && s.key.role == role.role
                && s.key.evaluator_version == evaluator
        });
        let profile = live
            .map(|s| QualityProfile {
                model_id: model.id.clone(),
                model_version: model.version.clone(),
                task_type: kind,
                role: role.role.clone(),
                evaluator_version: evaluator.to_owned(),
                prior: configured.map_or(model.acceptance, |p| p.prior),
                prior_weight: configured.map_or(10.0, |p| p.prior_weight),
                accepted: s.accepted,
                samples: s.samples,
            })
            .or_else(|| configured.cloned());
        if let Some(profile) = profile {
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
