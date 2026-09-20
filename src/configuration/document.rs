//! File-schema ownership and provider-to-model resolution; execution uses contracts::Config.
use super::error;
use crate::{
    assessment::ExecutionClass,
    contracts::{Config, Endpoint, Model, RankingConfig, Result, RoutingProfiles, Weights},
};
use serde::Deserialize;
use std::{collections::BTreeSet, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Document {
    schema_version: u32,
    storage: Storage,
    providers: Vec<Provider>,
    routing: Routing,
    runtime: Runtime,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Storage {
    database_path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Provider {
    id: String,
    kind: ProviderKind,
    base_url: String,
    auth: Auth,
    endpoints: Vec<Endpoint>,
    region: String,
    local: bool,
    models: Vec<ModelDefinition>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProviderKind {
    Direct,
    Relay,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Auth {
    r#type: AuthType,
    env: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum AuthType {
    Bearer,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelDefinition {
    id: String,
    model: String,
    version: String,
    endpoint: Endpoint,
    capabilities: BTreeSet<String>,
    context_tokens: u64,
    input_price: u64,
    output_price: u64,
    price_version: String,
    acceptance: f64,
    reliability: f64,
    latency_ms: u64,
    uncertainty: f64,
    #[serde(default)]
    execution_class: ExecutionClass,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Routing {
    #[serde(default)]
    planner_models: BTreeSet<String>,
    #[serde(default)]
    profiles: Option<RoutingProfiles>,
    #[serde(default)]
    rankings: Option<RankingConfig>,
    #[serde(default)]
    assessment_rules: Option<crate::assessment::RuleSet>,
    #[serde(default)]
    decision: Option<crate::decision::DecisionConfig>,
    weights: Weights,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Runtime {
    max_concurrency: usize,
    event_capacity: usize,
    event_max_bytes: usize,
    stream_frame_max_bytes: usize,
    context_max_bytes: usize,
    close_grace_ms: u64,
    cleanup_timeout_ms: u64,
}

impl Provider {
    fn validate(&self, index: usize) -> Result<()> {
        let field = |name: &str| format!("providers[{index}].{name}");
        if self.id.trim().is_empty() || self.region.trim().is_empty() {
            return Err(error(
                &field("id"),
                "provider identity and region must be nonempty",
            ));
        }
        let key = self.auth.env.as_bytes();
        if key.is_empty()
            || !(key[0].is_ascii_alphabetic() || key[0] == b'_')
            || !key.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            return Err(error(
                &field("auth.env"),
                "invalid credential environment variable name",
            ));
        }
        let url = reqwest::Url::parse(&self.base_url)
            .map_err(|_| error(&field("base_url"), "invalid service URL"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(error(
                &field("base_url"),
                "service URL must be HTTP(S) without credentials, query or fragment",
            ));
        }
        if self.endpoints.is_empty()
            || self
                .endpoints
                .iter()
                .enumerate()
                .any(|(i, e)| self.endpoints[..i].contains(e))
        {
            return Err(error(
                &field("endpoints"),
                "service endpoints must be nonempty and unique",
            ));
        }
        // Typed declarations describe the connection, never an upstream identity.
        let _ = (&self.kind, &self.auth.r#type);
        Ok(())
    }
}

impl Document {
    pub(super) fn resolve(self, base: &Path) -> Result<Config> {
        if self.schema_version != 1 {
            return Err(error(
                "schema_version",
                "only configuration schema version 1 is supported",
            ));
        }
        if self.storage.database_path.trim().is_empty() || self.storage.database_path == ":memory:"
        {
            return Err(error(
                "storage.database_path",
                "database path must name a file",
            ));
        }
        if self.providers.is_empty() {
            return Err(error("providers", "providers must not be empty"));
        }
        let mut providers = BTreeSet::new();
        let mut ids = BTreeSet::new();
        let mut models = Vec::new();
        for (index, mut provider) in self.providers.into_iter().enumerate() {
            provider.validate(index)?;
            if !providers.insert(provider.id.clone()) {
                return Err(error(
                    &format!("providers[{index}].id"),
                    "duplicate provider ID",
                ));
            }
            let definitions = std::mem::take(&mut provider.models);
            models.extend(resolve_models(definitions, &provider, index, &mut ids)?);
        }
        let path = base.join(&self.storage.database_path);
        let database_path = path
            .to_str()
            .ok_or_else(|| error("storage.database_path", "database path must be UTF-8"))?
            .to_owned();
        let runtime = self.runtime;
        let config = Config {
            database_path,
            models,
            planner_models: self.routing.planner_models,
            routing_profiles: self.routing.profiles,
            rankings: self.routing.rankings,
            assessment_rules: self.routing.assessment_rules,
            decision: self.routing.decision,
            weights: self.routing.weights,
            max_concurrency: runtime.max_concurrency,
            event_capacity: runtime.event_capacity,
            event_max_bytes: runtime.event_max_bytes,
            stream_frame_max_bytes: runtime.stream_frame_max_bytes,
            context_max_bytes: runtime.context_max_bytes,
            close_grace_ms: runtime.close_grace_ms,
            cleanup_timeout_ms: runtime.cleanup_timeout_ms,
        };
        config
            .validate_runtime()
            .map_err(|failure| error("runtime", &failure.message))?;
        config
            .validate(true)
            .map_err(|failure| error("routing", &failure.message))?;
        Ok(config)
    }
}

fn resolve_models(
    definitions: Vec<ModelDefinition>,
    provider: &Provider,
    provider_index: usize,
    ids: &mut BTreeSet<String>,
) -> Result<Vec<Model>> {
    if definitions.is_empty() {
        return Err(error(
            &format!("providers[{provider_index}].models"),
            "models must not be empty",
        ));
    }
    let mut models = Vec::new();
    for (index, definition) in definitions.into_iter().enumerate() {
        let field = |name: &str| format!("providers[{provider_index}].models[{index}].{name}");
        if definition.id.trim().is_empty() || !ids.insert(definition.id.clone()) {
            return Err(error(&field("id"), "model IDs must be nonempty and unique"));
        }
        if !provider.endpoints.contains(&definition.endpoint) {
            return Err(error(
                &field("endpoint"),
                "endpoint is not declared by the provider",
            ));
        }
        let model = Model {
            id: definition.id,
            model: definition.model,
            version: definition.version,
            endpoint: definition.endpoint,
            base_url: provider.base_url.trim_end_matches('/').to_owned(),
            api_key_env: provider.auth.env.clone(),
            provider: provider.id.clone(),
            region: provider.region.clone(),
            local: provider.local,
            capabilities: definition.capabilities,
            context_tokens: definition.context_tokens,
            input_price: definition.input_price,
            output_price: definition.output_price,
            price_version: definition.price_version,
            acceptance: definition.acceptance,
            reliability: definition.reliability,
            latency_ms: definition.latency_ms,
            uncertainty: definition.uncertainty,
            execution_class: definition.execution_class,
        };
        model.validate().map_err(|failure| {
            error(
                &format!("providers[{provider_index}].models[{index}]"),
                &failure.message,
            )
        })?;
        models.push(model);
    }
    Ok(models)
}
