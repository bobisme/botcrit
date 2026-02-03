//! ID generation for reviews, threads, and comments.
//!
//! Uses short, human-readable slugs: cr-xxx, th-xxx, c-xxx
//! Powered by terseid for adaptive-length, collision-resistant IDs.

use terseid::{IdConfig, IdGenerator, parse_id};

/// Length of the random suffix (in base36 chars)
const HASH_LENGTH: usize = 4;

fn review_generator() -> IdGenerator {
    IdGenerator::new(IdConfig::new("cr"))
}

fn thread_generator() -> IdGenerator {
    IdGenerator::new(IdConfig::new("th"))
}

fn comment_generator() -> IdGenerator {
    IdGenerator::new(IdConfig::new("c"))
}

/// Generate random bytes for seeding ID generation.
fn random_seed() -> [u8; 16] {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).expect("failed to generate random bytes");
    buf
}

/// Generate a new review ID (e.g., "cr-1d3f")
pub fn new_review_id() -> String {
    review_generator().candidate(&random_seed(), HASH_LENGTH)
}

/// Generate a new thread ID (e.g., "th-99az")
pub fn new_thread_id() -> String {
    thread_generator().candidate(&random_seed(), HASH_LENGTH)
}

/// Generate a new comment ID (e.g., "c-ab12")
pub fn new_comment_id() -> String {
    comment_generator().candidate(&random_seed(), HASH_LENGTH)
}

/// Check if a string looks like a valid review ID
pub fn is_review_id(s: &str) -> bool {
    parse_id(s)
        .map(|parsed| parsed.prefix == "cr" && parsed.hash.len() >= 3)
        .unwrap_or(false)
}

/// Check if a string looks like a valid thread ID
pub fn is_thread_id(s: &str) -> bool {
    parse_id(s)
        .map(|parsed| parsed.prefix == "th" && parsed.hash.len() >= 3)
        .unwrap_or(false)
}

/// Check if a string looks like a valid comment ID
pub fn is_comment_id(s: &str) -> bool {
    parse_id(s)
        .map(|parsed| parsed.prefix == "c" && parsed.hash.len() >= 3)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_review_id_format() {
        let id = new_review_id();
        assert!(id.starts_with("cr-"), "ID should start with 'cr-': {}", id);
        assert!(id.len() >= 6, "ID should be at least 6 chars: {}", id);
        assert!(is_review_id(&id));
    }

    #[test]
    fn test_thread_id_format() {
        let id = new_thread_id();
        assert!(id.starts_with("th-"), "ID should start with 'th-': {}", id);
        assert!(id.len() >= 6, "ID should be at least 6 chars: {}", id);
        assert!(is_thread_id(&id));
    }

    #[test]
    fn test_comment_id_format() {
        let id = new_comment_id();
        assert!(id.starts_with("c-"), "ID should start with 'c-': {}", id);
        assert!(id.len() >= 5, "ID should be at least 5 chars: {}", id);
        assert!(is_comment_id(&id));
    }

    #[test]
    fn test_uniqueness() {
        // Smoke test: verify we can generate 100 unique IDs
        let mut ids: HashSet<String> = HashSet::new();
        for _ in 0..100 {
            let id = new_review_id();
            assert!(ids.insert(id.clone()), "Generated duplicate ID: {}", id);
        }
    }

    #[test]
    fn test_validators() {
        // Valid IDs with new format (flexible length)
        // Note: 4+ char hashes must contain at least one digit (terseid rule)
        assert!(is_review_id("cr-a1cd"));
        assert!(is_review_id("cr-abc"));
        assert!(is_review_id("cr-a1cdefgh")); // longer IDs are valid with digit
        assert!(!is_review_id("th-a1cd"));
        assert!(!is_review_id("cr-ab")); // too short (min 3 chars)

        assert!(is_thread_id("th-1234"));
        assert!(is_thread_id("th-abc"));
        assert!(!is_thread_id("cr-1234"));

        assert!(is_comment_id("c-wx1z"));
        assert!(is_comment_id("c-abc"));
        assert!(!is_comment_id("c-ab")); // too short (min 3 chars)
    }
}
