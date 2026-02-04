//! Implementation of `crit init` command.

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use crate::version::{detect_version, write_version_file, DataVersion};

/// The directory name for crit data
pub const CRIT_DIR: &str = ".crit";

/// The event log filename (v1 legacy)
pub const EVENTS_FILE: &str = "events.jsonl";

/// The reviews directory (v2)
pub const REVIEWS_DIR: &str = "reviews";

/// The gitignore filename
pub const GITIGNORE_FILE: &str = ".gitignore";

/// Files to ignore (local caches, not to be tracked)
const GITIGNORE_CONTENT: &str = "# Local caches (do not track)
index.db
index.db-journal
";

/// Run the init command.
///
/// Creates the .crit directory with v2 structure:
/// - .crit/version (contains "2")
/// - .crit/reviews/ (empty directory for per-review event logs)
/// - .crit/.gitignore (ignores index.db)
pub fn run_init(repo_root: &Path) -> Result<()> {
    let crit_dir = repo_root.join(CRIT_DIR);

    // Check if already initialized
    if crit_dir.exists() {
        // Check version
        match detect_version(repo_root)? {
            Some(DataVersion::V1) => {
                ensure_gitignore(&crit_dir)?;
                println!("Already initialized (v1 format): {}", crit_dir.display());
                println!();
                println!("To upgrade to v2 format, run:");
                println!("  crit migrate");
                return Ok(());
            }
            Some(DataVersion::V2) => {
                ensure_gitignore(&crit_dir)?;
                println!("Already initialized (v2 format): {}", crit_dir.display());
                return Ok(());
            }
            None => {
                // Directory exists but no version - check for events.jsonl
                let events_file = crit_dir.join(EVENTS_FILE);
                if events_file.exists() {
                    ensure_gitignore(&crit_dir)?;
                    println!("Already initialized (v1 format): {}", crit_dir.display());
                    println!();
                    println!("To upgrade to v2 format, run:");
                    println!("  crit migrate");
                    return Ok(());
                }
                // Empty .crit/ - continue to initialize as v2
            }
        }
    }

    // Create directory structure for v2
    fs::create_dir_all(&crit_dir)
        .with_context(|| format!("Failed to create directory: {}", crit_dir.display()))?;

    // Create reviews directory
    let reviews_dir = crit_dir.join(REVIEWS_DIR);
    fs::create_dir_all(&reviews_dir)
        .with_context(|| format!("Failed to create reviews directory: {}", reviews_dir.display()))?;

    // Write version file
    write_version_file(repo_root, DataVersion::V2)?;

    // Create gitignore
    ensure_gitignore(&crit_dir)?;

    println!("✓ Crit initialized (v2) in {}", crit_dir.display());
    println!();
    println!("Next steps:");
    println!("  1. Create a review:");
    println!("     crit --agent <your-name> reviews create --title \"Your change description\"");
    println!();
    println!("  2. Or check agent setup:");
    println!("     crit --agent <your-name> agents show");

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
///
/// Returns true if either v1 (events.jsonl) or v2 (version file or reviews dir) exists.
pub fn is_initialized(repo_root: &Path) -> bool {
    let crit_dir = repo_root.join(CRIT_DIR);
    if !crit_dir.exists() {
        return false;
    }

    // v1: events.jsonl exists
    let events_file = crit_dir.join(EVENTS_FILE);
    if events_file.exists() {
        return true;
    }

    // v2: version file or reviews directory exists
    let version_file = crit_dir.join("version");
    let reviews_dir = crit_dir.join(REVIEWS_DIR);
    version_file.exists() || reviews_dir.exists()
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
    fn test_init_creates_v2_structure() {
        let temp = TempDir::new().unwrap();
        let repo_root = temp.path();

        run_init(repo_root).unwrap();

        // Check v2 structure
        assert!(repo_root.join(CRIT_DIR).exists());
        assert!(repo_root.join(CRIT_DIR).join("version").exists());
        assert!(repo_root.join(CRIT_DIR).join(REVIEWS_DIR).exists());
        assert!(repo_root.join(CRIT_DIR).join(GITIGNORE_FILE).exists());
        assert!(is_initialized(repo_root));

        // No v1 events.jsonl
        assert!(!repo_root.join(CRIT_DIR).join(EVENTS_FILE).exists());

        // Check version file content
        let version =
            std::fs::read_to_string(repo_root.join(CRIT_DIR).join("version")).unwrap();
        assert_eq!(version.trim(), "2");

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
    fn test_is_initialized_v1() {
        let temp = TempDir::new().unwrap();
        let repo_root = temp.path();
        let crit_dir = repo_root.join(CRIT_DIR);

        // Create v1 structure
        fs::create_dir_all(&crit_dir).unwrap();
        fs::write(crit_dir.join(EVENTS_FILE), "").unwrap();

        assert!(is_initialized(repo_root));
    }

    #[test]
    fn test_is_initialized_v2() {
        let temp = TempDir::new().unwrap();
        let repo_root = temp.path();
        let crit_dir = repo_root.join(CRIT_DIR);

        // Create v2 structure
        fs::create_dir_all(crit_dir.join(REVIEWS_DIR)).unwrap();
        fs::write(crit_dir.join("version"), "2\n").unwrap();

        assert!(is_initialized(repo_root));
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
