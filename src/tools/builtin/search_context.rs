//! Composite search across workspace memory and the knowledge graph.
//!
//! Memory (unstructured chat/notes, hybrid FTS+vector search) and the
//! knowledge graph (structured entities/relationships) are complementary but
//! previously had no code linkage -- `memory_search` never surfaced related
//! graph entities and vice versa. `search_context` fans both out in parallel
//! and returns merged, labeled results in one call, so relationship context
//! doesn't depend on the model remembering to call two separate tools.

use std::sync::Arc;

use async_trait::async_trait;

use crate::context::JobContext;
use crate::graph::GraphClient;
use crate::tools::tool::{Tool, ToolError, ToolOutput};
use crate::workspace::Workspace;

/// Searches both workspace memory and the knowledge graph for a query.
pub struct SearchContextTool {
    workspace: Arc<Workspace>,
    graph_client: Arc<GraphClient>,
}

impl SearchContextTool {
    pub fn new(workspace: Arc<Workspace>, graph_client: Arc<GraphClient>) -> Self {
        Self {
            workspace,
            graph_client,
        }
    }
}

#[async_trait]
impl Tool for SearchContextTool {
    fn name(&self) -> &str {
        "search_context"
    }

    fn description(&self) -> &str {
        "Search both workspace memory (chat history, notes) and the knowledge graph \
         (entities and their relationships) in a single call. Prefer this over \
         memory_search alone whenever the question touches people, projects, \
         organizations, or how things relate to each other -- it grounds the answer in \
         both prior conversation and accumulated relationship context at once."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query. Use natural language to describe what you're looking for."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results per source (default: 5, max: 20)",
                    "default": 5,
                    "minimum": 1,
                    "maximum": 20
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

        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParameters("missing 'query' parameter".to_string()))?;

        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .min(20) as usize;

        let (memory_result, graph_result) = tokio::join!(
            self.workspace.search(query, limit),
            self.graph_client.search_entities(query, limit as u32)
        );

        let mut output = serde_json::json!({ "query": query });

        match memory_result {
            Ok(results) => {
                output["memory_results"] = serde_json::json!(
                    results
                        .iter()
                        .map(|r| serde_json::json!({
                            "content": r.content,
                            "score": r.score,
                            "document_id": r.document_id.to_string(),
                            "is_hybrid_match": r.is_hybrid(),
                        }))
                        .collect::<Vec<_>>()
                );
                output["memory_count"] = serde_json::json!(results.len());
            }
            Err(e) => {
                output["memory_error"] = serde_json::json!(e.to_string());
            }
        }

        match graph_result {
            Ok(results) => {
                output["graph_count"] = serde_json::json!(results.len());
                output["graph_results"] = serde_json::json!(results);
            }
            Err(e) => {
                output["graph_error"] = serde_json::json!(e.to_string());
            }
        }

        Ok(ToolOutput::success(output, start.elapsed()))
    }

    fn requires_sanitization(&self) -> bool {
        false // Internal memory + graph, trusted content (same as memory_search/search_entities)
    }
}
