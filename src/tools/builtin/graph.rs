//! Knowledge graph tools (Neo4j-backed), gated behind the `neo4j` Cargo feature.
//!
//! Lets the agent build and query a persistent graph of entities (people,
//! projects, organizations, meetings, documents, topics) and typed
//! relationships between them, so it doesn't have to be re-briefed every
//! conversation. See `rusty-talon-prd.md` for the design.
//!
//! `get_entity_context`'s description tells the model to call it whenever a
//! question touches people/projects/relationships -- there's no separate
//! hardcoded pre-fetch step, the tool-calling loop already surfaces this
//! tool every turn (same approach as `memory_search`).

use std::sync::Arc;

use async_trait::async_trait;

use crate::context::JobContext;
use crate::graph::GraphClient;
use crate::tools::tool::{Tool, ToolError, ToolOutput};

fn param_str<'a>(params: &'a serde_json::Value, key: &str) -> Result<&'a str, ToolError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::InvalidParameters(format!("missing '{key}' parameter")))
}

/// Create or upsert an entity node (F1/F5).
pub struct CreateEntityTool {
    client: Arc<GraphClient>,
}

impl CreateEntityTool {
    pub fn new(client: Arc<GraphClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for CreateEntityTool {
    fn name(&self) -> &str {
        "create_entity"
    }

    fn description(&self) -> &str {
        "Create or update an entity in the knowledge graph (a person, project, organization, \
         meeting, document, topic, or any other type you need). Idempotent: calling this again \
         with the same type+name updates the existing entity instead of duplicating it. Use \
         this whenever the user tells you something notable about a person, project, or \
         relationship worth remembering long-term."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "type": {
                    "type": "string",
                    "description": "Entity type/label, e.g. Person, Project, Organization, Meeting, Document, Topic. Must start with a letter and contain only letters, digits, underscores."
                },
                "name": {
                    "type": "string",
                    "description": "The entity's canonical name. Used to dedupe -- reuse the exact same name for the same real-world entity."
                },
                "properties": {
                    "type": "object",
                    "description": "Additional properties to store on the entity, e.g. {\"role\": \"engineer\", \"since\": \"2026-01\"}"
                }
            },
            "required": ["type", "name"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();

        let label = param_str(&params, "type")?;
        let name = param_str(&params, "name")?;
        let properties = params.get("properties").cloned();

        let entity = self
            .client
            .create_entity(label, name, properties)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolOutput::success(entity, start.elapsed()))
    }

    fn requires_sanitization(&self) -> bool {
        false // Internal tool, structured output
    }
}

/// Update an existing entity's properties without duplicating it (F5).
pub struct UpdateEntityTool {
    client: Arc<GraphClient>,
}

impl UpdateEntityTool {
    pub fn new(client: Arc<GraphClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for UpdateEntityTool {
    fn name(&self) -> &str {
        "update_entity"
    }

    fn description(&self) -> &str {
        "Update properties on an existing knowledge graph entity, identified by type+name. \
         Fails if the entity doesn't exist yet -- use create_entity for that."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "type": {
                    "type": "string",
                    "description": "The entity's existing type/label."
                },
                "name": {
                    "type": "string",
                    "description": "The entity's existing name."
                },
                "properties": {
                    "type": "object",
                    "description": "Properties to set/overwrite on the entity."
                }
            },
            "required": ["type", "name", "properties"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();

        let label = param_str(&params, "type")?;
        let name = param_str(&params, "name")?;
        let properties = params.get("properties").cloned().ok_or_else(|| {
            ToolError::InvalidParameters("missing 'properties' parameter".to_string())
        })?;

        let entity = self
            .client
            .update_entity(label, name, properties)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolOutput::success(entity, start.elapsed()))
    }

    fn requires_sanitization(&self) -> bool {
        false
    }
}

/// Create a typed, directional relationship between two existing entities (F2).
pub struct CreateRelationshipTool {
    client: Arc<GraphClient>,
}

impl CreateRelationshipTool {
    pub fn new(client: Arc<GraphClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for CreateRelationshipTool {
    fn name(&self) -> &str {
        "create_relationship"
    }

    fn description(&self) -> &str {
        "Create a typed, directional relationship between two entities that already exist in \
         the knowledge graph (create them first with create_entity if needed). Idempotent for \
         the same from/to/type triple. Examples: LEADS, MEMBER_OF, ATTENDED, RELATES_TO, ABOUT."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "from_entity": {
                    "type": "string",
                    "description": "Name of the source entity."
                },
                "to_entity": {
                    "type": "string",
                    "description": "Name of the target entity."
                },
                "type": {
                    "type": "string",
                    "description": "Relationship type, e.g. LEADS, MEMBER_OF, ATTENDED. Must start with a letter and contain only letters, digits, underscores."
                },
                "properties": {
                    "type": "object",
                    "description": "Additional properties, e.g. {\"since\": \"2026-03\"}"
                }
            },
            "required": ["from_entity", "to_entity", "type"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();

        let from_entity = param_str(&params, "from_entity")?;
        let to_entity = param_str(&params, "to_entity")?;
        let rel_type = param_str(&params, "type")?;
        let properties = params.get("properties").cloned();

        let relationship = self
            .client
            .create_relationship(from_entity, to_entity, rel_type, properties)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolOutput::success(relationship, start.elapsed()))
    }

    fn requires_sanitization(&self) -> bool {
        false
    }
}

/// Fuzzy search entities by name (F3).
pub struct SearchEntitiesTool {
    client: Arc<GraphClient>,
}

impl SearchEntitiesTool {
    pub fn new(client: Arc<GraphClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for SearchEntitiesTool {
    fn name(&self) -> &str {
        "search_entities"
    }

    fn description(&self) -> &str {
        "Search the knowledge graph for entities (people, projects, organizations, meetings, \
         documents, topics) by fuzzy name match. Call this when the user references a person, \
         project, or organization you may already have context on, before asking them to \
         re-explain who or what it is."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Name or partial name to search for."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 10, max: 50)",
                    "default": 10,
                    "minimum": 1,
                    "maximum": 50
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();

        let query = param_str(&params, "query")?;
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .min(50) as u32;

        let results = self
            .client
            .search_entities(query, limit)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let output = serde_json::json!({
            "query": query,
            "results": results,
            "result_count": results.len(),
        });

        Ok(ToolOutput::success(output, start.elapsed()))
    }

    fn requires_sanitization(&self) -> bool {
        false
    }
}

/// Traverse N hops from an entity to get its surrounding context (F4/F10).
pub struct GetEntityContextTool {
    client: Arc<GraphClient>,
}

impl GetEntityContextTool {
    pub fn new(client: Arc<GraphClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for GetEntityContextTool {
    fn name(&self) -> &str {
        "get_entity_context"
    }

    fn description(&self) -> &str {
        "Get everything the knowledge graph knows about an entity: its properties plus \
         everyone/everything connected to it within a few hops. Call this before answering \
         questions like 'what's the status of X' or 'who's involved with Y' -- it's grounded \
         in accumulated relationships rather than the current conversation alone."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The entity's name."
                },
                "hops": {
                    "type": "integer",
                    "description": "How many relationship hops to traverse (default: 2, max: 3).",
                    "default": 2,
                    "minimum": 1,
                    "maximum": 3
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();

        let name = param_str(&params, "name")?;
        let hops = params.get("hops").and_then(|v| v.as_u64()).unwrap_or(2) as u32;

        let context = self
            .client
            .entity_context(name, hops)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolOutput::success(context, start.elapsed()))
    }

    fn requires_sanitization(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // GraphClient::connect() requires a live Neo4j instance, so these tests
    // only cover the pure parts: schema shape and parameter validation.
    // Query-level behavior needs `docker compose -f docker-compose.dev.yml
    // --profile neo4j up` and is not exercised here.

    fn param_str_from(json: serde_json::Value, key: &str) -> Result<String, ToolError> {
        param_str(&json, key).map(|s| s.to_string())
    }

    #[test]
    fn param_str_rejects_missing_and_empty() {
        assert!(param_str_from(serde_json::json!({}), "name").is_err());
        assert!(param_str_from(serde_json::json!({"name": ""}), "name").is_err());
        assert!(param_str_from(serde_json::json!({"name": "Jane"}), "name").is_ok());
    }
}
