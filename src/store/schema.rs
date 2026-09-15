//! Development databases use one current schema, without automatic migration or reset.
use super::{APPLICATION_ID, SCHEMA_VERSION};
use crate::contracts::{EngineError, Result};
use rusqlite::Connection;
use serde_json::json;

pub(super) fn open(connection: &mut Connection) -> Result<()> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let application: i64 = connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    let objects: i64 = connection.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    let fresh = version == 0 && application == 0 && objects == 0;
    if !fresh {
        if application != APPLICATION_ID {
            return Err(EngineError::new("schema", "file is not an engine database"));
        }
        if version != SCHEMA_VERSION {
            return Err(EngineError::new(
                "schema",
                "unsupported database schema; automatic migration is disabled during development; back up the existing database and use a new database_path to rebuild; existing data is unchanged",
            ).details(json!({
                "current": version,
                "target": SCHEMA_VERSION,
                "action": "backup_and_rebuild",
            })));
        }
    }
    connection.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA busy_timeout=5000;",
    )?;
    if fresh {
        initialize(connection)?;
    }
    Ok(())
}

fn initialize(connection: &mut Connection) -> Result<()> {
    // Initialize every table and both version markers in a single transaction.
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE TABLE tasks(
            id TEXT PRIMARY KEY, spec TEXT NOT NULL, config_hash TEXT NOT NULL,
            plan TEXT NOT NULL, status TEXT NOT NULL, total INTEGER NOT NULL,
            settled INTEGER NOT NULL DEFAULT 0, reserved INTEGER NOT NULL DEFAULT 0,
            calls INTEGER NOT NULL DEFAULT 0, checkpoint TEXT NOT NULL, result TEXT);
         CREATE TABLE attempts(
            id TEXT PRIMARY KEY, task TEXT NOT NULL REFERENCES tasks(id),
            amount INTEGER NOT NULL, cost INTEGER, state TEXT NOT NULL,
            metadata TEXT NOT NULL, outcome TEXT);
         CREATE TABLE events(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task TEXT NOT NULL REFERENCES tasks(id), payload TEXT NOT NULL);
         CREATE TABLE evaluations(
            artifact_id TEXT PRIMARY KEY,
            task TEXT NOT NULL REFERENCES tasks(id), record TEXT NOT NULL);
         CREATE INDEX evaluations_task ON evaluations(task);
         CREATE TABLE feedback(
            id INTEGER PRIMARY KEY AUTOINCREMENT, feedback_id TEXT NOT NULL UNIQUE,
            task TEXT NOT NULL REFERENCES tasks(id),
            artifact_id TEXT NOT NULL REFERENCES evaluations(artifact_id),
            kind TEXT NOT NULL, key_json TEXT NOT NULL,
            accepted INTEGER NOT NULL CHECK(accepted IN (0,1)),
            payload TEXT NOT NULL, UNIQUE(artifact_id,kind));
         CREATE INDEX feedback_quality ON feedback(key_json,kind);",
    )?;
    transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}
