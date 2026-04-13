//! Web search tool with pluggable backends.
//!
//! Supports three backends, selected at construction time from operator-supplied
//! config (never from LLM input):
//!
//! | Backend    | Env var               | Notes                                     |
//! |------------|-----------------------|-------------------------------------------|
//! | SearXNG    | `SEARXNG_URL`         | Self-hosted, no rate limits, HTTP-ok      |
//! | Brave      | `BRAVE_SEARCH_API_KEY`| Free tier: 2 000 req/month                |
//! | Tavily     | `TAVILY_API_KEY`      | Free tier: 1 000 req/month, AI-optimised  |
//!
//! Because the endpoint / credentials are set by the operator rather than by the
//! LLM, SSRF and private-IP restrictions that apply to the generic `http` tool
//! are not relevant here.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

use crate::context::JobContext;
use crate::tools::tool::{Tool, ToolError, ToolOutput};

/// Maximum results the LLM may request.
const MAX_RESULTS: usize = 10;

// ---------------------------------------------------------------------------
// Backend enum
// ---------------------------------------------------------------------------

/// Which search backend to use.
#[derive(Debug, Clone)]
pub enum SearchBackend {
    /// Self-hosted SearXNG instance.
    Searxng {
        /// Base URL, e.g. `http://10.0.0.2:8888` (trailing slash stripped).
        base_url: String,
    },
    /// Brave Search REST API.
    Brave {
        api_key: String,
    },
    /// Tavily AI Search API.
    Tavily {
        api_key: String,
    },
}

impl SearchBackend {
    /// Human-readable name shown in logs.
    pub fn name(&self) -> &str {
        match self {
            SearchBackend::Searxng { .. } => "SearXNG",
            SearchBackend::Brave { .. } => "Brave Search",
            SearchBackend::Tavily { .. } => "Tavily",
        }
    }
}

// ---------------------------------------------------------------------------
// Per-backend response deserialization helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SearxResult {
    title: String,
    url: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    score: f64,
}

#[derive(Debug, Deserialize)]
struct SearxResponse {
    results: Vec<SearxResult>,
}

// Brave nested: { web: { results: [...] } }
#[derive(Debug, Deserialize)]
struct BraveWebResults {
    results: Vec<BraveResult>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
    title: String,
    url: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    score: f64,
}

#[derive(Debug, Deserialize)]
struct BraveResponse {
    web: Option<BraveWebResults>,
}

// Tavily: { results: [...] }
#[derive(Debug, Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    score: f64,
}

#[derive(Debug, Deserialize)]
struct TavilyResponse {
    results: Vec<TavilyResult>,
}

// ---------------------------------------------------------------------------
// Tool
// ---------------------------------------------------------------------------

/// Tool for searching the web via a configured search backend.
pub struct WebSearchTool {
    backend: SearchBackend,
    client: Client,
}

impl WebSearchTool {
    /// Create a new tool using the given backend.
    pub fn new(backend: SearchBackend) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("failed to build HTTP client for web search");

        Self { backend, client }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for current information. Returns titles, URLs, and snippets for \
         the most relevant results. Use this whenever you need up-to-date information, \
         facts, documentation, product details, prices, or anything outside your training \
         data. Prefer this over guessing."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "num_results": {
                    "type": "integer",
                    "description": "Number of results to return (default: 5, max: 10)",
                    "minimum": 1,
                    "maximum": 10
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
            .ok_or_else(|| {
                ToolError::InvalidParameters("missing 'query' parameter".to_string())
            })?;

        if query.trim().is_empty() {
            return Err(ToolError::InvalidParameters(
                "query must not be empty".to_string(),
            ));
        }

        let num_results = params
            .get("num_results")
            .and_then(|v| v.as_u64())
            .map(|n| n.min(MAX_RESULTS as u64) as usize)
            .unwrap_or(5);

        let results = match &self.backend {
            SearchBackend::Searxng { base_url } => {
                search_searxng(&self.client, base_url, query, num_results).await?
            }
            SearchBackend::Brave { api_key } => {
                search_brave(&self.client, api_key, query, num_results).await?
            }
            SearchBackend::Tavily { api_key } => {
                search_tavily(&self.client, api_key, query, num_results).await?
            }
        };

        if results.is_empty() {
            return Ok(ToolOutput::text(
                format!("No results found for: {}", query),
                start.elapsed(),
            ));
        }

        let output = serde_json::json!({
            "query": query,
            "result_count": results.len(),
            "results": results,
        });

        Ok(ToolOutput::success(output, start.elapsed()))
    }

    fn estimated_duration(&self, _params: &serde_json::Value) -> Option<Duration> {
        Some(Duration::from_secs(3))
    }

    fn requires_sanitization(&self) -> bool {
        true // Web results are external data
    }

    fn requires_approval(&self) -> bool {
        false // Admin-configured backend, no per-query approval needed
    }
}

// ---------------------------------------------------------------------------
// Backend implementations
// ---------------------------------------------------------------------------

async fn search_searxng(
    client: &Client,
    base_url: &str,
    query: &str,
    num_results: usize,
) -> Result<Vec<serde_json::Value>, ToolError> {
    let url = format!("{}/search", base_url.trim_end_matches('/'));

    let resp = client
        .get(&url)
        .query(&[("q", query), ("format", "json")])
        .send()
        .await
        .map_err(map_request_err)?;

    check_status(&resp, "SearXNG")?;

    let body: SearxResponse = resp.json().await.map_err(|e| {
        ToolError::ExternalService(format!("failed to parse SearXNG response: {}", e))
    })?;

    Ok(body
        .results
        .into_iter()
        .take(num_results)
        .map(|r| {
            serde_json::json!({
                "title": r.title,
                "url": r.url,
                "snippet": r.content,
                "score": r.score,
            })
        })
        .collect())
}

async fn search_brave(
    client: &Client,
    api_key: &str,
    query: &str,
    num_results: usize,
) -> Result<Vec<serde_json::Value>, ToolError> {
    let resp = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", api_key)
        .header("Accept", "application/json")
        .query(&[("q", query), ("count", &num_results.to_string())])
        .send()
        .await
        .map_err(map_request_err)?;

    check_status(&resp, "Brave Search")?;

    let body: BraveResponse = resp.json().await.map_err(|e| {
        ToolError::ExternalService(format!("failed to parse Brave Search response: {}", e))
    })?;

    let items = body.web.map(|w| w.results).unwrap_or_default();
    Ok(items
        .into_iter()
        .take(num_results)
        .map(|r| {
            serde_json::json!({
                "title": r.title,
                "url": r.url,
                "snippet": r.description,
                "score": r.score,
            })
        })
        .collect())
}

async fn search_tavily(
    client: &Client,
    api_key: &str,
    query: &str,
    num_results: usize,
) -> Result<Vec<serde_json::Value>, ToolError> {
    let resp = client
        .post("https://api.tavily.com/search")
        .json(&serde_json::json!({
            "api_key": api_key,
            "query": query,
            "max_results": num_results,
            "search_depth": "basic",
        }))
        .send()
        .await
        .map_err(map_request_err)?;

    check_status(&resp, "Tavily")?;

    let body: TavilyResponse = resp.json().await.map_err(|e| {
        ToolError::ExternalService(format!("failed to parse Tavily response: {}", e))
    })?;

    Ok(body
        .results
        .into_iter()
        .take(num_results)
        .map(|r| {
            serde_json::json!({
                "title": r.title,
                "url": r.url,
                "snippet": r.content,
                "score": r.score,
            })
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn map_request_err(e: reqwest::Error) -> ToolError {
    if e.is_timeout() {
        ToolError::Timeout(Duration::from_secs(15))
    } else {
        ToolError::ExternalService(format!("search request failed: {}", e))
    }
}

fn check_status(resp: &reqwest::Response, service: &str) -> Result<(), ToolError> {
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(ToolError::NotAuthorized(format!(
            "{} rejected the request (HTTP {}) — check your API key",
            service, status
        )));
    }
    if !status.is_success() {
        return Err(ToolError::ExternalService(format!(
            "{} returned HTTP {}",
            service, status
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_requires_query() {
        let tool = WebSearchTool::new(SearchBackend::Tavily {
            api_key: "test".to_string(),
        });
        let schema = tool.parameters_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("query")));
    }

    #[test]
    fn test_backend_names() {
        assert_eq!(
            SearchBackend::Searxng {
                base_url: "http://localhost".to_string()
            }
            .name(),
            "SearXNG"
        );
        assert_eq!(
            SearchBackend::Brave {
                api_key: "k".to_string()
            }
            .name(),
            "Brave Search"
        );
        assert_eq!(
            SearchBackend::Tavily {
                api_key: "k".to_string()
            }
            .name(),
            "Tavily"
        );
    }
}
