//! ID generation for reviews and threads.
//!
//! Uses short, human-readable slugs: cr-xxx, th-xxx
//! Comments use thread child IDs: th-xxx.1, th-xxx.2, etc.
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

/// Generate a comment ID as a child of a thread (e.g., "th-abc.1")
pub fn make_comment_id(thread_id: &str, comment_number: u32) -> String {
    format!("{}.{}", thread_id, comment_number)
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

/// Check if a string looks like a valid comment ID (th-xxx.N format)
pub fn is_comment_id(s: &str) -> bool {
    // Split on '.' to separate thread ID from comment number
    let parts: Vec<&str> = s.splitn(2, '.').collect();
    if parts.len() != 2 {
        return false;
    }
    // First part must be a valid thread ID
    if !is_thread_id(parts[0]) {
        return false;
    }
    // Second part must be a positive integer
    parts[1].parse::<u32>().map(|n| n > 0).unwrap_or(false)
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
        // Comment IDs are now thread child IDs: th-xxx.N
        let thread_id = new_thread_id();
        let comment_id = make_comment_id(&thread_id, 1);
        assert!(
            comment_id.ends_with(".1"),
            "Comment ID should end with '.1': {}",
            comment_id
        );
        assert!(is_comment_id(&comment_id));

        // Multiple comments
        let comment_id_2 = make_comment_id(&thread_id, 42);
        assert!(comment_id_2.ends_with(".42"));
        assert!(is_comment_id(&comment_id_2));
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

        // Comment IDs are now thread child IDs
        assert!(is_comment_id("th-abc.1"));
        assert!(is_comment_id("th-1234.42"));
        assert!(is_comment_id("th-abc1ef.999")); // long hash needs digit (terseid rule)
        assert!(!is_comment_id("th-abc")); // missing comment number
        assert!(!is_comment_id("th-abc.")); // empty comment number
        assert!(!is_comment_id("th-abc.0")); // zero not allowed
        assert!(!is_comment_id("th-abc.-1")); // negative not allowed
        assert!(!is_comment_id("c-abc")); // old format not valid
        assert!(!is_comment_id("cr-abc.1")); // review ID, not thread
    }
}
