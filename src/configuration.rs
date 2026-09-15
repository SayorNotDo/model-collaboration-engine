//! Strict file configuration resolves service references before any external operation.
mod document;

use crate::contracts::{Config, EngineError, Result};
use document::Document;
use std::{fs::File, io::Read, path::Path};

const MAX_CONFIG_BYTES: u64 = 1_048_576;

/// Validated configuration. Its internal snapshot has no mutation interface.
#[derive(Debug, Clone)]
pub struct ResolvedConfig(Config);
impl ResolvedConfig {
    pub fn as_config(&self) -> &Config {
        &self.0
    }
    /// Transfer ownership to the engine's existing component assembly boundary.
    pub fn into_config(self) -> Config {
        self.0
    }
}

pub(super) fn error(field: &str, message: &str) -> EngineError {
    EngineError::new("configuration", message).details(serde_json::json!({"field":field}))
}

/// Load UTF-8 JSON; database paths resolve against the file directory.
/// Does not read credentials, contact services, create directories or open the database.
pub fn load(path: impl AsRef<Path>) -> Result<ResolvedConfig> {
    let absolute = std::path::absolute(path)
        .map_err(|_| error("$file", "cannot resolve configuration path"))?;
    let mut bytes = Vec::new();
    File::open(&absolute)
        .map_err(|_| error("$file", "cannot open configuration file"))?
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error("$file", "cannot read configuration file"))?;
    let raw =
        std::str::from_utf8(&bytes).map_err(|_| error("$file", "configuration must be UTF-8"))?;
    parse(
        raw,
        absolute
            .parent()
            .ok_or_else(|| error("$file", "configuration has no parent directory"))?,
    )
}

/// Parse a document; relative paths use base_dir (itself relative to the process directory).
/// Values are never echoed in parse errors. The result owns a detached snapshot.
pub fn parse(raw: &str, base_dir: impl AsRef<Path>) -> Result<ResolvedConfig> {
    if raw.len() as u64 > MAX_CONFIG_BYTES {
        return Err(error("$", "configuration exceeds 1 MiB"));
    }
    let mut deserializer = serde_json::Deserializer::from_str(raw.trim_start_matches('\u{feff}'));
    let document: Document =
        serde_path_to_error::deserialize(&mut deserializer).map_err(|failure| {
            error(
                &failure.path().to_string(),
                "invalid configuration fields, types or JSON",
            )
        })?;
    deserializer
        .end()
        .map_err(|_| error("$", "unexpected trailing JSON"))?;
    let base = std::path::absolute(base_dir)
        .map_err(|_| error("storage.database_path", "cannot resolve base directory"))?;
    Ok(ResolvedConfig(document.resolve(&base)?))
}
