//! Neo4j-backed knowledge graph client.
//!
//! Wraps `neo4rs::Graph` with the entity/relationship operations the graph
//! tools need (`src/tools/builtin/graph.rs`). All node labels and
//! relationship types passed through here are validated via
//! `crate::graph::validate` before being interpolated into Cypher, since
//! Neo4j does not support parameter binding for labels/relationship types.

use neo4rs::{Graph, Node, query};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Neo4jConfig;
use crate::graph::error::GraphError;
use crate::graph::validate::{clamp_hops, validate_label, validate_rel_type};

/// Name of the full-text index used by `search_entities`. Created on connect.
const ENTITY_SEARCH_INDEX: &str = "entity_search";

/// A proposed entity inside a staged candidate (F8/F9). Mirrors `create_entity`'s
/// params so `approve_candidate` can replay it unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateEntity {
    #[serde(rename = "type")]
    pub label: String,
    pub name: String,
    #[serde(default)]
    pub properties: Option<Value>,
}

/// A proposed relationship inside a staged candidate (F8/F9). Mirrors
/// `create_relationship`'s params so `approve_candidate` can replay it unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateRelationship {
    pub from_entity: String,
    pub to_entity: String,
    #[serde(rename = "type")]
    pub rel_type: String,
    #[serde(default)]
    pub properties: Option<Value>,
}

pub struct GraphClient {
    graph: Graph,
}

impl GraphClient {
    /// Connect to Neo4j and ensure the full-text search index exists.
    pub async fn connect(cfg: &Neo4jConfig) -> Result<Self, GraphError> {
        let uri = cfg
            .uri
            .clone()
            .ok_or_else(|| GraphError::Connection("NEO4J_URI is not set".to_string()))?;
        let user = cfg.user.clone().unwrap_or_else(|| "neo4j".to_string());
        let password = cfg.password().unwrap_or_default().to_string();

        let graph = Graph::new(uri, user, password)
            .await
            .map_err(|e| GraphError::Connection(e.to_string()))?;

        let client = Self { graph };
        client.ensure_indexes().await?;
        Ok(client)
    }

    async fn ensure_indexes(&self) -> Result<(), GraphError> {
        // Neo4j's fulltext index syntax requires an explicit label on the node
        // pattern -- a bare `(n)` (needed here since entity types are open-ended,
        // not a fixed enum) is a syntax error for CREATE FULLTEXT INDEX, unlike a
        // plain MATCH. Every entity gets a common `Entity` label (see
        // `create_entity`) alongside its specific type label so this index can
        // cover all of them regardless of type.
        let cypher = format!(
            "CREATE FULLTEXT INDEX {ENTITY_SEARCH_INDEX} IF NOT EXISTS FOR (n:Entity) ON EACH [n.name]"
        );
        self.graph
            .run(query(&cypher))
            .await
            .map_err(|e| GraphError::Query(e.to_string()))?;

        self.graph
            .run(query(
                "CREATE CONSTRAINT graph_candidate_id IF NOT EXISTS \
                 FOR (c:GraphCandidate) REQUIRE c.id IS UNIQUE",
            ))
            .await
            .map_err(|e| GraphError::Query(e.to_string()))
    }

    fn properties_to_bolt(properties: Option<Value>) -> Result<neo4rs::BoltType, GraphError> {
        let value = properties.unwrap_or_else(|| json!({}));
        if !value.is_object() {
            return Err(GraphError::Query(
                "properties must be a JSON object".to_string(),
            ));
        }
        value
            .try_into()
            .map_err(|e: neo4rs::Error| GraphError::Query(format!("invalid properties: {e}")))
    }

    /// Create or update (F1/F5/N4) an entity node, deduped on (label, name).
    pub async fn create_entity(
        &self,
        label: &str,
        name: &str,
        properties: Option<Value>,
    ) -> Result<Value, GraphError> {
        validate_label(label)?;
        let props = Self::properties_to_bolt(properties)?;
        let now = chrono::Utc::now().to_rfc3339();

        // Every entity also gets the common `Entity` label (in addition to its
        // specific type label) so the fulltext search index -- which must target
        // a concrete label -- can cover entities of any type.
        let cypher = format!(
            "MERGE (n:{label}:Entity {{name: $name}}) \
             ON CREATE SET n += $props, n.created_at = $now \
             ON MATCH SET n += $props, n.updated_at = $now \
             RETURN n"
        );

        let q = query(&cypher)
            .param("name", name)
            .param("props", props)
            .param("now", now);

        let row = self.execute_single(q).await?;
        let node: Node = row.get("n").map_err(|e| GraphError::Query(e.to_string()))?;
        node_to_json(&node)
    }

    /// Update an existing entity's properties without creating a duplicate (F5).
    pub async fn update_entity(
        &self,
        label: &str,
        name: &str,
        properties: Value,
    ) -> Result<Value, GraphError> {
        validate_label(label)?;
        let props = Self::properties_to_bolt(Some(properties))?;
        let now = chrono::Utc::now().to_rfc3339();

        let cypher = format!(
            "MATCH (n:{label} {{name: $name}}) SET n += $props, n.updated_at = $now RETURN n"
        );

        let q = query(&cypher)
            .param("name", name)
            .param("props", props)
            .param("now", now);

        let row = self
            .execute_optional(q)
            .await?
            .ok_or_else(|| GraphError::NotFound(name.to_string()))?;
        let node: Node = row.get("n").map_err(|e| GraphError::Query(e.to_string()))?;
        node_to_json(&node)
    }

    /// Create a typed, directional relationship between two existing entities (F2).
    pub async fn create_relationship(
        &self,
        from_name: &str,
        to_name: &str,
        rel_type: &str,
        properties: Option<Value>,
    ) -> Result<Value, GraphError> {
        validate_rel_type(rel_type)?;
        let props = Self::properties_to_bolt(properties)?;
        let now = chrono::Utc::now().to_rfc3339();

        let cypher = format!(
            "MATCH (a {{name: $from}}), (b {{name: $to}}) \
             MERGE (a)-[r:{rel_type}]->(b) \
             ON CREATE SET r += $props, r.created_at = $now \
             ON MATCH SET r += $props, r.updated_at = $now \
             RETURN a.name AS from_name, b.name AS to_name, type(r) AS rel_type"
        );

        let q = query(&cypher)
            .param("from", from_name)
            .param("to", to_name)
            .param("props", props)
            .param("now", now);

        let row = self.execute_optional(q).await?.ok_or_else(|| {
            GraphError::NotFound(format!(
                "one or both entities not found: '{from_name}', '{to_name}'"
            ))
        })?;

        let from: String = row
            .get("from_name")
            .map_err(|e| GraphError::Query(e.to_string()))?;
        let to: String = row
            .get("to_name")
            .map_err(|e| GraphError::Query(e.to_string()))?;
        let rel: String = row
            .get("rel_type")
            .map_err(|e| GraphError::Query(e.to_string()))?;

        Ok(json!({ "from": from, "to": to, "type": rel }))
    }

    /// Fuzzy search entities by name (F3). Tries the full-text index first,
    /// falling back to a case-insensitive substring match if the index isn't
    /// ready yet or the query has no full-text matches.
    pub async fn search_entities(&self, text: &str, limit: u32) -> Result<Vec<Value>, GraphError> {
        let limit = i64::from(limit.clamp(1, 50));

        let fulltext_cypher = format!(
            "CALL db.index.fulltext.queryNodes('{ENTITY_SEARCH_INDEX}', $q) \
             YIELD node, score RETURN node, score ORDER BY score DESC LIMIT $limit"
        );
        let q = query(&fulltext_cypher)
            .param("q", text)
            .param("limit", limit);

        if let Ok(mut stream) = self.graph.execute(q).await {
            let mut results = Vec::new();
            while let Some(row) = stream
                .next()
                .await
                .map_err(|e| GraphError::Query(e.to_string()))?
            {
                let node: Node = row
                    .get("node")
                    .map_err(|e| GraphError::Query(e.to_string()))?;
                let score: f64 = row.get("score").unwrap_or(0.0);
                let mut value = node_to_json(&node)?;
                value["score"] = json!(score);
                results.push(value);
            }
            if !results.is_empty() {
                return Ok(results);
            }
        }

        let contains_cypher =
            "MATCH (n) WHERE toLower(n.name) CONTAINS toLower($q) RETURN n LIMIT $limit";
        let q = query(contains_cypher)
            .param("q", text)
            .param("limit", limit);
        let mut stream = self
            .graph
            .execute(q)
            .await
            .map_err(|e| GraphError::Query(e.to_string()))?;

        let mut results = Vec::new();
        while let Some(row) = stream
            .next()
            .await
            .map_err(|e| GraphError::Query(e.to_string()))?
        {
            let node: Node = row.get("n").map_err(|e| GraphError::Query(e.to_string()))?;
            results.push(node_to_json(&node)?);
        }
        Ok(results)
    }

    /// Traverse N hops (clamped to 1-3, N5) from an entity and return the
    /// reachable subgraph -- neighbor nodes plus edges among the returned
    /// node set (F4/F10).
    pub async fn entity_context(&self, name: &str, hops: u32) -> Result<Value, GraphError> {
        let hops = clamp_hops(hops);

        let cypher = format!(
            "MATCH (n {{name: $name}}) \
             OPTIONAL MATCH (n)-[*1..{hops}]-(m) \
             WITH n, collect(DISTINCT m) AS neighbors \
             WITH n, neighbors, [n] + neighbors AS all_nodes \
             UNWIND all_nodes AS a \
             UNWIND all_nodes AS b \
             OPTIONAL MATCH (a)-[r]->(b) \
             WITH n, neighbors, \
                  collect(DISTINCT CASE WHEN r IS NULL THEN NULL \
                          ELSE {{from: a.name, to: b.name, type: type(r)}} END) AS raw_edges \
             RETURN n, neighbors, [e IN raw_edges WHERE e IS NOT NULL] AS edges"
        );

        let q = query(&cypher).param("name", name);
        let row = self
            .execute_optional(q)
            .await?
            .ok_or_else(|| GraphError::NotFound(name.to_string()))?;

        let center: Node = row.get("n").map_err(|e| GraphError::Query(e.to_string()))?;
        let neighbors: Vec<Node> = row.get("neighbors").unwrap_or_default();
        let edges: Vec<Value> = row.get("edges").unwrap_or_default();

        let neighbors = neighbors
            .iter()
            .map(node_to_json)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(json!({
            "entity": node_to_json(&center)?,
            "hops": hops,
            "neighbors": neighbors,
            "relationships": edges,
        }))
    }

    /// Aggregate counts for the graph browser panel (F13): live entities and
    /// edges, plus pending staged candidates. `:GraphCandidate` nodes are
    /// staging-only and excluded from the entity count.
    pub async fn graph_stats(&self) -> Result<Value, GraphError> {
        let entities: i64 = self
            .execute_single(query(
                "MATCH (n) WHERE NOT n:GraphCandidate RETURN count(n) AS c",
            ))
            .await?
            .get("c")
            .map_err(|e| GraphError::Query(e.to_string()))?;

        let edges: i64 = self
            .execute_single(query("MATCH ()-[r]->() RETURN count(r) AS c"))
            .await?
            .get("c")
            .map_err(|e| GraphError::Query(e.to_string()))?;

        let pending_candidates: i64 = self
            .execute_single(query(
                "MATCH (c:GraphCandidate {status: 'pending'}) RETURN count(c) AS c",
            ))
            .await?
            .get("c")
            .map_err(|e| GraphError::Query(e.to_string()))?;

        Ok(json!({
            "entities": entities,
            "edges": edges,
            "pending_candidates": pending_candidates,
        }))
    }

    /// List entities (not candidates) with their degree (relationship count),
    /// ordered by degree descending, for the graph browser sidebar (F12).
    pub async fn list_entities(&self, limit: u32) -> Result<Vec<Value>, GraphError> {
        let limit = i64::from(limit.clamp(1, 2000));
        let cypher = "MATCH (n) WHERE NOT n:GraphCandidate \
             OPTIONAL MATCH (n)-[r]-() \
             WITH n, count(r) AS degree \
             RETURN n, degree ORDER BY degree DESC LIMIT $limit";
        let q = query(cypher).param("limit", limit);

        let mut stream = self
            .graph
            .execute(q)
            .await
            .map_err(|e| GraphError::Query(e.to_string()))?;

        let mut results = Vec::new();
        while let Some(row) = stream
            .next()
            .await
            .map_err(|e| GraphError::Query(e.to_string()))?
        {
            let node: Node = row.get("n").map_err(|e| GraphError::Query(e.to_string()))?;
            let degree: i64 = row.get("degree").unwrap_or(0);
            let mut value = node_to_json(&node)?;
            value["degree"] = json!(degree);
            results.push(value);
        }
        Ok(results)
    }

    /// A bounded sample of the graph for visualization (F12): the top-N
    /// entities by degree, plus every relationship where both endpoints are
    /// in that sample set. The edge lookup runs as a `CALL` subquery keyed
    /// on node id so it stays a single indexed match rather than the O(n^2)
    /// `UNWIND`-cross-product `entity_context` uses (fine there since it's
    /// scoped to one entity's small neighborhood; not fine at graph-wide
    /// scale), and so aggregation still returns a row with empty lists when
    /// the sample or its edges are empty, rather than dropping the row.
    pub async fn graph_sample(&self, limit: u32) -> Result<Value, GraphError> {
        let limit = i64::from(limit.clamp(1, 500));
        let cypher = "MATCH (n) WHERE NOT n:GraphCandidate \
             OPTIONAL MATCH (n)-[r]-() \
             WITH n, count(r) AS degree \
             ORDER BY degree DESC LIMIT $limit \
             WITH collect(id(n)) AS ids, collect(n) AS nodes \
             CALL { \
                 WITH ids \
                 MATCH (a)-[rel]->(b) WHERE id(a) IN ids AND id(b) IN ids \
                 RETURN collect({from: a.name, to: b.name, type: type(rel)}) AS edges \
             } \
             RETURN nodes, edges";
        let q = query(cypher).param("limit", limit);

        let Some(row) = self.execute_optional(q).await? else {
            return Ok(json!({ "nodes": [], "edges": [] }));
        };

        let nodes: Vec<Node> = row.get("nodes").unwrap_or_default();
        let edges: Vec<Value> = row.get("edges").unwrap_or_default();

        let nodes = nodes
            .iter()
            .map(node_to_json)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(json!({ "nodes": nodes, "edges": edges }))
    }

    /// Delete an entity and all its relationships (F6).
    pub async fn delete_entity(&self, label: &str, name: &str) -> Result<(), GraphError> {
        validate_label(label)?;

        // `count(n)` counts the matched rows before deletion, which is how
        // Neo4j distinguishes "no match" (0) from "deleted" (>0) in one query.
        let cypher =
            format!("MATCH (n:{label} {{name: $name}}) DETACH DELETE n RETURN count(n) AS n");
        let q = query(&cypher).param("name", name);
        let row = self
            .execute_optional(q)
            .await?
            .ok_or_else(|| GraphError::NotFound(name.to_string()))?;
        let matched: i64 = row.get("n").unwrap_or(0);
        if matched == 0 {
            return Err(GraphError::NotFound(name.to_string()));
        }
        Ok(())
    }

    /// Merge a duplicate entity into a canonical one, combining properties and
    /// re-pointing all relationships (F6). Requires the APOC plugin
    /// (`apoc.refactor.mergeNodes`) -- plain Cypher can't create relationships
    /// with a dynamic type, which is required to preserve the source node's
    /// edges on the target.
    pub async fn merge_entities(
        &self,
        label: &str,
        source_name: &str,
        target_name: &str,
    ) -> Result<Value, GraphError> {
        validate_label(label)?;

        let cypher = format!(
            "MATCH (source:{label} {{name: $source}}), (target:{label} {{name: $target}}) \
             CALL apoc.refactor.mergeNodes([source, target], \
                 {{properties: 'combine', mergeRels: true}}) \
             YIELD node RETURN node"
        );
        let q = query(&cypher)
            .param("source", source_name)
            .param("target", target_name);

        let row = self.execute_optional(q).await.map_err(|e| {
            if e.to_string().to_lowercase().contains("apoc") {
                GraphError::Query(format!(
                    "merge_entities requires the APOC plugin (apoc.refactor.mergeNodes) \
                     to be installed on the Neo4j server: {e}"
                ))
            } else {
                e
            }
        })?;
        let row = row.ok_or_else(|| {
            GraphError::NotFound(format!("'{source_name}' or '{target_name}' not found"))
        })?;

        let node: Node = row
            .get("node")
            .map_err(|e| GraphError::Query(e.to_string()))?;
        node_to_json(&node)
    }

    /// Stage a candidate batch of entities/relationships for review (F8/F9).
    /// Nothing is written to the live graph until `approve_candidate` is called.
    pub async fn stage_candidate(
        &self,
        entities: &[CandidateEntity],
        relationships: &[CandidateRelationship],
        source: &str,
        confidence: f64,
    ) -> Result<Value, GraphError> {
        for entity in entities {
            validate_label(&entity.label)?;
        }
        for rel in relationships {
            validate_rel_type(&rel.rel_type)?;
        }

        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let entities_json = serde_json::to_string(entities)
            .map_err(|e| GraphError::Query(format!("failed to serialize entities: {e}")))?;
        let relationships_json = serde_json::to_string(relationships)
            .map_err(|e| GraphError::Query(format!("failed to serialize relationships: {e}")))?;

        let cypher = "CREATE (c:GraphCandidate { \
                id: $id, source: $source, confidence: $confidence, status: 'pending', \
                entities_json: $entities_json, relationships_json: $relationships_json, \
                created_at: $now \
            }) RETURN c";
        let q = query(cypher)
            .param("id", id)
            .param("source", source)
            .param("confidence", confidence.clamp(0.0, 1.0))
            .param("entities_json", entities_json)
            .param("relationships_json", relationships_json)
            .param("now", now);

        let row = self.execute_single(q).await?;
        candidate_node_to_json(&row)
    }

    /// List staged candidates, optionally filtered by status (F9).
    pub async fn list_candidates(
        &self,
        status: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Value>, GraphError> {
        let limit = i64::from(limit.clamp(1, 100));
        let cypher = "MATCH (c:GraphCandidate) \
             WHERE $status IS NULL OR c.status = $status \
             RETURN c ORDER BY c.created_at DESC LIMIT $limit";
        let q = query(cypher).param("status", status).param("limit", limit);

        let mut stream = self
            .graph
            .execute(q)
            .await
            .map_err(|e| GraphError::Query(e.to_string()))?;

        let mut results = Vec::new();
        while let Some(row) = stream
            .next()
            .await
            .map_err(|e| GraphError::Query(e.to_string()))?
        {
            results.push(candidate_node_to_json(&row)?);
        }
        Ok(results)
    }

    /// Commit a pending candidate: replay its entities/relationships through
    /// `create_entity`/`create_relationship` (idempotent via `MERGE`), then
    /// mark it approved (F9). Per-item failures are collected rather than
    /// aborting the whole batch, since a bad relationship (e.g. a typo'd
    /// entity name) shouldn't discard otherwise-valid entities.
    pub async fn approve_candidate(&self, id: &str) -> Result<Value, GraphError> {
        let candidate = self.get_pending_candidate(id).await?;
        let entities: Vec<CandidateEntity> =
            serde_json::from_str(candidate["entities_json"].as_str().unwrap_or("[]"))
                .map_err(|e| GraphError::Query(format!("corrupt candidate entities: {e}")))?;
        let relationships: Vec<CandidateRelationship> =
            serde_json::from_str(candidate["relationships_json"].as_str().unwrap_or("[]"))
                .map_err(|e| GraphError::Query(format!("corrupt candidate relationships: {e}")))?;

        let mut committed_entities = Vec::new();
        let mut committed_relationships = Vec::new();
        let mut errors = Vec::new();

        for entity in &entities {
            match self
                .create_entity(&entity.label, &entity.name, entity.properties.clone())
                .await
            {
                Ok(v) => committed_entities.push(v),
                Err(e) => errors.push(json!({ "entity": entity.name, "error": e.to_string() })),
            }
        }
        for rel in &relationships {
            match self
                .create_relationship(
                    &rel.from_entity,
                    &rel.to_entity,
                    &rel.rel_type,
                    rel.properties.clone(),
                )
                .await
            {
                Ok(v) => committed_relationships.push(v),
                Err(e) => errors.push(json!({
                    "relationship": format!("{} -[{}]-> {}", rel.from_entity, rel.rel_type, rel.to_entity),
                    "error": e.to_string(),
                })),
            }
        }

        let now = chrono::Utc::now().to_rfc3339();
        let q = query(
            "MATCH (c:GraphCandidate {id: $id, status: 'pending'}) \
             SET c.status = 'approved', c.reviewed_at = $now RETURN c",
        )
        .param("id", id)
        .param("now", now);
        self.execute_single(q).await?;

        Ok(json!({
            "id": id,
            "committed_entities": committed_entities,
            "committed_relationships": committed_relationships,
            "errors": errors,
        }))
    }

    /// Edit a pending candidate's staged entities/relationships before approval
    /// (F9 review UI). Only the fields provided are replaced; omitting one
    /// leaves it as-is. Used to fix extraction naming inconsistencies (e.g.
    /// the same project staged under two different names across runs) by
    /// renaming an entity to the canonical name before approving, so
    /// `create_entity`'s `MERGE` collapses it into the existing node instead
    /// of creating a duplicate.
    pub async fn update_candidate(
        &self,
        id: &str,
        entities: Option<&[CandidateEntity]>,
        relationships: Option<&[CandidateRelationship]>,
    ) -> Result<Value, GraphError> {
        self.get_pending_candidate(id).await?;

        if let Some(entities) = entities {
            for entity in entities {
                validate_label(&entity.label)?;
            }
        }
        if let Some(relationships) = relationships {
            for rel in relationships {
                validate_rel_type(&rel.rel_type)?;
            }
        }

        let entities_json = entities
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| GraphError::Query(format!("failed to serialize entities: {e}")))?;
        let relationships_json = relationships
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| GraphError::Query(format!("failed to serialize relationships: {e}")))?;

        let mut set_clauses = Vec::new();
        if entities_json.is_some() {
            set_clauses.push("c.entities_json = $entities_json");
        }
        if relationships_json.is_some() {
            set_clauses.push("c.relationships_json = $relationships_json");
        }
        if set_clauses.is_empty() {
            return self.get_pending_candidate(id).await;
        }

        let cypher = format!(
            "MATCH (c:GraphCandidate {{id: $id, status: 'pending'}}) SET {} RETURN c",
            set_clauses.join(", ")
        );
        let mut q = query(&cypher).param("id", id);
        if let Some(entities_json) = entities_json {
            q = q.param("entities_json", entities_json);
        }
        if let Some(relationships_json) = relationships_json {
            q = q.param("relationships_json", relationships_json);
        }

        let row = self.execute_single(q).await?;
        candidate_node_to_json(&row)
    }

    /// Reject a pending candidate without committing anything (F9). Kept
    /// (status set to `rejected`) rather than deleted, for audit purposes.
    pub async fn reject_candidate(&self, id: &str) -> Result<Value, GraphError> {
        self.get_pending_candidate(id).await?;

        let now = chrono::Utc::now().to_rfc3339();
        let q = query(
            "MATCH (c:GraphCandidate {id: $id, status: 'pending'}) \
             SET c.status = 'rejected', c.reviewed_at = $now RETURN c",
        )
        .param("id", id)
        .param("now", now);

        let row = self.execute_single(q).await?;
        candidate_node_to_json(&row)
    }

    async fn get_pending_candidate(&self, id: &str) -> Result<Value, GraphError> {
        let q = query("MATCH (c:GraphCandidate {id: $id}) RETURN c").param("id", id);
        let row = self
            .execute_optional(q)
            .await?
            .ok_or_else(|| GraphError::NotFound(id.to_string()))?;
        let candidate = candidate_node_to_json(&row)?;
        if candidate["status"] != "pending" {
            return Err(GraphError::Query(format!(
                "candidate '{id}' is not pending (status: {})",
                candidate["status"]
            )));
        }
        Ok(candidate)
    }

    /// Run a query expected to return exactly one row.
    async fn execute_single(&self, q: neo4rs::Query) -> Result<neo4rs::Row, GraphError> {
        self.execute_optional(q)
            .await?
            .ok_or_else(|| GraphError::Query("query returned no rows".to_string()))
    }

    /// Run a query and return its first row, if any.
    async fn execute_optional(&self, q: neo4rs::Query) -> Result<Option<neo4rs::Row>, GraphError> {
        let mut stream = self
            .graph
            .execute(q)
            .await
            .map_err(|e| GraphError::Query(e.to_string()))?;
        stream
            .next()
            .await
            .map_err(|e| GraphError::Query(e.to_string()))
    }
}

/// Convert a `Node` into a JSON representation: id, labels, and properties.
fn node_to_json(node: &Node) -> Result<Value, GraphError> {
    let properties: Value = node
        .to::<Value>()
        .map_err(|e| GraphError::Query(format!("failed to decode node: {e}")))?;

    // Every entity carries both its semantic type label (e.g. "Project") and
    // the common "Entity" label (added by `create_entity` so the fulltext
    // search index has one label to target). Neo4j does not guarantee label
    // order is preserved, so callers picking `labels[0]` as "the type" for
    // e.g. UI color-coding would get an unpredictable mix of the two -- drop
    // "Entity" here so the first (and normally only) label left is always
    // the meaningful one.
    let labels: Vec<&str> = node
        .labels()
        .into_iter()
        .filter(|l| *l != "Entity")
        .collect();

    Ok(json!({
        "id": node.id(),
        "labels": labels,
        "properties": properties,
    }))
}

/// Extract a `:GraphCandidate` row's properties as JSON (id, source,
/// confidence, status, entities_json, relationships_json, timestamps).
fn candidate_node_to_json(row: &neo4rs::Row) -> Result<Value, GraphError> {
    let node: Node = row.get("c").map_err(|e| GraphError::Query(e.to_string()))?;
    node.to::<Value>()
        .map_err(|e| GraphError::Query(format!("failed to decode candidate: {e}")))
}
