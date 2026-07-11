//! Knowledge graph tools (Neo4j-backed), gated behind the `neo4j` Cargo feature.
//!
//! Lets the agent build and query a persistent graph of entities (people,
//! projects, organizations, meetings, documents, topics) and typed
//! relationships between them, so it doesn't have to be re-briefed every
//! conversation. See `docs/KNOWLEDGE_GRAPH_PRD.md` for the design.
//!
//! `get_entity_context`'s description tells the model to call it whenever a
//! question touches people/projects/relationships -- there's no separate
//! hardcoded pre-fetch step, the tool-calling loop already surfaces this
//! tool every turn (same approach as `memory_search`).
//!
//! `stage_candidate`/`list_candidates`/`approve_candidate`/`reject_candidate`
//! implement the PRD's Phase B review workflow: extraction-style writes go
//! through staging instead of `create_entity`/`create_relationship` directly,
//! and nothing commits to the live graph without an explicit approval. There
//! is no built-in scheduled extraction routine -- create one via the
//! existing `routine_create` tool (a `FullJob` whose description tells the
//! agent to review recent conversation and call `stage_candidate`).

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

/// Delete an entity and all its relationships (F6).
pub struct DeleteEntityTool {
    client: Arc<GraphClient>,
}

impl DeleteEntityTool {
    pub fn new(client: Arc<GraphClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for DeleteEntityTool {
    fn name(&self) -> &str {
        "delete_entity"
    }

    fn description(&self) -> &str {
        "Permanently delete an entity and all its relationships from the knowledge graph. \
         Use this to correct a bad extraction or a duplicate, not for routine cleanup -- \
         prefer update_entity when the entity is still valid."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "type": {
                    "type": "string",
                    "description": "The entity's type/label."
                },
                "name": {
                    "type": "string",
                    "description": "The entity's name."
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

        self.client
            .delete_entity(label, name)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolOutput::success(
            serde_json::json!({ "status": "deleted", "type": label, "name": name }),
            start.elapsed(),
        ))
    }

    fn requires_approval(&self) -> bool {
        true // Destructive, irreversible
    }

    fn requires_sanitization(&self) -> bool {
        false
    }
}

/// Merge a duplicate entity into a canonical one (F6).
pub struct MergeEntitiesTool {
    client: Arc<GraphClient>,
}

impl MergeEntitiesTool {
    pub fn new(client: Arc<GraphClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for MergeEntitiesTool {
    fn name(&self) -> &str {
        "merge_entities"
    }

    fn description(&self) -> &str {
        "Merge a duplicate entity into a canonical one of the same type -- e.g. 'Sanjay' and \
         'Sanjay Mehta' turning out to be the same person. Combines properties and re-points \
         all relationships onto the target; the source entity is removed. Requires the Neo4j \
         APOC plugin to be installed on the server."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "type": {
                    "type": "string",
                    "description": "The shared entity type/label."
                },
                "source_name": {
                    "type": "string",
                    "description": "Name of the duplicate entity to merge away."
                },
                "target_name": {
                    "type": "string",
                    "description": "Name of the canonical entity to keep."
                }
            },
            "required": ["type", "source_name", "target_name"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();

        let label = param_str(&params, "type")?;
        let source_name = param_str(&params, "source_name")?;
        let target_name = param_str(&params, "target_name")?;

        let merged = self
            .client
            .merge_entities(label, source_name, target_name)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolOutput::success(merged, start.elapsed()))
    }

    fn requires_approval(&self) -> bool {
        true // Destructive, irreversible
    }

    fn requires_sanitization(&self) -> bool {
        false
    }
}

/// Stage a batch of proposed entities/relationships for review (F8/F9).
pub struct StageCandidateTool {
    client: Arc<GraphClient>,
}

impl StageCandidateTool {
    pub fn new(client: Arc<GraphClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for StageCandidateTool {
    fn name(&self) -> &str {
        "stage_candidate"
    }

    fn description(&self) -> &str {
        "Propose entities and/or relationships for the knowledge graph WITHOUT committing them. \
         Use this instead of create_entity/create_relationship when you're extracting \
         information from conversation history rather than acting on something the user just \
         told you directly -- extraction quality is unproven, so proposals need human review \
         via list_candidates/approve_candidate/reject_candidate before they land in the graph."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "entities": {
                    "type": "array",
                    "description": "Proposed entities.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "type": {"type": "string"},
                            "name": {"type": "string"},
                            "properties": {"type": "object"}
                        },
                        "required": ["type", "name"]
                    }
                },
                "relationships": {
                    "type": "array",
                    "description": "Proposed relationships.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "from_entity": {"type": "string"},
                            "to_entity": {"type": "string"},
                            "type": {"type": "string"},
                            "properties": {"type": "object"}
                        },
                        "required": ["from_entity", "to_entity", "type"]
                    }
                },
                "source": {
                    "type": "string",
                    "description": "Where this was extracted from, e.g. 'conversation:2026-07-10' or a channel/message reference."
                },
                "confidence": {
                    "type": "number",
                    "description": "How confident you are this is correct, 0.0-1.0 (default: 0.5).",
                    "default": 0.5,
                    "minimum": 0.0,
                    "maximum": 1.0
                }
            },
            "required": ["source"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();

        let entities: Vec<crate::graph::CandidateEntity> = match params.get("entities") {
            Some(v) => serde_json::from_value(v.clone())
                .map_err(|e| ToolError::InvalidParameters(format!("invalid 'entities': {e}")))?,
            None => Vec::new(),
        };
        let relationships: Vec<crate::graph::CandidateRelationship> =
            match params.get("relationships") {
                Some(v) => serde_json::from_value(v.clone()).map_err(|e| {
                    ToolError::InvalidParameters(format!("invalid 'relationships': {e}"))
                })?,
                None => Vec::new(),
            };

        if entities.is_empty() && relationships.is_empty() {
            return Err(ToolError::InvalidParameters(
                "at least one of 'entities' or 'relationships' must be non-empty".to_string(),
            ));
        }

        let source = param_str(&params, "source")?;
        let confidence = params
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.5);

        let candidate = self
            .client
            .stage_candidate(&entities, &relationships, source, confidence)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolOutput::success(candidate, start.elapsed()))
    }

    fn requires_sanitization(&self) -> bool {
        false
    }
}

/// List staged candidates awaiting review (F9).
pub struct ListCandidatesTool {
    client: Arc<GraphClient>,
}

impl ListCandidatesTool {
    pub fn new(client: Arc<GraphClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for ListCandidatesTool {
    fn name(&self) -> &str {
        "list_candidates"
    }

    fn description(&self) -> &str {
        "List staged knowledge graph candidates awaiting review. Defaults to pending ones. \
         Use this when the user asks to review, approve, or reject graph proposals."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["pending", "approved", "rejected"],
                    "description": "Filter by status (default: pending)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 20, max: 100)",
                    "default": 20,
                    "minimum": 1,
                    "maximum": 100
                }
            }
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();

        let status = params
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending");
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20)
            .min(100) as u32;

        let candidates = self
            .client
            .list_candidates(Some(status), limit)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let output = serde_json::json!({
            "status_filter": status,
            "candidates": candidates,
            "count": candidates.len(),
        });

        Ok(ToolOutput::success(output, start.elapsed()))
    }

    fn requires_sanitization(&self) -> bool {
        false
    }
}

/// Commit a pending candidate into the live graph (F9).
pub struct ApproveCandidateTool {
    client: Arc<GraphClient>,
}

impl ApproveCandidateTool {
    pub fn new(client: Arc<GraphClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for ApproveCandidateTool {
    fn name(&self) -> &str {
        "approve_candidate"
    }

    fn description(&self) -> &str {
        "Approve a staged knowledge graph candidate by id, committing its entities and \
         relationships into the live graph. Call list_candidates first to find the id. \
         Only call this after the user has confirmed they want it committed -- staged \
         candidates exist specifically so nothing enters the graph without review."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The candidate's id, from list_candidates."
                }
            },
            "required": ["id"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();

        let id = param_str(&params, "id")?;

        let result = self
            .client
            .approve_candidate(id)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolOutput::success(result, start.elapsed()))
    }

    fn requires_approval(&self) -> bool {
        true // Commits data into the live graph
    }

    fn requires_sanitization(&self) -> bool {
        false
    }
}

/// Reject a pending candidate without committing anything (F9).
pub struct RejectCandidateTool {
    client: Arc<GraphClient>,
}

impl RejectCandidateTool {
    pub fn new(client: Arc<GraphClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for RejectCandidateTool {
    fn name(&self) -> &str {
        "reject_candidate"
    }

    fn description(&self) -> &str {
        "Reject a staged knowledge graph candidate by id. Nothing is committed; the candidate \
         is kept with status 'rejected' for audit purposes. Call list_candidates first to find \
         the id."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The candidate's id, from list_candidates."
                }
            },
            "required": ["id"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();

        let id = param_str(&params, "id")?;

        let result = self
            .client
            .reject_candidate(id)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolOutput::success(result, start.elapsed()))
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
