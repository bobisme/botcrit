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

### Essential Commands

```bash
crit reviews list                        # List reviews
crit reviews create --title "..."        # Create review for current change
crit review <id>                         # Show full review with threads/comments
crit comment <id> --file F --line L "M"  # Add comment (auto-creates thread)
crit lgtm <id> -m "..."                  # Approve (LGTM)
crit block <id> -r "..."                 # Request changes
crit threads resolve <id> --reason "..." # Resolve a thread
crit reviews merge <id> --self-approve   # Approve + merge (solo workflow)
```

### Workflow

1. **Review code**: `crit review <id>` or `crit threads list <id> -v`
2. **Add feedback**: `crit comment <id> --file <path> --line <n> "comment"`
3. **Vote**: `crit lgtm <id>` or `crit block <id> -r "reason"`
4. **Resolve threads**: `crit threads resolve <id>` after addressing feedback
5. **Merge**: `crit reviews merge <id>` (fails if blocking votes exist)

### Key Points

- Reviews anchor to jj Change IDs (survive rebases)
- `crit comment` auto-creates threads - preferred over `threads create` + `comments add`
- Use `--json` for machine-parseable output
- Set identity: `export BOTBUS_AGENT=my-agent-name`"#
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
