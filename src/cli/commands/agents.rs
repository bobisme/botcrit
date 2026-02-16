//! Implementation of `crit agents` commands.
//!
//! Manages agent instructions in AGENTS.md for crit usage.

use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::Path;

/// The AGENTS.md filename
const AGENTS_FILE: &str = "AGENTS.md";

/// Start marker for crit instructions block
const START_MARKER: &str = "<!-- crit-agent-instructions -->";

/// End marker for crit instructions block
const END_MARKER: &str = "<!-- end-crit-agent-instructions -->";

/// Returns the crit agent instructions text.
pub fn get_crit_instructions() -> String {
    let suggested_name = suggest_agent_name();

    format!(
        r#"## Crit: Agent-Centric Code Review

This project uses [crit](https://github.com/anomalyco/botcrit) for distributed code reviews optimized for AI agents.

### Identity

Pass `--agent <name>` on every crit command to identify yourself:

```bash
crit --agent {name} reviews list
crit --agent {name} comment <id> --file F --line L "msg"
```

Alternatively, set `BOTCRIT_AGENT`, `CRIT_AGENT`, `AGENT`, or `BOTBUS_AGENT` env vars (but note these may not persist across tool invocations in some environments).

In interactive (TTY) sessions, `$USER` is used as a fallback if no agent identity is set.

### Essential Commands

All commands require `--agent <name>` (env vars don't persist in sandboxed environments):

```bash
crit --agent {name} reviews list                        # List reviews
crit --agent {name} reviews create --title "..."        # Create review for current change
crit --agent {name} review <id>                         # Show full review with threads/comments
crit --agent {name} comment <id> --file F --line L "M"  # Add comment (auto-creates thread)
crit --agent {name} reply <thread_id> "M"               # Reply to an existing thread
crit --agent {name} lgtm <id> -m "..."                  # Approve (LGTM)
crit --agent {name} block <id> -r "..."                 # Request changes
crit --agent {name} threads resolve <id> --reason "..." # Resolve a thread
crit --agent {name} reviews mark-merged <id> --self-approve   # Approve + mark merged (solo workflow)
```

### Workflow

1. **Review code**: `crit --agent {name} review <id>` or `crit --agent {name} threads list <id> -v`
2. **Add feedback**: `crit --agent {name} comment <id> --file <path> --line <n> "comment"`
3. **Reply**: `crit --agent {name} reply <thread_id> "response"` to respond to existing threads
4. **Vote**: `crit --agent {name} lgtm <id>` or `crit --agent {name} block <id> -r "reason"`
5. **Resolve threads**: `crit --agent {name} threads resolve <id>` after addressing feedback
6. **Mark merged**: `crit --agent {name} reviews mark-merged <id>` (fails if blocking votes exist)

### Key Points

- Reviews anchor to jj Change IDs (survive rebases)
- `crit comment` creates new feedback on a file+line (auto-creates threads)
- `crit reply` responds to an existing thread
- Use `--json` for machine-parseable output
- **Identity**: Use `--agent <name>` flag (preferred) or set BOTCRIT_AGENT/CRIT_AGENT/AGENT/BOTBUS_AGENT env var
- In TTY sessions, `$USER` is used as fallback if no agent identity is set"#,
        name = suggested_name,
    )
}

/// Suggest an agent name based on the project directory.
///
/// Priority: CRIT_AGENT > BOTBUS_AGENT > <dirname>-dev
fn suggest_agent_name() -> String {
    // Check env vars first - don't override an existing identity
    if let Ok(name) = env::var("CRIT_AGENT") {
        if !name.is_empty() {
            return name;
        }
    }
    if let Ok(name) = env::var("BOTBUS_AGENT") {
        if !name.is_empty() {
            return name;
        }
    }

    // Fall back to project-based name
    env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .map(|dir| format!("{}-dev", dir))
        .unwrap_or_else(|| "my-agent".to_string())
}

/// Run the `crit agents init` command.
///
/// Inserts crit usage instructions into AGENTS.md, creating the file if needed.
/// Uses HTML comment markers for idempotent updates.
pub fn run_agents_init(repo_root: &Path) -> Result<()> {
    let agents_path = repo_root.join(AGENTS_FILE);

    // Read existing content or start with empty
    let content = if agents_path.exists() {
        fs::read_to_string(&agents_path)
            .with_context(|| format!("Failed to read {}", agents_path.display()))?
    } else {
        String::new()
    };

    // Build the instruction block
    let instruction_block = format!(
        "{}\n\n{}\n\n{}",
        START_MARKER,
        get_crit_instructions(),
        END_MARKER
    );

    // Check if markers already exist
    let has_start = content.contains(START_MARKER);
    let has_end = content.contains(END_MARKER);

    let updated_content = if has_start && has_end {
        // Replace existing block
        let start_idx = content.find(START_MARKER).unwrap();
        let end_idx = content.find(END_MARKER).unwrap() + END_MARKER.len();

        let mut result = String::with_capacity(content.len());
        result.push_str(&content[..start_idx]);
        result.push_str(&instruction_block);
        result.push_str(&content[end_idx..]);
        result
    } else if has_start || has_end {
        // Malformed - one marker without the other
        anyhow::bail!(
            "AGENTS.md has mismatched crit markers. Please remove partial markers and retry."
        );
    } else {
        // No existing block - append to end
        if content.is_empty() {
            instruction_block
        } else {
            format!("{}\n\n{}", content.trim_end(), instruction_block)
        }
    };

    // Write the updated content
    fs::write(&agents_path, &updated_content)
        .with_context(|| format!("Failed to write {}", agents_path.display()))?;

    if agents_path.exists() && (has_start && has_end) {
        println!("Updated crit instructions in {}", agents_path.display());
    } else {
        println!("Added crit instructions to {}", agents_path.display());
    }

    Ok(())
}

/// Run the `crit agents show` command.
///
/// Prints the crit instructions block to stdout.
/// The output includes a suggested agent name based on the project directory.
pub fn run_agents_show() -> Result<()> {
    println!("{}", get_crit_instructions());
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_get_crit_instructions_not_empty() {
        let instructions = get_crit_instructions();
        assert!(!instructions.is_empty());
        assert!(instructions.contains("crit"));
        assert!(instructions.contains("reviews"));
        assert!(instructions.contains("threads"));
        assert!(instructions.contains("--agent"));
        assert!(instructions.contains("reply"));
    }

    #[test]
    fn test_agents_init_creates_file() {
        let temp = TempDir::new().unwrap();
        let repo_root = temp.path();

        run_agents_init(repo_root).unwrap();

        let agents_path = repo_root.join(AGENTS_FILE);
        assert!(agents_path.exists());

        let content = fs::read_to_string(&agents_path).unwrap();
        assert!(content.contains(START_MARKER));
        assert!(content.contains(END_MARKER));
        assert!(content.contains("crit"));
    }

    #[test]
    fn test_agents_init_appends_to_existing() {
        let temp = TempDir::new().unwrap();
        let repo_root = temp.path();
        let agents_path = repo_root.join(AGENTS_FILE);

        // Create existing AGENTS.md with some content
        let existing = "# Agent Instructions\n\nSome existing content here.\n";
        fs::write(&agents_path, existing).unwrap();

        run_agents_init(repo_root).unwrap();

        let content = fs::read_to_string(&agents_path).unwrap();
        assert!(content.contains("Some existing content here"));
        assert!(content.contains(START_MARKER));
        assert!(content.contains(END_MARKER));
    }

    #[test]
    fn test_agents_init_updates_existing_block() {
        let temp = TempDir::new().unwrap();
        let repo_root = temp.path();
        let agents_path = repo_root.join(AGENTS_FILE);

        // Create file with existing crit block
        let existing = format!(
            "# Header\n\n{}\n\nOld instructions\n\n{}\n\n# Footer\n",
            START_MARKER, END_MARKER
        );
        fs::write(&agents_path, &existing).unwrap();

        run_agents_init(repo_root).unwrap();

        let content = fs::read_to_string(&agents_path).unwrap();
        assert!(content.contains("# Header"));
        assert!(content.contains("# Footer"));
        assert!(content.contains(START_MARKER));
        assert!(content.contains(END_MARKER));
        // Should have new instructions, not old
        assert!(!content.contains("Old instructions"));
        assert!(content.contains("crit --agent"));
    }

    #[test]
    fn test_agents_init_idempotent() {
        let temp = TempDir::new().unwrap();
        let repo_root = temp.path();

        run_agents_init(repo_root).unwrap();
        let first_content = fs::read_to_string(repo_root.join(AGENTS_FILE)).unwrap();

        run_agents_init(repo_root).unwrap();
        let second_content = fs::read_to_string(repo_root.join(AGENTS_FILE)).unwrap();

        assert_eq!(first_content, second_content);
    }

    #[test]
    fn test_agents_init_fails_on_mismatched_markers() {
        let temp = TempDir::new().unwrap();
        let repo_root = temp.path();
        let agents_path = repo_root.join(AGENTS_FILE);

        // Only start marker, no end marker
        fs::write(
            &agents_path,
            format!("# Header\n\n{}\n\nBroken", START_MARKER),
        )
        .unwrap();

        let result = run_agents_init(repo_root);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("mismatched"));
    }

    #[test]
    fn test_agents_show() {
        // Just verify it doesn't panic and returns Ok
        run_agents_show().unwrap();
    }
}
