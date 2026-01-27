//! Agent identity resolution.
//!
//! Determines the author field for events based on environment variables
//! or explicit override.

use anyhow::{bail, Result};
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
///
/// Returns error if no identity is set - agents must identify themselves.
/// Use `--user` flag to explicitly use $USER for human usage.
pub fn get_agent_identity(explicit: Option<&str>) -> Result<String> {
    if let Some(name) = explicit {
        return Ok(name.to_string());
    }

    if let Ok(name) = env::var(CRIT_AGENT_VAR) {
        if !name.is_empty() {
            return Ok(name);
        }
    }

    if let Ok(name) = env::var(BOTBUS_AGENT_VAR) {
        if !name.is_empty() {
            return Ok(name);
        }
    }

    bail!(
        "Agent identity required. Set CRIT_AGENT or BOTBUS_AGENT, or use --user for human identity."
    )
}

/// Get the system user identity (for --user flag).
pub fn get_user_identity() -> Result<String> {
    env::var(USER_VAR)
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("$USER not set"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_explicit_override() {
        let identity = get_agent_identity(Some("explicit_agent")).unwrap();
        assert_eq!(identity, "explicit_agent");
    }

    #[test]
    fn test_user_identity() {
        // USER should be set on most systems
        let identity = get_user_identity();
        // May or may not be set depending on environment
        if let Ok(id) = identity {
            assert!(!id.is_empty());
        }
    }
}
