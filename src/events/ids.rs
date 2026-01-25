//! ID generation for reviews, threads, and comments.
//!
//! Uses short, human-readable slugs: cr-xxx, th-xxx, c-xxx

use uuid::Uuid;

/// Prefix for review IDs
const REVIEW_PREFIX: &str = "cr";
/// Prefix for thread IDs
const THREAD_PREFIX: &str = "th";
/// Prefix for comment IDs
const COMMENT_PREFIX: &str = "c";

/// Length of the random suffix (in base36 chars)
const SUFFIX_LEN: usize = 4;

/// Generate a base36 suffix from UUID bytes.
fn base36_suffix(len: usize) -> String {
    let uuid = Uuid::new_v4();
    let bytes = uuid.as_bytes();

    // Use first 8 bytes as a u64 for base36 encoding
    let num = u64::from_le_bytes(bytes[..8].try_into().unwrap());

    // Convert to base36
    let mut result = String::new();
    let mut n = num;
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

    while result.len() < len {
        result.push(CHARS[(n % 36) as usize] as char);
        n /= 36;
    }

    result
}

/// Generate a new review ID (e.g., "cr-1d3f")
pub fn new_review_id() -> String {
    format!("{}-{}", REVIEW_PREFIX, base36_suffix(SUFFIX_LEN))
}

/// Generate a new thread ID (e.g., "th-99az")
pub fn new_thread_id() -> String {
    format!("{}-{}", THREAD_PREFIX, base36_suffix(SUFFIX_LEN))
}

/// Generate a new comment ID (e.g., "c-ab12")
pub fn new_comment_id() -> String {
    format!("{}-{}", COMMENT_PREFIX, base36_suffix(SUFFIX_LEN))
}

/// Check if a string looks like a valid review ID
pub fn is_review_id(s: &str) -> bool {
    s.starts_with("cr-") && s.len() == 3 + SUFFIX_LEN
}

/// Check if a string looks like a valid thread ID
pub fn is_thread_id(s: &str) -> bool {
    s.starts_with("th-") && s.len() == 3 + SUFFIX_LEN
}

/// Check if a string looks like a valid comment ID
pub fn is_comment_id(s: &str) -> bool {
    s.starts_with("c-") && s.len() == 2 + SUFFIX_LEN
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_review_id_format() {
        let id = new_review_id();
        assert!(id.starts_with("cr-"), "ID should start with 'cr-': {}", id);
        assert_eq!(id.len(), 7, "ID should be 7 chars: {}", id);
        assert!(is_review_id(&id));
    }

    #[test]
    fn test_thread_id_format() {
        let id = new_thread_id();
        assert!(id.starts_with("th-"), "ID should start with 'th-': {}", id);
        assert_eq!(id.len(), 7, "ID should be 7 chars: {}", id);
        assert!(is_thread_id(&id));
    }

    #[test]
    fn test_comment_id_format() {
        let id = new_comment_id();
        assert!(id.starts_with("c-"), "ID should start with 'c-': {}", id);
        assert_eq!(id.len(), 6, "ID should be 6 chars: {}", id);
        assert!(is_comment_id(&id));
    }

    #[test]
    fn test_uniqueness() {
        // Smoke test: verify we can generate 100 unique IDs
        // With 4 base36 chars (36^4 = 1.6M possibilities), this should never collide
        let mut ids: HashSet<String> = HashSet::new();
        for _ in 0..100 {
            let id = new_review_id();
            assert!(ids.insert(id.clone()), "Generated duplicate ID: {}", id);
        }
    }

    #[test]
    fn test_validators() {
        assert!(is_review_id("cr-abcd"));
        assert!(!is_review_id("th-abcd"));
        assert!(!is_review_id("cr-abc")); // too short

        assert!(is_thread_id("th-1234"));
        assert!(!is_thread_id("cr-1234"));

        assert!(is_comment_id("c-wxyz"));
        assert!(!is_comment_id("c-abc")); // too short
    }
}
