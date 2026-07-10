//! Native knowledge graph (Neo4j-backed), gated behind the `neo4j` Cargo
//! feature. See `rusty-talon-prd.md` for the design rationale.
//!
//! Entities and relationships are exposed to the agent's LLM tool-calling
//! via `src/tools/builtin/graph.rs`. This module only handles the Neo4j
//! client and query construction/validation.

mod client;
mod error;
mod validate;

pub use client::GraphClient;
pub use error::GraphError;
