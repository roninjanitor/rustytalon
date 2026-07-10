//! Errors for the knowledge graph client.

use thiserror::Error;

/// Errors returned by the knowledge graph client and its query helpers.
#[derive(Debug, Error)]
pub enum GraphError {
    #[error("failed to connect to Neo4j: {0}")]
    Connection(String),

    #[error("invalid entity/relationship type '{0}': must match ^[A-Za-z][A-Za-z0-9_]{{0,63}}$")]
    InvalidType(String),

    #[error("entity not found: {0}")]
    NotFound(String),

    #[error("query failed: {0}")]
    Query(String),
}
