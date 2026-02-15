//! Response quality validation for smart routing.
//!
//! Provides heuristic-based quality scoring for LLM responses to detect
//! low-quality outputs that should be escalated to a more capable provider.

use super::analyzer::ComplexityLevel;

/// Quality score for an LLM response.
#[derive(Debug, Clone)]
pub struct QualityScore {
    /// Overall quality score from 0.0 (terrible) to 1.0 (excellent).
    pub score: f32,
    /// Individual quality signals that contributed to the score.
    pub signals: Vec<QualitySignal>,
    /// Whether the response should be escalated to a better provider.
    pub should_escalate: bool,
}

/// An individual quality signal contributing to the overall score.
#[derive(Debug, Clone)]
pub struct QualitySignal {
    /// Name of the signal check.
    pub name: &'static str,
    /// Score from 0.0 to 1.0 for this signal.
    pub score: f32,
    /// Weight of this signal in the overall score.
    pub weight: f32,
}

/// Heuristic-based response quality checker.
///
/// Evaluates response quality without requiring an additional LLM call.
/// Checks for empty responses, truncation, length appropriateness, and
/// content quality signals.
pub struct ResponseQualityChecker {
    /// Minimum quality score before escalation is recommended.
    min_quality: f32,
}

impl ResponseQualityChecker {
    /// Create a new quality checker with the given minimum quality threshold.
    pub fn new(min_quality: f32) -> Self {
        Self {
            min_quality: min_quality.clamp(0.0, 1.0),
        }
    }

    /// Evaluate the quality of a response given the query complexity.
    pub fn evaluate(
        &self,
        response_content: &str,
        query: &str,
        complexity: ComplexityLevel,
    ) -> QualityScore {
        let mut signals = Vec::new();

        // Signal 1: Non-empty response
        let emptiness_score = self.check_emptiness(response_content);
        signals.push(QualitySignal {
            name: "non_empty",
            score: emptiness_score,
            weight: 0.3,
        });

        // Signal 2: Response length relative to complexity
        let length_score = self.check_length(response_content, complexity);
        signals.push(QualitySignal {
            name: "appropriate_length",
            score: length_score,
            weight: 0.25,
        });

        // Signal 3: Truncation detection
        let truncation_score = self.check_truncation(response_content);
        signals.push(QualitySignal {
            name: "not_truncated",
            score: truncation_score,
            weight: 0.2,
        });

        // Signal 4: Content relevance (basic keyword overlap)
        let relevance_score = self.check_relevance(response_content, query);
        signals.push(QualitySignal {
            name: "relevant_content",
            score: relevance_score,
            weight: 0.15,
        });

        // Signal 5: Error/refusal detection
        let refusal_score = self.check_refusal(response_content);
        signals.push(QualitySignal {
            name: "not_refused",
            score: refusal_score,
            weight: 0.1,
        });

        // Calculate weighted score
        let total_weight: f32 = signals.iter().map(|s| s.weight).sum();
        let weighted_sum: f32 = signals.iter().map(|s| s.score * s.weight).sum();
        let score = if total_weight > 0.0 {
            weighted_sum / total_weight
        } else {
            0.0
        };

        let should_escalate = score < self.min_quality;

        QualityScore {
            score,
            signals,
            should_escalate,
        }
    }

    /// Check if response is non-empty and has meaningful content.
    fn check_emptiness(&self, content: &str) -> f32 {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return 0.0;
        }
        // Very short responses (< 10 chars) get partial score
        if trimmed.len() < 10 {
            return 0.3;
        }
        1.0
    }

    /// Check if response length is appropriate for the complexity level.
    fn check_length(&self, content: &str, complexity: ComplexityLevel) -> f32 {
        let word_count = content.split_whitespace().count();

        let (min_words, ideal_words) = match complexity {
            ComplexityLevel::Simple => (3, 50),
            ComplexityLevel::Medium => (20, 200),
            ComplexityLevel::Complex => (50, 500),
            ComplexityLevel::Expert => (100, 1000),
        };

        if word_count < min_words {
            // Too short for the complexity level
            return (word_count as f32 / min_words as f32).min(1.0) * 0.5;
        }

        if word_count >= ideal_words {
            return 1.0;
        }

        // Linearly interpolate between min and ideal
        0.5 + 0.5 * ((word_count - min_words) as f32 / (ideal_words - min_words) as f32)
    }

    /// Detect if the response appears to have been truncated.
    fn check_truncation(&self, content: &str) -> f32 {
        let trimmed = content.trim();

        // Empty content isn't truncated — it's handled by emptiness check
        if trimmed.is_empty() {
            return 1.0;
        }

        // Check for common truncation indicators
        let truncation_markers = [
            "...", // Trailing ellipsis
        ];

        if truncation_markers
            .iter()
            .any(|m| trimmed.ends_with(m) && trimmed.len() > 50)
        {
            return 0.3;
        }

        // Check for sentence ending (doesn't end mid-word)
        let last_char = trimmed.chars().last().unwrap_or('.');
        if last_char.is_alphanumeric() && trimmed.len() > 100 {
            // Might be cut off mid-sentence
            return 0.6;
        }

        1.0
    }

    /// Basic relevance check via keyword overlap between query and response.
    fn check_relevance(&self, content: &str, query: &str) -> f32 {
        let query_lower = query.to_lowercase();
        let query_words: std::collections::HashSet<&str> = query_lower
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .collect();

        if query_words.is_empty() {
            return 1.0; // Can't check relevance without query words
        }

        let content_lower = content.to_lowercase();
        let matches = query_words
            .iter()
            .filter(|w| content_lower.contains(*w))
            .count();

        let overlap = matches as f32 / query_words.len() as f32;

        // Even 20% keyword overlap suggests relevance
        if overlap >= 0.5 {
            1.0
        } else if overlap >= 0.2 {
            0.7
        } else if overlap > 0.0 {
            0.4
        } else {
            0.2 // Zero overlap doesn't necessarily mean irrelevant
        }
    }

    /// Detect refusal or error responses.
    fn check_refusal(&self, content: &str) -> f32 {
        let lower = content.to_lowercase();

        let refusal_patterns = [
            "i cannot",
            "i can't",
            "i'm unable to",
            "i am unable to",
            "as an ai",
            "i don't have the ability",
            "i'm not able to",
        ];

        let error_patterns = [
            "error occurred",
            "internal server error",
            "something went wrong",
            "an error has occurred",
        ];

        for pattern in &refusal_patterns {
            if lower.contains(pattern) {
                return 0.3;
            }
        }

        for pattern in &error_patterns {
            if lower.contains(pattern) {
                return 0.1;
            }
        }

        1.0
    }
}

impl Default for ResponseQualityChecker {
    fn default() -> Self {
        Self::new(0.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_response_low_quality() {
        let checker = ResponseQualityChecker::new(0.5);
        let score = checker.evaluate("", "What is Rust?", ComplexityLevel::Simple);
        assert!(score.score < 0.5);
        assert!(score.should_escalate);
    }

    #[test]
    fn test_good_response_high_quality() {
        let checker = ResponseQualityChecker::new(0.5);
        let response = "Rust is a systems programming language focused on safety, \
                        speed, and concurrency. It achieves memory safety without \
                        garbage collection through its ownership system.";
        let score = checker.evaluate(response, "What is Rust?", ComplexityLevel::Simple);
        assert!(score.score > 0.5, "Score was {}", score.score);
        assert!(!score.should_escalate);
    }

    #[test]
    fn test_short_response_for_complex_query() {
        let checker = ResponseQualityChecker::new(0.7);
        let score = checker.evaluate(
            "It depends.",
            "Explain the tradeoffs between different database architectures for a distributed system",
            ComplexityLevel::Expert,
        );
        assert!(score.score < 0.7, "Score was {}", score.score);
        assert!(score.should_escalate);
    }

    #[test]
    fn test_refusal_detection() {
        let checker = ResponseQualityChecker::new(0.5);
        let score = checker.evaluate(
            "I cannot help with that request as an AI language model.",
            "Write a function",
            ComplexityLevel::Medium,
        );
        assert!(score.score < 0.7, "Score was {}", score.score);
    }

    #[test]
    fn test_truncated_response() {
        let checker = ResponseQualityChecker::new(0.5);
        let long_text = "a ".repeat(100) + "and then the function should handle...";
        let score = checker.evaluate(&long_text, "Write a function", ComplexityLevel::Medium);
        let truncation_signal = score
            .signals
            .iter()
            .find(|s| s.name == "not_truncated")
            .map(|s| s.score)
            .unwrap_or(1.0);
        assert!(truncation_signal < 1.0);
    }

    #[test]
    fn test_quality_threshold() {
        let strict = ResponseQualityChecker::new(0.9);
        let lenient = ResponseQualityChecker::new(0.1);

        let response = "Yes.";
        let query = "Is Rust good?";

        let strict_score = strict.evaluate(response, query, ComplexityLevel::Simple);
        let lenient_score = lenient.evaluate(response, query, ComplexityLevel::Simple);

        // Same score, different escalation decisions
        assert!((strict_score.score - lenient_score.score).abs() < f32::EPSILON);
        assert!(strict_score.should_escalate);
        assert!(!lenient_score.should_escalate);
    }
}
