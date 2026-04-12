//! Routing strategies for provider selection.
//!
//! Strategies determine how queries are routed based on:
//! - Cost optimization
//! - Quality requirements
//! - Provider availability
//! - User preferences

use super::analyzer::ComplexityLevel;
use crate::config::LlmBackend;

/// Routing strategy that determines provider selection behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoutingStrategy {
    /// Minimize cost while meeting minimum quality requirements.
    CostOptimized,
    /// Maximize response quality regardless of cost.
    QualityFirst,
    /// Balance cost and quality based on query complexity.
    #[default]
    Balanced,
    /// Always use a specific provider.
    Fixed(LlmBackend),
    /// Use local models (Ollama) when possible, fall back to cloud.
    LocalFirst,
}

impl RoutingStrategy {
    /// Parse a strategy from a string.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "cost" | "cost_optimized" | "cheap" => Some(Self::CostOptimized),
            "quality" | "quality_first" | "best" => Some(Self::QualityFirst),
            "balanced" | "auto" | "smart" => Some(Self::Balanced),
            "local" | "local_first" | "ollama" => Some(Self::LocalFirst),
            "anthropic" | "claude" => Some(Self::Fixed(LlmBackend::Anthropic)),
            "openai" | "gpt" => Some(Self::Fixed(LlmBackend::OpenAi)),
            _ => None,
        }
    }

    /// Get the quality weight for this strategy (0.0 = cost only, 1.0 = quality only).
    pub fn quality_weight(&self, complexity: ComplexityLevel) -> f32 {
        match self {
            Self::CostOptimized => 0.2,
            Self::QualityFirst => 1.0,
            Self::Balanced => complexity.quality_weight(),
            Self::Fixed(_) => 0.5, // Not used for routing
            Self::LocalFirst => 0.3,
        }
    }

    /// Whether this strategy allows fallback to other providers.
    pub fn allows_fallback(&self) -> bool {
        !matches!(self, Self::Fixed(_))
    }
}

/// Configuration for the smart router.
#[derive(Debug, Clone)]
pub struct RoutingConfig {
    /// The routing strategy to use.
    pub strategy: RoutingStrategy,
    /// Maximum cost per request in USD (None = unlimited).
    pub max_cost_per_request: Option<f64>,
    /// Minimum acceptable quality score (0.0 - 1.0).
    pub min_quality_score: f32,
    /// Whether to enable automatic retries with fallback providers.
    pub enable_fallback: bool,
    /// Maximum retry attempts before failing.
    pub max_retries: u32,
    /// Timeout for provider health checks in seconds.
    pub health_check_timeout_secs: u64,
    /// How often to refresh provider health status in seconds.
    pub health_refresh_interval_secs: u64,
    /// Preferred providers in order (used as tiebreaker).
    pub preferred_providers: Vec<LlmBackend>,
    /// Providers to exclude from routing.
    pub excluded_providers: Vec<LlmBackend>,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            strategy: RoutingStrategy::Balanced,
            max_cost_per_request: None,
            min_quality_score: 0.5,
            enable_fallback: true,
            max_retries: 3,
            health_check_timeout_secs: 5,
            health_refresh_interval_secs: 60,
            preferred_providers: vec![LlmBackend::Anthropic, LlmBackend::OpenAi],
            excluded_providers: Vec::new(),
        }
    }
}

impl RoutingConfig {
    /// Create a cost-optimized config.
    pub fn cost_optimized() -> Self {
        Self {
            strategy: RoutingStrategy::CostOptimized,
            max_cost_per_request: Some(0.01), // $0.01 per request
            min_quality_score: 0.3,
            ..Default::default()
        }
    }

    /// Create a quality-first config.
    pub fn quality_first() -> Self {
        Self {
            strategy: RoutingStrategy::QualityFirst,
            max_cost_per_request: None,
            min_quality_score: 0.8,
            ..Default::default()
        }
    }

    /// Create a local-first config (prefer Ollama).
    pub fn local_first() -> Self {
        Self {
            strategy: RoutingStrategy::LocalFirst,
            preferred_providers: vec![LlmBackend::Ollama, LlmBackend::Anthropic],
            ..Default::default()
        }
    }

    /// Load config from environment variables.
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(strategy) = std::env::var("ROUTING_STRATEGY")
            && let Some(s) = RoutingStrategy::parse(&strategy) {
                config.strategy = s;
            }

        if let Ok(max_cost) = std::env::var("ROUTING_MAX_COST") {
            config.max_cost_per_request = max_cost.parse().ok();
        }

        if let Ok(min_quality) = std::env::var("ROUTING_MIN_QUALITY")
            && let Ok(q) = min_quality.parse::<f32>() {
                config.min_quality_score = q.clamp(0.0, 1.0);
            }

        if let Ok(fallback) = std::env::var("ROUTING_ENABLE_FALLBACK") {
            config.enable_fallback = fallback.to_lowercase() == "true" || fallback == "1";
        }

        if let Ok(retries) = std::env::var("ROUTING_MAX_RETRIES")
            && let Ok(r) = retries.parse::<u32>() {
                config.max_retries = r;
            }

        config
    }
}

/// Model tier classification for routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelTier {
    /// Fast, cheap models (Haiku, GPT-4o-mini, small Ollama models).
    Economy,
    /// Balanced models (Sonnet, GPT-4o).
    Standard,
    /// Premium models (Opus, o1).
    Premium,
}

impl ModelTier {
    /// Classify a model by its ID.
    pub fn from_model_id(model_id: &str) -> Self {
        let lower = model_id.to_lowercase();

        // Premium tier
        if lower.contains("opus")
            || lower.contains("o1")
            || lower.contains("o3")
            || lower.starts_with("claude-3-opus")
        {
            return Self::Premium;
        }

        // Economy tier
        if lower.contains("haiku")
            || lower.contains("mini")
            || lower.contains("3.5-turbo")
            || lower.contains("llama")
            || lower.contains("mistral")
            || lower.contains("phi")
        {
            return Self::Economy;
        }

        // Default to standard
        Self::Standard
    }

    /// Get the approximate quality score for this tier.
    pub fn quality_score(&self) -> f32 {
        match self {
            Self::Economy => 0.6,
            Self::Standard => 0.8,
            Self::Premium => 0.95,
        }
    }

    /// Get the approximate relative cost multiplier.
    pub fn cost_multiplier(&self) -> f32 {
        match self {
            Self::Economy => 1.0,
            Self::Standard => 5.0,
            Self::Premium => 20.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strategy_parsing() {
        assert_eq!(
            RoutingStrategy::parse("cost"),
            Some(RoutingStrategy::CostOptimized)
        );
        assert_eq!(
            RoutingStrategy::parse("quality"),
            Some(RoutingStrategy::QualityFirst)
        );
        assert_eq!(
            RoutingStrategy::parse("balanced"),
            Some(RoutingStrategy::Balanced)
        );
        assert_eq!(
            RoutingStrategy::parse("anthropic"),
            Some(RoutingStrategy::Fixed(LlmBackend::Anthropic))
        );
    }

    #[test]
    fn test_model_tier_classification() {
        assert_eq!(
            ModelTier::from_model_id("claude-3-5-haiku-20241022"),
            ModelTier::Economy
        );
        assert_eq!(
            ModelTier::from_model_id("claude-sonnet-4-20250514"),
            ModelTier::Standard
        );
        assert_eq!(
            ModelTier::from_model_id("claude-opus-4-20250514"),
            ModelTier::Premium
        );
        assert_eq!(ModelTier::from_model_id("gpt-4o-mini"), ModelTier::Economy);
        assert_eq!(ModelTier::from_model_id("gpt-4o"), ModelTier::Standard);
        assert_eq!(ModelTier::from_model_id("o1"), ModelTier::Premium);
    }

    #[test]
    fn test_quality_weight_by_complexity() {
        let strategy = RoutingStrategy::Balanced;
        assert!(strategy.quality_weight(ComplexityLevel::Simple) < 0.5);
        assert!(strategy.quality_weight(ComplexityLevel::Expert) > 0.8);
    }
}
