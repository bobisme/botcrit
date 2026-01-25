//! Implementation of `crit init` command.

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// The directory name for crit data
pub const CRIT_DIR: &str = ".crit";

/// The event log filename
pub const EVENTS_FILE: &str = "events.jsonl";

/// The gitignore filename
pub const GITIGNORE_FILE: &str = ".gitignore";

/// Files to ignore (local caches, not to be tracked)
const GITIGNORE_CONTENT: &str = "# Local caches (do not track)
index.db
index.db-journal
";

/// Run the init command.
///
/// Creates the .crit directory with an empty events.jsonl file and .gitignore.
pub fn run_init(repo_root: &Path) -> Result<()> {
    let crit_dir = repo_root.join(CRIT_DIR);

    // Check if already initialized
    if crit_dir.exists() {
        let events_file = crit_dir.join(EVENTS_FILE);
        if events_file.exists() {
            // Ensure gitignore exists even for existing repos
            ensure_gitignore(&crit_dir)?;
            println!("Already initialized: {}", crit_dir.display());
            return Ok(());
        }
    }

    // Create directory
    fs::create_dir_all(&crit_dir)
        .with_context(|| format!("Failed to create directory: {}", crit_dir.display()))?;

    // Create empty events file
    let events_file = crit_dir.join(EVENTS_FILE);
    fs::write(&events_file, "")
        .with_context(|| format!("Failed to create events file: {}", events_file.display()))?;

    // Create gitignore
    ensure_gitignore(&crit_dir)?;

    println!("Initialized crit in {}", crit_dir.display());
    println!("  Created: {}", events_file.display());

    Ok(())
}

/// Ensure .gitignore exists with required entries.
fn ensure_gitignore(crit_dir: &Path) -> Result<()> {
    let gitignore_path = crit_dir.join(GITIGNORE_FILE);

    if gitignore_path.exists() {
        // Check if index.db is already ignored
        let content = fs::read_to_string(&gitignore_path)
            .with_context(|| format!("Failed to read {}", gitignore_path.display()))?;

        if !content.contains("index.db") {
            // Append to existing gitignore
            let updated = format!("{}\n{}", content.trim_end(), GITIGNORE_CONTENT);
            fs::write(&gitignore_path, updated)
                .with_context(|| format!("Failed to update {}", gitignore_path.display()))?;
        }
    } else {
        // Create new gitignore
        fs::write(&gitignore_path, GITIGNORE_CONTENT)
            .with_context(|| format!("Failed to create {}", gitignore_path.display()))?;
    }

    Ok(())
}

/// Check if crit is initialized in the given directory.
pub fn is_initialized(repo_root: &Path) -> bool {
    let crit_dir = repo_root.join(CRIT_DIR);
    let events_file = crit_dir.join(EVENTS_FILE);
    crit_dir.exists() && events_file.exists()
}

/// Get the path to the events file.
pub fn events_path(repo_root: &Path) -> std::path::PathBuf {
    repo_root.join(CRIT_DIR).join(EVENTS_FILE)
}

/// Get the path to the index database.
pub fn index_path(repo_root: &Path) -> std::path::PathBuf {
    repo_root.join(CRIT_DIR).join("index.db")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_init_creates_directory() {
        let temp = TempDir::new().unwrap();
        let repo_root = temp.path();

        run_init(repo_root).unwrap();

        assert!(repo_root.join(CRIT_DIR).exists());
        assert!(repo_root.join(CRIT_DIR).join(EVENTS_FILE).exists());
        assert!(repo_root.join(CRIT_DIR).join(GITIGNORE_FILE).exists());
        assert!(is_initialized(repo_root));

        // Check gitignore content
        let gitignore =
            std::fs::read_to_string(repo_root.join(CRIT_DIR).join(GITIGNORE_FILE)).unwrap();
        assert!(gitignore.contains("index.db"));
    }

    #[test]
    fn test_init_idempotent() {
        let temp = TempDir::new().unwrap();
        let repo_root = temp.path();

        // First init
        run_init(repo_root).unwrap();

        // Second init should succeed
        run_init(repo_root).unwrap();

        assert!(is_initialized(repo_root));
    }

    #[test]
    fn test_is_initialized_false_when_missing() {
        let temp = TempDir::new().unwrap();
        assert!(!is_initialized(temp.path()));
    }

    #[test]
    fn test_paths() {
        let repo_root = Path::new("/tmp/test-repo");
        assert_eq!(
            events_path(repo_root),
            Path::new("/tmp/test-repo/.crit/events.jsonl")
        );
        assert_eq!(
            index_path(repo_root),
            Path::new("/tmp/test-repo/.crit/index.db")
        );
    }
}
