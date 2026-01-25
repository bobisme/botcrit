//! Agent identity resolution.
//!
//! Determines the author field for events based on environment variables
//! or explicit override.

use std::env;

/// Environment variable for crit-specific agent identity
const CRIT_AGENT_VAR: &str = "CRIT_AGENT";

/// Environment variable for BotBus agent identity (interop)
const BOTBUS_AGENT_VAR: &str = "BOTBUS_AGENT";

/// Fallback to system user
const USER_VAR: &str = "USER";

/// Get the current agent identity.
///
/// Resolution order:
/// 1. Explicit override (if provided)
/// 2. CRIT_AGENT environment variable
/// 3. BOTBUS_AGENT environment variable
/// 4. USER environment variable
/// 5. "unknown" as final fallback
pub fn get_agent_identity(explicit: Option<&str>) -> String {
    if let Some(name) = explicit {
        return name.to_string();
    }

    if let Ok(name) = env::var(CRIT_AGENT_VAR) {
        if !name.is_empty() {
            return name;
        }
    }

    if let Ok(name) = env::var(BOTBUS_AGENT_VAR) {
        if !name.is_empty() {
            return name;
        }
    }

    if let Ok(name) = env::var(USER_VAR) {
        if !name.is_empty() {
            return name;
        }
    }

    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_explicit_override() {
        let identity = get_agent_identity(Some("explicit_agent"));
        assert_eq!(identity, "explicit_agent");
    }

    #[test]
    fn test_fallback_to_user() {
        // USER should be set on most systems
        // We can't safely clear env vars in tests, so just check explicit override works
        let identity = get_agent_identity(None);
        // Just verify we get something, not empty
        assert!(!identity.is_empty());
    }
}
