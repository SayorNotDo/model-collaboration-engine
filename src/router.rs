use crate::contracts::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

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
    let mut rejected = BTreeMap::new();
    let mut candidates = vec![];
    for m in &config.models {
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
            .chain((!r.task.tools.is_empty()).then_some("tools"))
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
        if m.acceptance < r.quality_floor {
            reasons.push("quality_floor".into());
        }
        let h = r.health.get(&m.id).cloned().unwrap_or_default();
        if h.unavailable_until > now_ms() {
            reasons.push("unavailable".into());
        }
        if !reasons.is_empty() {
            rejected.insert(m.id.clone(), reasons);
            continue;
        }
        let acceptance = (m.acceptance * 10.0 + h.accepted as f64) / (10.0 + h.calls as f64);
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
        let w = &config.weights;
        let parts = BTreeMap::from([
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
                        / r.task.deadline_ms.saturating_sub(now_ms()).max(1) as f64)
                        .min(1.0),
            ),
            ("uncertainty".into(), -w.uncertainty * m.uncertainty),
        ]);
        candidates.push(RoutingDecision {
            decision_id: id(),
            node_id: r.node.into(),
            model_id: m.id.clone(),
            model_version: m.version.clone(),
            estimated_cost: cost,
            estimated_input: r.input_tokens,
            estimated_latency_ms: m.latency_ms,
            score: parts.values().sum(),
            breakdown: parts,
            excluded: BTreeMap::new(),
            weights_version: w.version.clone(),
            metrics_snapshot: digest(&r.health),
            uncertainty: m.uncertainty,
        });
    }
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.model_id.cmp(&b.model_id))
    });
    let mut chosen = candidates.into_iter().next().ok_or_else(|| {
        EngineError::new("routing", "no model satisfies all hard constraints")
            .details(json!({"excluded":rejected}))
    })?;
    chosen.excluded = rejected;
    Ok(chosen)
}
