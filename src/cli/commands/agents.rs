//! Implementation of `crit agents` commands.
//!
//! Manages agent instructions in AGENTS.md for crit usage.

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// The AGENTS.md filename
const AGENTS_FILE: &str = "AGENTS.md";

/// Start marker for crit instructions block
const START_MARKER: &str = "<!-- crit-agent-instructions -->";

/// End marker for crit instructions block
const END_MARKER: &str = "<!-- end-crit-agent-instructions -->";

/// Returns the crit agent instructions text.
pub fn get_crit_instructions() -> &'static str {
    r#"## Crit: Agent-Centric Code Review

This project uses [crit](https://github.com/anomalyco/botcrit) for distributed code reviews optimized for AI agents.

### Quick Start

```bash
# Initialize crit in the repository (once)
crit init

# Create a review for current change
crit reviews create --title "Add feature X"

# List open reviews
crit reviews list

# Check reviews needing your attention
crit reviews list --needs-review --author $BOTBUS_AGENT

# Show review details
crit reviews show <review_id>
```

### Adding Comments (Recommended)

The simplest way to comment on code - auto-creates threads:

```bash
# Add a comment on a specific line (creates thread automatically)
crit comment <review_id> --file src/main.rs --line 42 "Consider using Option here"

# Add another comment on same line (reuses existing thread)
crit comment <review_id> --file src/main.rs --line 42 "Good point, will fix"

# Comment on a line range
crit comment <review_id> --file src/main.rs --line 10-20 "This block needs refactoring"
```

### Managing Threads

```bash
# List threads on a review
crit threads list <review_id>

# Show thread with context
crit threads show <thread_id>

# Resolve a thread
crit threads resolve <thread_id> --reason "Fixed in latest commit"
```

### Voting on Reviews

```bash
# Approve a review (LGTM)
crit lgtm <review_id> -m "Looks good!"

# Block a review (request changes)
crit block <review_id> -r "Need more test coverage"
```

### Viewing Full Reviews

```bash
# Show full review with all threads and comments
crit review <review_id>

# Show with more context lines
crit review <review_id> --context 5

# List threads with first comment preview
crit threads list <review_id> -v
```

### Approving and Merging

```bash
# Approve a review (changes status to approved)
crit reviews approve <review_id>

# Mark as merged (after jj squash/merge)
# Note: Will fail if there are blocking votes
crit reviews merge <review_id>

# Self-approve and merge in one step (solo workflows)
crit reviews merge <review_id> --self-approve
```

### Agent Best Practices

1. **Set your identity** via environment:
   ```bash
   export BOTBUS_AGENT=my-agent-name
   ```

2. **Check for pending reviews** at session start:
   ```bash
   crit reviews list --needs-review --author $BOTBUS_AGENT
   ```

3. **Check status** to see unresolved threads:
   ```bash
   crit status <review_id> --unresolved-only
   ```

4. **Run doctor** to verify setup:
   ```bash
   crit doctor
   ```

### Output Formats

- Default output is TOON (token-optimized, human-readable)
- Use `--json` flag for machine-parseable JSON output

### Key Concepts

- **Reviews** are anchored to jj Change IDs (survive rebases)
- **Threads** group comments on specific file locations
- **crit comment** is the simple way to leave feedback (auto-creates threads)
- Works across jj workspaces (shared .crit/ in main repo)"#
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
        assert!(content.contains("crit reviews create"));
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
