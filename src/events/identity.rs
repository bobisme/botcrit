//! Agent identity resolution.
//!
//! Determines the author field for events based on environment variables
//! or explicit override.

use anyhow::{bail, Result};
use std::env;

/// Environment variables checked for agent identity, in priority order.
const IDENTITY_VARS: &[&str] = &[
    "BOTCRIT_AGENT",
    "CRIT_AGENT",
    "AGENT",
    "BOTBUS_AGENT",
];

/// Fallback to system user
const USER_VAR: &str = "USER";

/// Get the current agent identity.
///
/// Resolution order:
/// 1. Explicit override (`--agent`)
/// 2. BOTCRIT_AGENT environment variable
/// 3. CRIT_AGENT environment variable
/// 4. AGENT environment variable
/// 5. BOTBUS_AGENT environment variable
///
/// Returns error if no identity is set - agents must identify themselves.
/// Use `--user` flag to explicitly use $USER for human usage.
pub fn get_agent_identity(explicit: Option<&str>) -> Result<String> {
    if let Some(name) = explicit {
        return Ok(name.to_string());
    }

    for var in IDENTITY_VARS {
        if let Ok(name) = env::var(var) {
            if !name.is_empty() {
                return Ok(name);
            }
        }
    }

    bail!(
        "Agent identity required. Use --agent <name>, set BOTCRIT_AGENT/CRIT_AGENT/AGENT/BOTBUS_AGENT, or use --user for human identity."
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
