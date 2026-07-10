//! Neo4j-backed knowledge graph client.
//!
//! Wraps `neo4rs::Graph` with the entity/relationship operations the graph
//! tools need (`src/tools/builtin/graph.rs`). All node labels and
//! relationship types passed through here are validated via
//! `crate::graph::validate` before being interpolated into Cypher, since
//! Neo4j does not support parameter binding for labels/relationship types.

use neo4rs::{Graph, Node, query};
use serde_json::{Value, json};

use crate::config::Neo4jConfig;
use crate::graph::error::GraphError;
use crate::graph::validate::{clamp_hops, validate_label, validate_rel_type};

/// Name of the full-text index used by `search_entities`. Created on connect.
const ENTITY_SEARCH_INDEX: &str = "entity_search";

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
        let cypher = format!(
            "CREATE FULLTEXT INDEX {ENTITY_SEARCH_INDEX} IF NOT EXISTS FOR (n) ON EACH [n.name]"
        );
        self.graph
            .run(query(&cypher))
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

        let cypher = format!(
            "MERGE (n:{label} {{name: $name}}) \
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

    Ok(json!({
        "id": node.id(),
        "labels": node.labels(),
        "properties": properties,
    }))
}
