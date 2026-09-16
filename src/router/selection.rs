//! Pure task-fit value calculation; eligibility remains owned by the router.
use crate::contracts::{SelectionPolicy, Weights};
use std::collections::BTreeMap;

pub(super) struct Inputs {
    pub quality: f64,
    pub capability: f64,
    pub reliability: f64,
    pub cost: u64,
    pub latency_ms: u64,
    pub uncertainty: f64,
}

pub(super) fn primary_parts(
    weights: &Weights,
    policy: &SelectionPolicy,
    inputs: Inputs,
) -> BTreeMap<String, f64> {
    BTreeMap::from([
        (
            "quality".into(),
            weights.quality * policy.quality_value(inputs.quality),
        ),
        ("capability".into(), weights.capability * inputs.capability),
        (
            "reliability".into(),
            weights.reliability * inputs.reliability,
        ),
        (
            "cost".into(),
            -weights.cost * inputs.cost as f64 / policy.cost_reference as f64,
        ),
        (
            "latency".into(),
            -weights.latency * inputs.latency_ms as f64 / policy.latency_reference_ms as f64,
        ),
        (
            "uncertainty".into(),
            -weights.uncertainty * inputs.uncertainty,
        ),
    ])
}
