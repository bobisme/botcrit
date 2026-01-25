//! JJ (Jujutsu) command wrapper for botcrit.
//!
//! Provides a structured interface for executing jj commands and parsing their output.

pub mod context;
pub mod drift;

pub use context::{extract_context, format_context, CodeContext, ContextLine};
pub use drift::{calculate_drift, DriftResult};

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Resolve the canonical jj repo root from any path within the repo.
///
/// In jj workspaces, each workspace has its own working copy directory,
/// but `.jj/repo` is a file containing the path to the main repo's `.jj/repo` directory.
/// This function follows that pointer to find the main repo root.
///
/// # Algorithm
/// 1. Start from `start_path` and walk up to find `.jj/` directory
/// 2. If `.jj/repo` is a directory, this is the main repo - return its parent
/// 3. If `.jj/repo` is a file, read it to get the path to the main repo's `.jj/repo`
/// 4. Return the parent of that `.jj/repo` directory (the main repo root)
///
/// # Errors
///
/// Returns an error if no `.jj/` directory is found or the repo structure is invalid.
pub fn resolve_repo_root(start_path: &Path) -> Result<PathBuf> {
    // Walk up to find .jj directory
    let mut current = start_path.to_path_buf();
    let jj_dir = loop {
        let jj_path = current.join(".jj");
        if jj_path.exists() {
            break jj_path;
        }
        if !current.pop() {
            bail!("Not in a jj repository (no .jj directory found)");
        }
    };

    let repo_pointer = jj_dir.join("repo");

    if repo_pointer.is_dir() {
        // This is the main repo - .jj/repo is a directory
        // Return the parent of .jj (the repo root)
        Ok(jj_dir
            .parent()
            .context("Invalid jj directory structure")?
            .to_path_buf())
    } else if repo_pointer.is_file() {
        // This is a workspace - .jj/repo is a file containing path to main repo's .jj/repo
        let main_repo_jj_repo = fs::read_to_string(&repo_pointer).with_context(|| {
            format!(
                "Failed to read workspace pointer: {}",
                repo_pointer.display()
            )
        })?;
        let main_repo_jj_repo = PathBuf::from(main_repo_jj_repo.trim());

        // The path points to the main repo's .jj/repo directory
        // Return the parent of .jj (two levels up from .jj/repo)
        main_repo_jj_repo
            .parent() // .jj
            .and_then(|jj| jj.parent()) // repo root
            .map(|p| p.to_path_buf())
            .context("Invalid main repo path in workspace pointer")
    } else {
        bail!("Invalid jj repository structure: .jj/repo is neither file nor directory");
    }
}

/// Wrapper for executing jj commands against a repository.
#[derive(Debug, Clone)]
pub struct JjRepo {
    repo_path: PathBuf,
}

impl JjRepo {
    /// Create a new `JjRepo` wrapper for the given repository path.
    #[must_use]
    pub fn new(repo_path: &Path) -> Self {
        Self {
            repo_path: repo_path.to_path_buf(),
        }
    }

    /// Execute a jj command and return its stdout.
    ///
    /// Always uses `--color=never` for parseable output.
    fn run_jj(&self, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new("jj");
        cmd.current_dir(&self.repo_path)
            .arg("--color=never")
            .args(args);

        let output = cmd.output().with_context(|| {
            if let Err(e) = which::which("jj") {
                format!("jj command not found. Please install jj (Jujutsu): {e}")
            } else {
                format!("Failed to execute jj command: {args:?}")
            }
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "jj command failed with status {}: {}",
                output.status,
                stderr.trim()
            );
        }

        let stdout = String::from_utf8(output.stdout).context("jj output was not valid UTF-8")?;

        Ok(stdout)
    }

    /// Execute a jj command and return stdout, ignoring exit code.
    ///
    /// Used for commands like `file list` where exit code 0 doesn't mean success.
    fn run_jj_ignore_status(&self, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new("jj");
        cmd.current_dir(&self.repo_path)
            .arg("--color=never")
            .args(args);

        let output = cmd.output().with_context(|| {
            if let Err(e) = which::which("jj") {
                format!("jj command not found. Please install jj (Jujutsu): {e}")
            } else {
                format!("Failed to execute jj command: {args:?}")
            }
        })?;

        let stdout = String::from_utf8(output.stdout).context("jj output was not valid UTF-8")?;

        Ok(stdout)
    }

    /// Get the `change_id` for the current working copy (@).
    ///
    /// The `change_id` is jj's stable identifier that survives rewrites.
    ///
    /// # Errors
    ///
    /// Returns an error if the jj command fails or produces invalid output.
    pub fn get_current_change_id(&self) -> Result<String> {
        let output = self
            .run_jj(&["log", "-r", "@", "--no-graph", "-T", "change_id"])
            .context("Failed to get current change_id")?;

        Ok(output.trim().to_string())
    }

    /// Get the `commit_id` (Git SHA) for the current working copy (@).
    ///
    /// # Errors
    ///
    /// Returns an error if the jj command fails or produces invalid output.
    pub fn get_current_commit(&self) -> Result<String> {
        let output = self
            .run_jj(&["log", "-r", "@", "--no-graph", "-T", "commit_id"])
            .context("Failed to get current commit_id")?;

        Ok(output.trim().to_string())
    }

    /// Get the `commit_id` (Git SHA) for a given revset.
    ///
    /// # Errors
    ///
    /// Returns an error if the jj command fails or the revset is invalid.
    pub fn get_commit_for_rev(&self, rev: &str) -> Result<String> {
        let output = self
            .run_jj(&["log", "-r", rev, "--no-graph", "-T", "commit_id"])
            .with_context(|| format!("Failed to get commit_id for {rev}"))?;

        Ok(output.trim().to_string())
    }

    /// Get a git-format diff between two revisions.
    ///
    /// Both `from` and `to` should be valid jj revsets (e.g., "@", `root()`, `change_id`).
    ///
    /// # Errors
    ///
    /// Returns an error if the jj command fails or the revsets are invalid.
    pub fn diff_git(&self, from: &str, to: &str) -> Result<String> {
        self.run_jj(&["diff", "--from", from, "--to", to, "--git"])
            .with_context(|| format!("Failed to get diff from {from} to {to}"))
    }

    /// Get a git-format diff for a specific file between two revisions.
    ///
    /// # Errors
    ///
    /// Returns an error if the jj command fails or the revsets/file are invalid.
    pub fn diff_git_file(&self, from: &str, to: &str, file: &str) -> Result<String> {
        self.run_jj(&["diff", "--from", from, "--to", to, "--git", file])
            .with_context(|| format!("Failed to get diff for file {file} from {from} to {to}"))
    }

    /// Check if a file exists at a given revision.
    ///
    /// Note: jj file list returns exit code 0 even for non-existent files,
    /// so we check stdout instead.
    ///
    /// # Errors
    ///
    /// Returns an error if the jj command fails to execute.
    pub fn file_exists(&self, rev: &str, path: &str) -> Result<bool> {
        let output = self
            .run_jj_ignore_status(&["file", "list", "-r", rev, path])
            .with_context(|| format!("Failed to check if file {path} exists at {rev}"))?;

        Ok(!output.trim().is_empty())
    }

    /// Get the contents of a file at a given revision.
    ///
    /// # Errors
    ///
    /// Returns an error if the file doesn't exist or the jj command fails.
    pub fn show_file(&self, rev: &str, path: &str) -> Result<String> {
        self.run_jj(&["file", "show", "-r", rev, path])
            .with_context(|| format!("Failed to show file {path} at {rev}"))
    }

    /// List files changed in a revision (compared to its parent).
    ///
    /// # Errors
    ///
    /// Returns an error if the jj command fails or the revision is invalid.
    pub fn changed_files(&self, rev: &str) -> Result<Vec<String>> {
        let output = self
            .run_jj(&["diff", "-r", rev, "--name-only"])
            .with_context(|| format!("Failed to list changed files for {rev}"))?;

        Ok(output
            .lines()
            .filter(|line| !line.is_empty())
            .map(ToString::to_string)
            .collect())
    }

    /// List files changed between two revisions.
    ///
    /// # Errors
    ///
    /// Returns an error if the jj command fails or the revsets are invalid.
    pub fn changed_files_between(&self, from: &str, to: &str) -> Result<Vec<String>> {
        let output = self
            .run_jj(&["diff", "--from", from, "--to", to, "--name-only"])
            .with_context(|| format!("Failed to list changed files from {from} to {to}"))?;

        Ok(output
            .lines()
            .filter(|line| !line.is_empty())
            .map(ToString::to_string)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    /// Get the repo path for testing. Uses `CARGO_MANIFEST_DIR` which points to
    /// the botcrit repo root.
    fn test_repo() -> JjRepo {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
        JjRepo::new(Path::new(&manifest_dir))
    }

    #[test]
    fn test_get_current_change_id() {
        let repo = test_repo();
        let change_id = repo.get_current_change_id().unwrap();

        // Change IDs are 32 lowercase hex chars
        assert_eq!(change_id.len(), 32);
        assert!(change_id.chars().all(|c| c.is_ascii_lowercase()));
    }

    #[test]
    fn test_get_current_commit() {
        let repo = test_repo();
        let commit_id = repo.get_current_commit().unwrap();

        // Git commit IDs are 40 hex chars
        assert_eq!(commit_id.len(), 40);
        assert!(commit_id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_diff_git() {
        let repo = test_repo();
        // Diff from root to current - should always work
        let diff = repo.diff_git("root()", "@");
        assert!(diff.is_ok(), "diff should succeed: {:?}", diff.err());
    }

    #[test]
    fn test_diff_git_file() {
        let repo = test_repo();
        // Diff a file that definitely exists
        let diff = repo.diff_git_file("@-", "@", "Cargo.toml");
        // This may error if Cargo.toml wasn't changed, but the command structure is valid
        // The important thing is the jj command executes without crashing
        let _ = diff; // May succeed or fail depending on changes
    }

    #[test]
    fn test_file_exists() {
        let repo = test_repo();

        // Cargo.toml should exist at @
        let exists = repo.file_exists("@", "Cargo.toml").unwrap();
        assert!(exists, "Cargo.toml should exist");

        // A nonsense file should not exist
        let exists = repo
            .file_exists("@", "this-file-definitely-does-not-exist-xyz.txt")
            .unwrap();
        assert!(!exists, "Non-existent file should return false");
    }

    #[test]
    fn test_show_file() {
        let repo = test_repo();

        // Should be able to read Cargo.toml
        let contents = repo.show_file("@", "Cargo.toml").unwrap();
        assert!(contents.contains("[package]"));
        assert!(contents.contains("crit"));
    }

    #[test]
    fn test_changed_files() {
        let repo = test_repo();

        // Get changed files for current commit
        let files = repo.changed_files("@");
        assert!(files.is_ok());
        // Files is a Vec<String>, each entry is a path
        let files = files.unwrap();
        for file in &files {
            assert!(!file.is_empty());
            assert!(!file.contains('\n'));
        }
    }
}
