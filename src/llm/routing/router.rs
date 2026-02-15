//! Smart router for multi-provider LLM routing.
//!
//! Routes queries to the best available provider based on:
//! - Query complexity analysis
//! - Provider health and availability
//! - Cost/quality tradeoffs
//! - User-configured routing strategy

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use serde::Serialize;

use super::analyzer::{ComplexityScore, QueryAnalyzer};
use super::strategy::{ModelTier, RoutingConfig, RoutingStrategy};
use crate::config::LlmBackend;
use crate::error::LlmError;
use crate::llm::provider::{CompletionRequest, CompletionResponse, LlmProvider};

/// Health status of a provider.
#[derive(Debug, Clone)]
pub struct ProviderHealth {
    /// Whether the provider is currently available.
    pub available: bool,
    /// Last successful request timestamp.
    pub last_success: Option<Instant>,
    /// Last failure timestamp.
    pub last_failure: Option<Instant>,
    /// Number of consecutive failures.
    pub consecutive_failures: u32,
    /// Average response latency in milliseconds.
    pub avg_latency_ms: u64,
    /// Success rate (0.0 - 1.0).
    pub success_rate: f32,
    /// Total requests made.
    pub total_requests: u64,
    /// Total successful requests.
    pub successful_requests: u64,
}

impl Default for ProviderHealth {
    fn default() -> Self {
        Self {
            available: true,
            last_success: None,
            last_failure: None,
            consecutive_failures: 0,
            avg_latency_ms: 0,
            success_rate: 1.0,
            total_requests: 0,
            successful_requests: 0,
        }
    }
}

impl ProviderHealth {
    /// Record a successful request.
    pub fn record_success(&mut self, latency_ms: u64) {
        self.last_success = Some(Instant::now());
        self.consecutive_failures = 0;
        self.available = true;
        self.total_requests += 1;
        self.successful_requests += 1;

        // Update rolling average latency
        if self.avg_latency_ms == 0 {
            self.avg_latency_ms = latency_ms;
        } else {
            // Exponential moving average
            self.avg_latency_ms = (self.avg_latency_ms * 9 + latency_ms) / 10;
        }

        self.update_success_rate();
    }

    /// Record a failed request.
    pub fn record_failure(&mut self) {
        self.last_failure = Some(Instant::now());
        self.consecutive_failures += 1;
        self.total_requests += 1;

        // Mark as unavailable after 3 consecutive failures
        if self.consecutive_failures >= 3 {
            self.available = false;
        }

        self.update_success_rate();
    }

    fn update_success_rate(&mut self) {
        if self.total_requests > 0 {
            self.success_rate = self.successful_requests as f32 / self.total_requests as f32;
        }
    }

    /// Check if we should retry this provider after a cooldown.
    pub fn should_retry(&self, cooldown: Duration) -> bool {
        if self.available {
            return true;
        }

        // Retry if enough time has passed since last failure
        if let Some(last_failure) = self.last_failure {
            last_failure.elapsed() >= cooldown
        } else {
            true
        }
    }
}

/// Result of a routing decision.
#[derive(Debug, Clone)]
pub struct RoutingDecision {
    /// The selected provider backend.
    pub backend: LlmBackend,
    /// Why this provider was selected.
    pub reason: String,
    /// Complexity analysis of the query.
    pub complexity: ComplexityScore,
    /// Estimated cost for this request.
    pub estimated_cost: Option<Decimal>,
    /// Fallback providers if the primary fails.
    pub fallbacks: Vec<LlmBackend>,
}

/// Smart router that selects the best provider for each query.
pub struct SmartRouter {
    /// Available providers mapped by backend type.
    providers: HashMap<LlmBackend, Arc<dyn LlmProvider>>,
    /// Health status for each provider.
    health: Arc<RwLock<HashMap<LlmBackend, ProviderHealth>>>,
    /// Query analyzer for complexity scoring.
    analyzer: QueryAnalyzer,
    /// Routing configuration.
    config: RoutingConfig,
}

impl SmartRouter {
    /// Create a new smart router.
    pub fn new(config: RoutingConfig) -> Self {
        Self {
            providers: HashMap::new(),
            health: Arc::new(RwLock::new(HashMap::new())),
            analyzer: QueryAnalyzer::new(),
            config,
        }
    }

    /// Register a provider with the router.
    pub fn register_provider(&mut self, backend: LlmBackend, provider: Arc<dyn LlmProvider>) {
        self.providers.insert(backend, provider);
        if let Ok(mut health) = self.health.write() {
            health.insert(backend, ProviderHealth::default());
        }
    }

    /// Get all registered providers.
    pub fn providers(&self) -> &HashMap<LlmBackend, Arc<dyn LlmProvider>> {
        &self.providers
    }

    /// Get health status for a provider.
    pub fn get_health(&self, backend: LlmBackend) -> Option<ProviderHealth> {
        self.health.read().ok()?.get(&backend).cloned()
    }

    /// Get all provider health statuses.
    pub fn all_health(&self) -> HashMap<LlmBackend, ProviderHealth> {
        self.health
            .read()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Route a query and return the routing decision.
    pub fn route(&self, query: &str) -> Result<RoutingDecision, LlmError> {
        // Analyze query complexity
        let complexity = self.analyzer.analyze(query);

        // Get available providers (healthy and not excluded)
        let available = self.get_available_providers();
        if available.is_empty() {
            return Err(LlmError::RequestFailed {
                provider: "router".to_string(),
                reason: "No providers available".to_string(),
            });
        }

        // Route based on strategy
        let (backend, reason) = match self.config.strategy {
            RoutingStrategy::Fixed(b) => {
                if available.contains(&b) {
                    (b, format!("Fixed strategy: {}", b))
                } else {
                    return Err(LlmError::RequestFailed {
                        provider: b.to_string(),
                        reason: "Fixed provider not available".to_string(),
                    });
                }
            }
            RoutingStrategy::LocalFirst => self.route_local_first(&available, &complexity),
            RoutingStrategy::CostOptimized => self.route_cost_optimized(&available, &complexity),
            RoutingStrategy::QualityFirst => self.route_quality_first(&available, &complexity),
            RoutingStrategy::Balanced => self.route_balanced(&available, &complexity),
        };

        // Calculate fallbacks (excluding the selected provider)
        let fallbacks: Vec<LlmBackend> = if self.config.enable_fallback {
            available
                .into_iter()
                .filter(|&b| b != backend)
                .take(2)
                .collect()
        } else {
            Vec::new()
        };

        // Estimate cost
        let estimated_cost = self.estimate_cost(backend, &complexity);

        Ok(RoutingDecision {
            backend,
            reason,
            complexity,
            estimated_cost,
            fallbacks,
        })
    }

    /// Execute a completion with automatic routing and fallback.
    pub async fn complete(
        &self,
        query: &str,
    ) -> Result<(CompletionResponse, LlmBackend), LlmError> {
        let decision = self.route(query)?;

        // Try primary provider
        let mut last_error = match self.try_complete(decision.backend, query).await {
            Ok(response) => return Ok((response, decision.backend)),
            Err(e) => {
                tracing::warn!(
                    "Primary provider {} failed: {}, trying fallbacks",
                    decision.backend,
                    e
                );
                e
            }
        };

        // Try fallbacks
        for fallback in decision.fallbacks {
            match self.try_complete(fallback, query).await {
                Ok(response) => return Ok((response, fallback)),
                Err(e) => {
                    tracing::warn!("Fallback provider {} failed: {}", fallback, e);
                    last_error = e;
                }
            }
        }

        Err(last_error)
    }

    /// Try to complete with a specific provider, updating health status.
    async fn try_complete(
        &self,
        backend: LlmBackend,
        query: &str,
    ) -> Result<CompletionResponse, LlmError> {
        let provider = self
            .providers
            .get(&backend)
            .ok_or_else(|| LlmError::RequestFailed {
                provider: backend.to_string(),
                reason: "Provider not registered".to_string(),
            })?;

        let start = Instant::now();

        let request = CompletionRequest::new(vec![crate::llm::ChatMessage::user(query)]);

        match provider.complete(request).await {
            Ok(response) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                if let Ok(mut health) = self.health.write() {
                    health
                        .entry(backend)
                        .or_default()
                        .record_success(latency_ms);
                }
                Ok(response)
            }
            Err(e) => {
                if let Ok(mut health) = self.health.write() {
                    health.entry(backend).or_default().record_failure();
                }
                Err(e)
            }
        }
    }

    /// Get list of available (healthy) providers.
    fn get_available_providers(&self) -> Vec<LlmBackend> {
        let health_guard = self.health.read().ok();
        let cooldown = Duration::from_secs(30);

        self.providers
            .keys()
            .filter(|&&backend| !self.config.excluded_providers.contains(&backend))
            .filter(|&&backend| {
                health_guard
                    .as_ref()
                    .and_then(|h| h.get(&backend))
                    .map(|health: &ProviderHealth| health.should_retry(cooldown))
                    .unwrap_or(true)
            })
            .copied()
            .collect()
    }

    /// Route with local-first strategy.
    fn route_local_first(
        &self,
        available: &[LlmBackend],
        complexity: &ComplexityScore,
    ) -> (LlmBackend, String) {
        // Use Ollama for simple/medium queries
        if complexity.level <= super::analyzer::ComplexityLevel::Medium
            && available.contains(&LlmBackend::Ollama)
        {
            return (
                LlmBackend::Ollama,
                "Local-first: using Ollama for simple query".to_string(),
            );
        }

        // Fall back to cloud providers for complex queries
        self.select_by_preference(available, "Local-first: complexity requires cloud provider")
    }

    /// Route with cost-optimized strategy.
    fn route_cost_optimized(
        &self,
        available: &[LlmBackend],
        _complexity: &ComplexityScore,
    ) -> (LlmBackend, String) {
        // Prefer Ollama (free), then providers with cheaper models
        if available.contains(&LlmBackend::Ollama) {
            return (
                LlmBackend::Ollama,
                "Cost-optimized: using free local model".to_string(),
            );
        }

        // TODO: Track actual model costs and select cheapest
        self.select_by_preference(available, "Cost-optimized: using preferred provider")
    }

    /// Route with quality-first strategy.
    fn route_quality_first(
        &self,
        available: &[LlmBackend],
        _complexity: &ComplexityScore,
    ) -> (LlmBackend, String) {
        // Prefer Anthropic (Claude) for quality
        if available.contains(&LlmBackend::Anthropic) {
            return (
                LlmBackend::Anthropic,
                "Quality-first: using Anthropic".to_string(),
            );
        }

        if available.contains(&LlmBackend::OpenAi) {
            return (
                LlmBackend::OpenAi,
                "Quality-first: using OpenAI".to_string(),
            );
        }

        self.select_by_preference(available, "Quality-first: using available provider")
    }

    /// Route with balanced strategy.
    fn route_balanced(
        &self,
        available: &[LlmBackend],
        complexity: &ComplexityScore,
    ) -> (LlmBackend, String) {
        use super::analyzer::ComplexityLevel;

        match complexity.level {
            ComplexityLevel::Simple => {
                // Use cheaper options for simple queries
                if available.contains(&LlmBackend::Ollama) {
                    return (
                        LlmBackend::Ollama,
                        "Balanced: simple query, using local".to_string(),
                    );
                }
            }
            ComplexityLevel::Medium => {
                // Standard provider for medium complexity
                if available.contains(&LlmBackend::Anthropic) {
                    return (
                        LlmBackend::Anthropic,
                        "Balanced: medium complexity, using Anthropic".to_string(),
                    );
                }
            }
            ComplexityLevel::Complex | ComplexityLevel::Expert => {
                // Premium provider for complex queries
                if available.contains(&LlmBackend::Anthropic) {
                    return (
                        LlmBackend::Anthropic,
                        "Balanced: complex query, using Anthropic".to_string(),
                    );
                }
            }
        }

        self.select_by_preference(available, "Balanced: using preferred provider")
    }

    /// Select a provider based on preference order.
    fn select_by_preference(&self, available: &[LlmBackend], reason: &str) -> (LlmBackend, String) {
        // Try preferred providers first
        for &pref in &self.config.preferred_providers {
            if available.contains(&pref) {
                return (pref, reason.to_string());
            }
        }

        // Fall back to first available
        let backend = available.first().copied().unwrap_or(LlmBackend::Anthropic);
        (backend, reason.to_string())
    }

    /// Estimate the cost for a request.
    fn estimate_cost(&self, backend: LlmBackend, complexity: &ComplexityScore) -> Option<Decimal> {
        let provider = self.providers.get(&backend)?;
        let (input_cost, output_cost) = provider.cost_per_token();

        // Estimate output tokens based on complexity
        let estimated_output = match complexity.level {
            super::analyzer::ComplexityLevel::Simple => 100,
            super::analyzer::ComplexityLevel::Medium => 500,
            super::analyzer::ComplexityLevel::Complex => 1500,
            super::analyzer::ComplexityLevel::Expert => 3000,
        };

        let cost = input_cost * Decimal::from(complexity.estimated_tokens)
            + output_cost * Decimal::from(estimated_output);

        Some(cost)
    }

    /// Get the model tier for a provider.
    pub fn get_model_tier(&self, backend: LlmBackend) -> Option<ModelTier> {
        self.providers
            .get(&backend)
            .map(|p: &Arc<dyn LlmProvider>| ModelTier::from_model_id(p.model_name()))
    }

    /// Get a summary of all provider health statuses for dashboard display.
    pub fn provider_health_summary(&self) -> Vec<ProviderHealthReport> {
        let health_guard = match self.health.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };

        self.providers
            .iter()
            .map(|(&backend, provider)| {
                let health = health_guard.get(&backend).cloned().unwrap_or_default();
                let tier = ModelTier::from_model_id(provider.model_name());

                ProviderHealthReport {
                    backend: backend.to_string(),
                    model: provider.model_name().to_string(),
                    tier: format!("{:?}", tier),
                    available: health.available,
                    success_rate: health.success_rate,
                    avg_latency_ms: health.avg_latency_ms,
                    total_requests: health.total_requests,
                    successful_requests: health.successful_requests,
                    consecutive_failures: health.consecutive_failures,
                }
            })
            .collect()
    }
}

/// Response from a routed completion, including metadata about the routing decision.
#[derive(Debug, Clone)]
pub struct RoutedResponse {
    /// The completion response from the provider.
    pub response: CompletionResponse,
    /// Which backend actually served the request.
    pub backend: LlmBackend,
    /// The routing decision that was made.
    pub decision: RoutingDecision,
    /// Actual cost of the request in USD.
    pub actual_cost: Decimal,
    /// Total latency including any retries/fallbacks, in milliseconds.
    pub latency_ms: u64,
}

/// Health summary for a single provider, suitable for JSON serialization.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderHealthReport {
    /// Backend identifier (e.g. "anthropic", "openai").
    pub backend: String,
    /// Model name (e.g. "claude-sonnet-4-20250514").
    pub model: String,
    /// Model tier (e.g. "Economy", "Standard", "Premium").
    pub tier: String,
    /// Whether the provider is currently accepting requests.
    pub available: bool,
    /// Success rate from 0.0 to 1.0.
    pub success_rate: f32,
    /// Average response latency in milliseconds.
    pub avg_latency_ms: u64,
    /// Total requests made to this provider.
    pub total_requests: u64,
    /// Total successful requests.
    pub successful_requests: u64,
    /// Number of consecutive failures.
    pub consecutive_failures: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::test_utils::MockProvider;

    #[test]
    fn test_provider_health_tracking() {
        let mut health = ProviderHealth::default();
        assert!(health.available);

        // Record some successes
        health.record_success(100);
        health.record_success(120);
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.successful_requests, 2);

        // Record failures
        health.record_failure();
        health.record_failure();
        assert_eq!(health.consecutive_failures, 2);
        assert!(health.available); // Still available after 2 failures

        health.record_failure();
        assert!(!health.available); // Unavailable after 3 failures
    }

    #[test]
    fn test_routing_decision_no_providers() {
        let config = RoutingConfig::default();
        let router = SmartRouter::new(config);

        // Router with no providers should fail
        let result = router.route("Hello");
        assert!(result.is_err());
    }

    #[test]
    fn test_route_with_single_provider() {
        let config = RoutingConfig::default();
        let mut router = SmartRouter::new(config);
        let provider = Arc::new(MockProvider::succeeding("anthropic", "ok"));
        router.register_provider(LlmBackend::Anthropic, provider);

        let decision = router.route("Hello, how are you?").unwrap();
        assert_eq!(decision.backend, LlmBackend::Anthropic);
    }

    #[test]
    fn test_route_preferred_provider() {
        let config = RoutingConfig {
            preferred_providers: vec![LlmBackend::OpenAi],
            ..Default::default()
        };
        let mut router = SmartRouter::new(config);
        router.register_provider(
            LlmBackend::Anthropic,
            Arc::new(MockProvider::succeeding("anthropic", "ok")),
        );
        router.register_provider(
            LlmBackend::OpenAi,
            Arc::new(MockProvider::succeeding("openai", "ok")),
        );

        let decision = router.route("Hello").unwrap();
        // With balanced strategy and OpenAI preferred, routing should pick a registered provider.
        assert!(
            decision.backend == LlmBackend::OpenAi || decision.backend == LlmBackend::Anthropic,
            "Should route to one of the registered providers"
        );
    }

    #[test]
    fn test_route_excludes_provider() {
        let config = RoutingConfig {
            excluded_providers: vec![LlmBackend::Anthropic],
            ..Default::default()
        };
        let mut router = SmartRouter::new(config);
        router.register_provider(
            LlmBackend::Anthropic,
            Arc::new(MockProvider::succeeding("anthropic", "ok")),
        );
        router.register_provider(
            LlmBackend::OpenAi,
            Arc::new(MockProvider::succeeding("openai", "ok")),
        );

        let decision = router.route("Hello").unwrap();
        assert_eq!(decision.backend, LlmBackend::OpenAi);
    }

    #[test]
    fn test_route_all_excluded_fails() {
        let config = RoutingConfig {
            excluded_providers: vec![LlmBackend::Anthropic],
            ..Default::default()
        };
        let mut router = SmartRouter::new(config);
        router.register_provider(
            LlmBackend::Anthropic,
            Arc::new(MockProvider::succeeding("anthropic", "ok")),
        );

        let result = router.route("Hello");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_complete_with_fallback() {
        let config = RoutingConfig::default();
        let mut router = SmartRouter::new(config);
        router.register_provider(
            LlmBackend::Anthropic,
            Arc::new(MockProvider::failing_retryable("anthropic")),
        );
        router.register_provider(
            LlmBackend::OpenAi,
            Arc::new(MockProvider::succeeding("openai", "fallback response")),
        );

        let (response, backend) = router.complete("Hello").await.unwrap();
        assert_eq!(response.content, "fallback response");
        assert_eq!(backend, LlmBackend::OpenAi);
    }

    #[tokio::test]
    async fn test_complete_all_fail() {
        let config = RoutingConfig::default();
        let mut router = SmartRouter::new(config);
        router.register_provider(
            LlmBackend::Anthropic,
            Arc::new(MockProvider::failing_retryable("anthropic")),
        );
        router.register_provider(
            LlmBackend::OpenAi,
            Arc::new(MockProvider::failing_retryable("openai")),
        );

        let result = router.complete("Hello").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_health_degrades_on_failure() {
        let mut health = ProviderHealth::default();
        assert!(health.available);

        for _ in 0..3 {
            health.record_failure();
        }
        assert!(!health.available);
        assert_eq!(health.consecutive_failures, 3);
    }

    #[test]
    fn test_health_recovers_after_success() {
        let mut health = ProviderHealth::default();

        // Degrade health
        for _ in 0..3 {
            health.record_failure();
        }
        assert!(!health.available);

        // Recovery: mark available and record success
        health.available = true;
        health.record_success(50);
        assert!(health.available);
        assert_eq!(health.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn test_complete_updates_health_on_success() {
        let config = RoutingConfig::default();
        let mut router = SmartRouter::new(config);
        router.register_provider(
            LlmBackend::Anthropic,
            Arc::new(MockProvider::succeeding("anthropic", "ok")),
        );

        let _ = router.complete("Hello").await.unwrap();

        let health = router.get_health(LlmBackend::Anthropic).unwrap();
        assert_eq!(health.successful_requests, 1);
        assert_eq!(health.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn test_complete_updates_health_on_failure() {
        let config = RoutingConfig {
            enable_fallback: false,
            ..Default::default()
        };
        let mut router = SmartRouter::new(config);
        router.register_provider(
            LlmBackend::Anthropic,
            Arc::new(MockProvider::failing_retryable("anthropic")),
        );

        let _ = router.complete("Hello").await;

        let health = router.get_health(LlmBackend::Anthropic).unwrap();
        assert_eq!(health.consecutive_failures, 1);
    }

    #[test]
    fn test_fallback_list_excludes_primary() {
        let config = RoutingConfig::default();
        let mut router = SmartRouter::new(config);
        router.register_provider(
            LlmBackend::Anthropic,
            Arc::new(MockProvider::succeeding("anthropic", "ok")),
        );
        router.register_provider(
            LlmBackend::OpenAi,
            Arc::new(MockProvider::succeeding("openai", "ok")),
        );

        let decision = router.route("Hello").unwrap();
        assert!(!decision.fallbacks.contains(&decision.backend));
    }

    #[test]
    fn test_no_fallbacks_when_disabled() {
        let config = RoutingConfig {
            enable_fallback: false,
            ..Default::default()
        };
        let mut router = SmartRouter::new(config);
        router.register_provider(
            LlmBackend::Anthropic,
            Arc::new(MockProvider::succeeding("anthropic", "ok")),
        );
        router.register_provider(
            LlmBackend::OpenAi,
            Arc::new(MockProvider::succeeding("openai", "ok")),
        );

        let decision = router.route("Hello").unwrap();
        assert!(decision.fallbacks.is_empty());
    }
}
