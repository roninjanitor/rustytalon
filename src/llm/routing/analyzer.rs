//! Query complexity analyzer for smart routing.
//!
//! Analyzes queries to determine their complexity level, which informs
//! provider selection. Simple queries can use cheaper/faster models while
//! complex queries benefit from more capable models.

use std::collections::HashSet;

/// Complexity level for routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ComplexityLevel {
    /// Simple queries: greetings, single-fact questions, basic formatting.
    Simple,
    /// Medium queries: multi-step reasoning, code snippets, summarization.
    Medium,
    /// Complex queries: long code generation, multi-file analysis, deep reasoning.
    Complex,
    /// Expert queries: architecture design, complex debugging, research tasks.
    Expert,
}

impl ComplexityLevel {
    /// Get the minimum recommended model tier for this complexity level.
    pub fn min_model_tier(&self) -> &'static str {
        match self {
            Self::Simple => "haiku",   // Fast, cheap models
            Self::Medium => "sonnet",  // Balanced models
            Self::Complex => "sonnet", // Capable models
            Self::Expert => "opus",    // Most capable models
        }
    }

    /// Weight for cost vs quality tradeoff (0.0 = cost only, 1.0 = quality only).
    pub fn quality_weight(&self) -> f32 {
        match self {
            Self::Simple => 0.2,
            Self::Medium => 0.5,
            Self::Complex => 0.7,
            Self::Expert => 0.9,
        }
    }
}

/// Detailed complexity score with component breakdowns.
#[derive(Debug, Clone)]
pub struct ComplexityScore {
    /// Overall complexity level.
    pub level: ComplexityLevel,
    /// Raw numeric score (0.0 - 1.0).
    pub raw_score: f32,
    /// Estimated token count for the query.
    pub estimated_tokens: u32,
    /// Whether code was detected in the query.
    pub has_code: bool,
    /// Whether multi-step reasoning is likely needed.
    pub multi_step: bool,
    /// Detected domain (if any).
    pub domain: Option<String>,
    /// Confidence in the complexity assessment (0.0 - 1.0).
    pub confidence: f32,
}

impl ComplexityScore {
    /// Create a simple score.
    pub fn simple() -> Self {
        Self {
            level: ComplexityLevel::Simple,
            raw_score: 0.1,
            estimated_tokens: 50,
            has_code: false,
            multi_step: false,
            domain: None,
            confidence: 0.8,
        }
    }
}

/// Query analyzer that scores complexity using heuristics.
#[derive(Debug, Clone)]
pub struct QueryAnalyzer {
    /// Keywords that suggest simple queries.
    simple_keywords: HashSet<&'static str>,
    /// Keywords that suggest complex queries.
    complex_keywords: HashSet<&'static str>,
    /// Keywords that suggest expert-level queries.
    expert_keywords: HashSet<&'static str>,
    /// Code-related patterns.
    code_indicators: Vec<&'static str>,
}

impl Default for QueryAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryAnalyzer {
    /// Create a new query analyzer with default heuristics.
    pub fn new() -> Self {
        Self {
            simple_keywords: [
                "hi",
                "hello",
                "hey",
                "thanks",
                "thank you",
                "bye",
                "goodbye",
                "what is",
                "what's",
                "who is",
                "when was",
                "where is",
                "yes",
                "no",
                "ok",
                "okay",
                "sure",
                "help",
            ]
            .into_iter()
            .collect(),

            complex_keywords: [
                "implement",
                "create",
                "build",
                "design",
                "architect",
                "refactor",
                "optimize",
                "debug",
                "analyze",
                "compare",
                "explain how",
                "step by step",
                "in detail",
                "comprehensive",
                "multiple",
                "several",
                "all the",
                "entire",
            ]
            .into_iter()
            .collect(),

            expert_keywords: [
                "architecture",
                "distributed",
                "scalability",
                "security audit",
                "performance analysis",
                "system design",
                "trade-offs",
                "microservices",
                "kubernetes",
                "concurrent",
                "async",
                "research",
                "state of the art",
                "best practices",
            ]
            .into_iter()
            .collect(),

            code_indicators: vec![
                "```",
                "fn ",
                "def ",
                "function ",
                "class ",
                "struct ",
                "impl ",
                "async ",
                "await ",
                "import ",
                "from ",
                "use ",
                "pub ",
                "private ",
                "public ",
                "static ",
                "const ",
                "let ",
                "var ",
                "val ",
                "if ",
                "else ",
                "for ",
                "while ",
                "return ",
                "yield ",
                "match ",
                "switch ",
                "case ",
            ],
        }
    }

    /// Analyze a query and return its complexity score.
    pub fn analyze(&self, query: &str) -> ComplexityScore {
        let lower_query = query.to_lowercase();
        let word_count = query.split_whitespace().count();
        let char_count = query.len();

        // Estimate tokens (rough: ~4 chars per token)
        let estimated_tokens = (char_count / 4).max(word_count) as u32;

        // Check for code
        let has_code = self.detect_code(query);

        // Check for multi-step reasoning indicators
        let multi_step = self.detect_multi_step(&lower_query);

        // Detect domain
        let domain = self.detect_domain(&lower_query);

        // Calculate raw score based on multiple factors
        let mut raw_score = 0.0f32;

        // Length factor (longer queries tend to be more complex)
        raw_score += match word_count {
            0..=10 => 0.1,
            11..=50 => 0.2,
            51..=200 => 0.3,
            _ => 0.4,
        };

        // Code factor
        if has_code {
            raw_score += 0.2;
        }

        // Multi-step factor
        if multi_step {
            raw_score += 0.15;
        }

        // Keyword factors
        let simple_matches = self.count_keyword_matches(&lower_query, &self.simple_keywords);
        let complex_matches = self.count_keyword_matches(&lower_query, &self.complex_keywords);
        let expert_matches = self.count_keyword_matches(&lower_query, &self.expert_keywords);

        raw_score -= simple_matches as f32 * 0.05;
        raw_score += complex_matches as f32 * 0.1;
        raw_score += expert_matches as f32 * 0.15;

        // Clamp to [0, 1]
        raw_score = raw_score.clamp(0.0, 1.0);

        // Determine level from raw score
        let level = match raw_score {
            s if s < 0.25 => ComplexityLevel::Simple,
            s if s < 0.5 => ComplexityLevel::Medium,
            s if s < 0.75 => ComplexityLevel::Complex,
            _ => ComplexityLevel::Expert,
        };

        // Calculate confidence based on signal strength
        let total_signals = simple_matches + complex_matches + expert_matches;
        let confidence = if total_signals == 0 {
            0.5 // No clear signals
        } else if total_signals <= 2 {
            0.7
        } else {
            0.9
        };

        ComplexityScore {
            level,
            raw_score,
            estimated_tokens,
            has_code,
            multi_step,
            domain,
            confidence,
        }
    }

    /// Detect if the query contains code.
    fn detect_code(&self, query: &str) -> bool {
        // Check for code blocks
        if query.contains("```") {
            return true;
        }

        // Check for code indicators (need multiple to be confident)
        let indicator_count = self
            .code_indicators
            .iter()
            .filter(|&ind| query.contains(ind))
            .count();

        indicator_count >= 2
    }

    /// Detect if multi-step reasoning is needed.
    fn detect_multi_step(&self, query: &str) -> bool {
        let indicators = [
            "step by step",
            "first,",
            "then,",
            "after that",
            "finally,",
            "multiple",
            "several",
            "list all",
            "explain each",
            "compare and",
            "analyze",
            "and also",
            "as well as",
        ];

        indicators.iter().any(|&ind| query.contains(ind))
    }

    /// Detect the domain of the query.
    fn detect_domain(&self, query: &str) -> Option<String> {
        let domains = [
            (vec!["rust", "cargo", "crate", "borrow checker"], "rust"),
            (
                vec!["python", "pip", "django", "flask", "pytorch"],
                "python",
            ),
            (
                vec!["javascript", "typescript", "npm", "node", "react", "vue"],
                "javascript",
            ),
            (vec!["go ", "golang", "goroutine"], "go"),
            (vec!["java ", "jvm", "spring", "maven", "gradle"], "java"),
            (
                vec!["sql", "database", "postgres", "mysql", "query"],
                "database",
            ),
            (vec!["docker", "kubernetes", "k8s", "container"], "devops"),
            (vec!["aws", "gcp", "azure", "cloud"], "cloud"),
            (
                vec!["ml", "machine learning", "neural", "model training"],
                "ml",
            ),
            (vec!["api", "rest", "graphql", "endpoint"], "api"),
        ];

        for (keywords, domain) in domains {
            if keywords.iter().any(|&kw| query.contains(kw)) {
                return Some(domain.to_string());
            }
        }

        None
    }

    /// Count keyword matches in the query.
    fn count_keyword_matches(&self, query: &str, keywords: &HashSet<&str>) -> usize {
        keywords.iter().filter(|&&kw| query.contains(kw)).count()
    }

    /// Quick check if a query is trivially simple.
    pub fn is_trivial(&self, query: &str) -> bool {
        let lower = query.to_lowercase().trim().to_string();
        let word_count = lower.split_whitespace().count();

        // Very short queries that are just greetings or confirmations
        if word_count <= 3 {
            let trivial = [
                "hi",
                "hello",
                "hey",
                "thanks",
                "thank you",
                "yes",
                "no",
                "ok",
                "okay",
            ];
            return trivial.iter().any(|&t| lower.starts_with(t));
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_query() {
        let analyzer = QueryAnalyzer::new();
        let score = analyzer.analyze("What is Rust?");
        assert_eq!(score.level, ComplexityLevel::Simple);
    }

    #[test]
    fn test_code_detection() {
        let analyzer = QueryAnalyzer::new();
        let score = analyzer.analyze("```rust\nfn main() {}\n```");
        assert!(score.has_code);
    }

    #[test]
    fn test_complex_query() {
        let analyzer = QueryAnalyzer::new();
        let score = analyzer.analyze(
            "Implement a distributed cache system with consistent hashing, \
             replication, and automatic failover. Analyze the trade-offs \
             between availability and consistency.",
        );
        assert!(score.level >= ComplexityLevel::Complex);
    }

    #[test]
    fn test_trivial_detection() {
        let analyzer = QueryAnalyzer::new();
        assert!(analyzer.is_trivial("Hello"));
        assert!(analyzer.is_trivial("Thanks!"));
        assert!(!analyzer.is_trivial("How do I implement a binary tree?"));
    }

    #[test]
    fn test_domain_detection() {
        let analyzer = QueryAnalyzer::new();
        let score = analyzer.analyze("How do I use cargo to build a Rust crate?");
        assert_eq!(score.domain, Some("rust".to_string()));
    }
}
