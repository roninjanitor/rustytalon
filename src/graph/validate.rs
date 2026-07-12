//! Pure validation helpers for Cypher query construction.
//!
//! Neo4j node labels and relationship types cannot be bound as query
//! parameters -- only property values can. Any label/type that gets
//! interpolated directly into a Cypher query string must be validated
//! against a strict allowlist pattern first, or a caller could inject
//! arbitrary Cypher via a crafted `type` value (e.g. `"Person) DETACH
//! DELETE n //"`).

use std::sync::LazyLock;

use regex::Regex;

use crate::graph::error::GraphError;

/// Matches a safe Neo4j label/relationship-type identifier: starts with a
/// letter, followed by up to 63 letters/digits/underscores.
static TYPE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9_]{0,63}$").expect("valid regex"));

/// Validate an entity label (e.g. "Person", "Project") for safe interpolation
/// into a Cypher query. The schema is intentionally open-ended (not a fixed
/// enum) so it can evolve, but the shape must still be safe to interpolate.
pub fn validate_label(label: &str) -> Result<(), GraphError> {
    if TYPE_PATTERN.is_match(label) {
        Ok(())
    } else {
        Err(GraphError::InvalidType(label.to_string()))
    }
}

/// Validate a relationship type (e.g. "LEADS", "MEMBER_OF") for safe
/// interpolation into a Cypher query. Same rules as entity labels.
pub fn validate_rel_type(rel_type: &str) -> Result<(), GraphError> {
    if TYPE_PATTERN.is_match(rel_type) {
        Ok(())
    } else {
        Err(GraphError::InvalidType(rel_type.to_string()))
    }
}

/// Clamp a requested traversal hop count to a safe range (1-3) so
/// `get_entity_context` can't be used to trigger an unbounded/slow traversal.
pub fn clamp_hops(hops: u32) -> u32 {
    hops.clamp(1, 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_labels() {
        assert!(validate_label("Person").is_ok());
        assert!(validate_label("Project").is_ok());
        assert!(validate_label("my_custom_type").is_ok());
        assert!(validate_label("A").is_ok());
    }

    #[test]
    fn rejects_injection_attempts() {
        assert!(validate_label("Person) DETACH DELETE n //").is_err());
        assert!(validate_label("Person`) MATCH (m) DETACH DELETE m //").is_err());
        assert!(validate_label("").is_err());
        assert!(validate_label("1Person").is_err());
        assert!(validate_label("Person Name").is_err());
        assert!(validate_label("Person;DROP").is_err());
    }

    #[test]
    fn rejects_overlong_labels() {
        let too_long = "A".repeat(65);
        assert!(validate_label(&too_long).is_err());
        let max_len = "A".repeat(64);
        assert!(validate_label(&max_len).is_ok());
    }

    #[test]
    fn rel_type_uses_same_rules() {
        assert!(validate_rel_type("LEADS").is_ok());
        assert!(validate_rel_type("MEMBER_OF").is_ok());
        assert!(validate_rel_type("bad type").is_err());
    }

    #[test]
    fn hops_are_clamped_to_1_3() {
        assert_eq!(clamp_hops(0), 1);
        assert_eq!(clamp_hops(1), 1);
        assert_eq!(clamp_hops(2), 2);
        assert_eq!(clamp_hops(3), 3);
        assert_eq!(clamp_hops(10), 3);
        assert_eq!(clamp_hops(u32::MAX), 3);
    }
}
