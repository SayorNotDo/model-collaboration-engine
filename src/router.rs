use crate::contracts::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
mod profiles;
mod rankings;
mod selection;
pub use profiles::{FallbackTier, NodeProfile, ResolvedQuality, RoutingSnapshot};
pub use rankings::{RankingStatus, ResolvedRanking};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Health {
    pub calls: u64,
    pub accepted: u64,
    pub failures: u64,
    pub unavailable_until: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub decision_id: String,
    pub node_id: String,
    pub model_id: String,
    pub model_version: String,
    pub estimated_cost: u64,
    pub estimated_input: u64,
    pub estimated_latency_ms: u64,
    pub score: f64,
    pub breakdown: BTreeMap<String, f64>,
    pub excluded: BTreeMap<String, Vec<String>>,
    pub weights_version: String,
    pub metrics_snapshot: String,
    pub uncertainty: f64,
    #[serde(default)]
    pub quality: Option<ResolvedQuality>,
    #[serde(default)]
    pub ranking: Option<ResolvedRanking>,
    #[serde(default)]
    pub routing_snapshot: Option<String>,
}
pub struct RouteRequest<'a> {
    pub task: &'a TaskSpec,
    pub node: &'a str,
    pub input_tokens: u64,
    pub available: u64,
    pub excluded: &'a BTreeSet<String>,
    pub quality_floor: f64,
    pub health: &'a BTreeMap<String, Health>,
}
pub fn route(config: &Config, r: RouteRequest<'_>) -> Result<RoutingDecision> {
    route_inner(
        &config.models,
        &config.weights,
        r,
        None,
        None,
        false,
        now_ms(),
    )
}

/// Re-evaluate a stored snapshot and recorded RouteRequest without refreshing profiles.
/// Decision IDs are new audit identities; selection, score and evidence are reproducible.
pub fn route_profiled(snapshot: &RoutingSnapshot, r: RouteRequest<'_>) -> Result<RoutingDecision> {
    if !matches!(snapshot.schema_version, 1..=4) {
        return Err(EngineError::new(
            "routing",
            "unsupported routing snapshot version",
        ));
    }
    if snapshot.schema_version >= 4
        && (snapshot.selection.is_none()
            || snapshot.algorithm_version.as_deref() != Some("task-fit-v1"))
    {
        return Err(EngineError::new(
            "routing",
            "task-fit routing snapshot lacks supported algorithm inputs",
        ));
    }
    let node = snapshot
        .nodes
        .get(r.node)
        .ok_or_else(|| EngineError::new("routing", "snapshot has no profile for this node"))?;
    let task_fit = snapshot.schema_version >= 4 && r.node != "critic";
    route_inner(
        &snapshot.models,
        &node.weights,
        r,
        Some((snapshot, node)),
        task_fit.then_some(snapshot.selection.as_ref()).flatten(),
        snapshot.schema_version >= 4,
        snapshot.captured_at_ms,
    )
}

fn route_inner(
    models: &[Model],
    weights: &Weights,
    r: RouteRequest<'_>,
    profile: Option<(&RoutingSnapshot, &NodeProfile)>,
    selection_policy: Option<&SelectionPolicy>,
    task_fit_algorithm: bool,
    at_ms: u64,
) -> Result<RoutingDecision> {
    let mut rejected = BTreeMap::new();
    let mut candidates = vec![];
    let routing_snapshot = profile.map(|(snapshot, _)| digest(snapshot));
    for m in models {
        let resolved = profile.and_then(|(_, node)| node.qualities.get(&m.id));
        if profile.is_some() && resolved.is_none() {
            return Err(EngineError::new(
                "routing",
                "snapshot lacks model quality evidence",
            ));
        }
        let ranking = profile.and_then(|(snapshot, node)| {
            (snapshot.schema_version >= 3)
                .then(|| node.rankings.get(&m.id))
                .flatten()
        });
        if profile.is_some_and(|(snapshot, _)| snapshot.schema_version >= 3) && ranking.is_none() {
            return Err(EngineError::new(
                "routing",
                "snapshot lacks model ranking evidence",
            ));
        }
        let h = r.health.get(&m.id).cloned().unwrap_or_default();
        let acceptance = resolved.map(|p| p.quality).unwrap_or(m.acceptance);
        let mut reasons = vec![];
        let c = &r.task.constraints;
        let cost = r
            .input_tokens
            .saturating_mul(m.input_price)
            .saturating_add(r.task.output_tokens.saturating_mul(m.output_price));
        let required = c
            .required_capabilities
            .iter()
            .map(String::as_str)
            .chain(std::iter::once("text"))
            .chain((!r.task.tools.is_empty() && r.node != "critic").then_some("tools"))
            .chain((r.task.acceptance.json_object || r.node == "critic").then_some("json"));
        if required
            .into_iter()
            .any(|cap| !m.capabilities.contains(cap))
        {
            reasons.push("capabilities".into());
        }
        if r.input_tokens
            .saturating_add(r.task.output_tokens)
            .saturating_add(256)
            > m.context_tokens
        {
            reasons.push("context".into());
        }
        if !c.allowed_models.is_empty() && !c.allowed_models.contains(&m.id) {
            reasons.push("allowed_models".into());
        }
        if !c.allowed_providers.is_empty() && !c.allowed_providers.contains(&m.provider) {
            reasons.push("provider".into());
        }
        if !c.allowed_regions.is_empty() && !c.allowed_regions.contains(&m.region) {
            reasons.push("region".into());
        }
        if c.local_only && !m.local {
            reasons.push("local_only".into());
        }
        if cost > r.available || cost > r.task.max_call_cost {
            reasons.push("budget".into());
        }
        if r.excluded.contains(&m.id) {
            reasons.push("attempt_history_or_diversity".into());
        }
        let required_quality = selection_policy.map_or(r.quality_floor, |policy| {
            r.quality_floor.max(policy.min_quality)
        });
        if resolved.map(|p| p.quality).unwrap_or(m.acceptance) < required_quality {
            reasons.push("quality_floor".into());
        }
        if h.unavailable_until > at_ms {
            reasons.push("unavailable".into());
        }
        if !reasons.is_empty() {
            rejected.insert(m.id.clone(), reasons);
            continue;
        }
        let reliability = (m.reliability * 10.0 + h.calls.saturating_sub(h.failures) as f64)
            / (10.0 + h.calls as f64);
        let capability = if c.preferred_capabilities.is_empty() {
            0.0
        } else {
            c.preferred_capabilities
                .intersection(&m.capabilities)
                .count() as f64
                / c.preferred_capabilities.len() as f64
        };
        let w = weights;
        let mut parts = if let Some(policy) = selection_policy {
            selection::primary_parts(
                w,
                policy,
                selection::Inputs {
                    quality: acceptance,
                    capability,
                    reliability,
                    cost,
                    latency_ms: m.latency_ms,
                    uncertainty: m.uncertainty,
                },
            )
        } else {
            BTreeMap::from([
                ("quality".into(), w.quality * acceptance),
                ("capability".into(), w.capability * capability),
                ("reliability".into(), w.reliability * reliability),
                (
                    "cost".into(),
                    -w.cost * cost as f64 / r.task.max_call_cost.max(1) as f64,
                ),
                (
                    "latency".into(),
                    -w.latency
                        * (m.latency_ms as f64
                            / r.task.deadline_ms.saturating_sub(at_ms).max(1) as f64)
                            .min(1.0),
                ),
                ("uncertainty".into(), -w.uncertainty * m.uncertainty),
            ])
        };
        let primary_score: f64 = parts.values().sum();
        if let Some(ranking) = ranking {
            parts.insert("ranking".into(), ranking.contribution);
        }
        candidates.push(RoutingDecision {
            decision_id: id(),
            node_id: r.node.into(),
            model_id: m.id.clone(),
            model_version: m.version.clone(),
            estimated_cost: cost,
            estimated_input: r.input_tokens,
            estimated_latency_ms: m.latency_ms,
            score: if task_fit_algorithm {
                primary_score
            } else {
                parts.values().sum()
            },
            breakdown: parts,
            excluded: BTreeMap::new(),
            weights_version: w.version.clone(),
            metrics_snapshot: digest(&r.health),
            uncertainty: m.uncertainty,
            quality: resolved.cloned(),
            ranking: ranking.cloned(),
            routing_snapshot: routing_snapshot.clone(),
        });
    }
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| {
                if task_fit_algorithm {
                    b.ranking
                        .as_ref()
                        .map_or(0.0, |ranking| ranking.contribution)
                        .total_cmp(
                            &a.ranking
                                .as_ref()
                                .map_or(0.0, |ranking| ranking.contribution),
                        )
                        .then(a.estimated_cost.cmp(&b.estimated_cost))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then(a.model_id.cmp(&b.model_id))
    });
    let mut chosen = candidates.into_iter().next().ok_or_else(|| {
        EngineError::new("routing", "no model satisfies all hard constraints")
            .details(json!({"category":"no_candidate","excluded":rejected}))
    })?;
    chosen.excluded = rejected;
    Ok(chosen)
}
