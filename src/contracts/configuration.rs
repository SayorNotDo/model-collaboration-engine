//! Effective execution configuration; shared validation for file and component callers.
use super::{profiles, Endpoint, EngineError, Result, RoutingProfiles};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub id: String,
    pub model: String,
    pub version: String,
    pub endpoint: Endpoint,
    pub base_url: String,
    pub api_key_env: String,
    pub provider: String,
    pub region: String,
    pub local: bool,
    pub capabilities: BTreeSet<String>,
    pub context_tokens: u64,
    /// Integer microcredits per token; no floating point money.
    pub input_price: u64,
    pub output_price: u64,
    pub price_version: String,
    pub acceptance: f64,
    pub reliability: f64,
    pub latency_ms: u64,
    pub uncertainty: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Weights {
    pub quality: f64,
    pub capability: f64,
    pub reliability: f64,
    pub cost: f64,
    pub latency: f64,
    pub uncertainty: f64,
    pub version: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub database_path: String,
    pub models: Vec<Model>,
    /// Explicit candidate IDs for planning. Empty disables model-based planning.
    #[serde(default)]
    pub planner_models: BTreeSet<String>,
    /// Optional quality evidence; missing profiles use the global prior in the same router.
    #[serde(default)]
    pub routing_profiles: Option<RoutingProfiles>,
    pub weights: Weights,
    pub max_concurrency: usize,
    pub event_capacity: usize,
    pub event_max_bytes: usize,
    pub stream_frame_max_bytes: usize,
    pub context_max_bytes: usize,
    pub close_grace_ms: u64,
    pub cleanup_timeout_ms: u64,
}
impl Config {
    pub(crate) fn validate_runtime(&self) -> Result<()> {
        let bad = |s| EngineError::new("configuration", s);
        if self.close_grace_ms > 86_400_000 || self.cleanup_timeout_ms > 86_400_000 {
            return Err(bad("shutdown timeouts must be between zero and one day"));
        }
        if !(1..=1024).contains(&self.max_concurrency)
            || !(1..=65536).contains(&self.event_capacity)
            || !(1024..=1048576).contains(&self.event_max_bytes)
            || !(1024..=4194304).contains(&self.stream_frame_max_bytes)
            || !(1024..=16777216).contains(&self.context_max_bytes)
        {
            return Err(bad("invalid concurrency or buffer bounds"));
        }
        Ok(())
    }

    pub fn validate(&self, sqlite: bool) -> Result<()> {
        let bad = |s| EngineError::new("configuration", s);
        if sqlite && (self.database_path.trim().is_empty() || self.database_path == ":memory:") {
            return Err(bad("database_path must name a file"));
        }
        if self.models.is_empty() {
            return Err(bad("models must not be empty"));
        }
        self.validate_runtime()?;
        profiles::validate_weights(&self.weights)?;
        let mut ids = BTreeSet::new();
        for model in &self.models {
            model.validate()?;
            if !ids.insert(&model.id) {
                return Err(bad("duplicate model ID"));
            }
        }
        if self.planner_models.iter().any(|id| !ids.contains(id)) {
            return Err(bad("planner_models must reference configured model IDs"));
        }
        if let Some(profiles) = &self.routing_profiles {
            profiles.validate(self)?;
        }
        Ok(())
    }
}

impl Model {
    pub(crate) fn validate(&self) -> Result<()> {
        let bad = |s| EngineError::new("configuration", s);
        let m = self;
        if m.id.is_empty()
            || m.model.is_empty()
            || m.version.is_empty()
            || m.price_version.is_empty()
            || m.api_key_env.is_empty()
        {
            return Err(bad(
                "model identifiers, credential references and versions must be nonempty and unique",
            ));
        }
        let url = reqwest::Url::parse(&m.base_url).map_err(|_| bad("invalid model base_url"))?;
        if !matches!(url.scheme(), "https" | "http")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(bad(
                "base_url must be HTTP(S) without credentials, query or fragment",
            ));
        }
        if m.context_tokens == 0
            || m.context_tokens > 100_000_000
            || m.input_price > 1_000_000_000
            || m.output_price > 1_000_000_000
            || [m.acceptance, m.reliability, m.uncertainty]
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err(bad("invalid model capacity, price or metrics"));
        }
        if m.capabilities
            .iter()
            .any(|c| !matches!(c.as_str(), "text" | "tools" | "json"))
        {
            return Err(bad(
                "phase one supports text, tools and json capabilities only",
            ));
        }
        Ok(())
    }
}
