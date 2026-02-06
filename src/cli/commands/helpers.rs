//! Shared helpers for CLI commands.
//!
//! Provides centralized, version-aware database operations for all commands.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use crate::cli::commands::init::{index_path, is_initialized, CRIT_DIR};
use crate::jj::{resolve_repo_root, JjRepo};
use crate::projection::{sync_from_review_logs, ProjectionDb, ReviewDetail};
use crate::version::{detect_version, require_v2, DataVersion};

/// Ensure crit is initialized in the given directory.
pub fn ensure_initialized(repo_root: &Path) -> Result<()> {
    if !is_initialized(repo_root) {
        bail!("Not a crit repository. Run 'crit --agent <your-name> init' first.");
    }
    Ok(())
}

/// Get the path to the .crit directory.
pub fn crit_dir(repo_root: &Path) -> std::path::PathBuf {
    repo_root.join(CRIT_DIR)
}

/// Open the projection database and sync from event logs (version-aware).
///
/// For v2 repos: Uses `sync_from_review_logs()` for timestamp-based sync
/// from per-review event logs.
///
/// For v1 repos: Fails with migration instructions.
///
/// This is the recommended way to get a synced projection database in commands.
pub fn open_and_sync(repo_root: &Path) -> Result<ProjectionDb> {
    ensure_initialized(repo_root)?;

    // Enforce v2 format
    require_v2(repo_root)?;

    // Open database and initialize schema
    let db = ProjectionDb::open(&index_path(repo_root))?;
    db.init_schema()?;

    // Sync from per-review event logs (v2)
    sync_from_review_logs(&db, repo_root)?;

    Ok(db)
}

/// Open the projection database and sync, allowing v1 format (for read-only operations).
///
/// Use this only for commands that need to read v1 data before migration.
/// Most commands should use `open_and_sync()` which enforces v2.
pub fn open_and_sync_any_version(repo_root: &Path) -> Result<ProjectionDb> {
    ensure_initialized(repo_root)?;

    let db = ProjectionDb::open(&index_path(repo_root))?;
    db.init_schema()?;

    match detect_version(repo_root)? {
        Some(DataVersion::V1) => {
            // v1: Use legacy sync
            use crate::cli::commands::init::events_path;
            use crate::log::open_or_create;
            use crate::projection::sync_from_log_with_backup;

            let log = open_or_create(&events_path(repo_root))?;
            let crit_dir = repo_root.join(CRIT_DIR);
            sync_from_log_with_backup(&db, &log, Some(&crit_dir))?;
        }
        Some(DataVersion::V2) | None => {
            // v2 or new: Use per-review sync
            sync_from_review_logs(&db, repo_root)?;
        }
    }

    Ok(db)
}

/// Result of finding a review across workspaces.
pub struct WorkspaceReview {
    /// The review data
    pub review: ReviewDetail,
    /// The workspace name where the review was found
    pub workspace_name: String,
    /// The workspace path
    pub workspace_path: PathBuf,
}

/// Find a review by ID, searching all workspaces if not found locally.
///
/// This function first checks the local crit_root, then falls back to
/// searching all jj workspaces for the review.
///
/// Returns `Ok(None)` if the review is not found anywhere.
/// Returns `Err` only for I/O or jj errors.
pub fn find_review_in_workspaces(
    crit_root: &Path,
    review_id: &str,
) -> Result<Option<WorkspaceReview>> {
    // First try locally
    if let Ok(db) = open_and_sync(crit_root) {
        if let Ok(Some(review)) = db.get_review(review_id) {
            return Ok(Some(WorkspaceReview {
                review,
                workspace_name: "default".to_string(),
                workspace_path: crit_root.to_path_buf(),
            }));
        }
    }

    // Not found locally - search workspaces
    let repo_root = match resolve_repo_root(crit_root) {
        Ok(root) => root,
        Err(_) => return Ok(None), // Not in a jj repo, can't search workspaces
    };

    let jj = JjRepo::new(&repo_root);
    let workspaces = jj.list_workspaces().context("Failed to list workspaces")?;

    for (ws_name, _ws_change_id, ws_path) in workspaces {
        // Skip current/default workspace (already checked)
        if ws_path == crit_root || ws_path == repo_root {
            continue;
        }

        let ws_crit = ws_path.join(".crit");
        if !ws_crit.exists() {
            continue;
        }

        // Try to find review in this workspace
        if let Ok(db) = open_and_sync(&ws_path) {
            if let Ok(Some(review)) = db.get_review(review_id) {
                return Ok(Some(WorkspaceReview {
                    review,
                    workspace_name: ws_name,
                    workspace_path: ws_path,
                }));
            }
        }
    }

    Ok(None)
}

/// Get a review by ID, with helpful error message if not found.
///
/// If the review exists in a workspace, returns the review and workspace info.
/// Returns `(review, Some((ws_name, ws_path)))` if found in another workspace.
pub fn get_review_or_suggest_path(
    crit_root: &Path,
    review_id: &str,
) -> Result<(ReviewDetail, Option<(String, PathBuf)>)> {
    // First try locally
    let db = open_and_sync(crit_root)?;
    if let Some(review) = db.get_review(review_id)? {
        return Ok((review, None));
    }

    // Search workspaces
    if let Some(ws_review) = find_review_in_workspaces(crit_root, review_id)? {
        // Found in another workspace - return the review and workspace info
        return Ok((
            ws_review.review,
            Some((ws_review.workspace_name, ws_review.workspace_path)),
        ));
    }

    // Not found anywhere
    bail!(
        "Review not found: {}\n  To fix: crit --agent <your-name> reviews list",
        review_id
    )
}

/// Require a review to exist locally (for write operations).
///
/// If the review is found in another workspace, returns a helpful error
/// suggesting the --path flag. This is for operations that need to write
/// to the review's event log.
pub fn require_local_review(crit_root: &Path, review_id: &str) -> Result<ReviewDetail> {
    // Try locally first
    let db = open_and_sync(crit_root)?;
    if let Some(review) = db.get_review(review_id)? {
        return Ok(review);
    }

    // Search workspaces for a helpful error message
    if let Some(ws_review) = find_review_in_workspaces(crit_root, review_id)? {
        bail!(
            "Review {} found in workspace '{}'\n  To fix: crit --path {} <command>",
            review_id,
            ws_review.workspace_name,
            ws_review.workspace_path.display()
        );
    }

    bail!(
        "Review not found: {}\n  To fix: crit --agent <your-name> reviews list",
        review_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_ensure_initialized_fails_on_empty_dir() {
        let dir = tempdir().unwrap();
        let result = ensure_initialized(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("init"));
    }

    #[test]
    fn test_ensure_initialized_v2() {
        let dir = tempdir().unwrap();
        let crit = dir.path().join(".crit");
        fs::create_dir(&crit).unwrap();
        fs::write(crit.join("version"), "2\n").unwrap();
        fs::create_dir(crit.join("reviews")).unwrap();

        assert!(ensure_initialized(dir.path()).is_ok());
    }

    #[test]
    fn test_open_and_sync_rejects_v1() {
        let dir = tempdir().unwrap();
        let crit = dir.path().join(".crit");
        fs::create_dir(&crit).unwrap();
        fs::write(crit.join("events.jsonl"), "some content\n").unwrap();

        let result = open_and_sync(dir.path());
        match result {
            Err(e) => assert!(e.to_string().contains("crit migrate")),
            Ok(_) => panic!("Expected error for v1 repo"),
        }
    }

    #[test]
    fn test_open_and_sync_v2() {
        let dir = tempdir().unwrap();
        let crit = dir.path().join(".crit");
        fs::create_dir(&crit).unwrap();
        fs::write(crit.join("version"), "2\n").unwrap();
        fs::create_dir(crit.join("reviews")).unwrap();

        let result = open_and_sync(dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_open_and_sync_any_version_v1() {
        let dir = tempdir().unwrap();
        let crit = dir.path().join(".crit");
        fs::create_dir(&crit).unwrap();
        fs::write(crit.join("events.jsonl"), "").unwrap();

        // v1 should work with open_and_sync_any_version
        let result = open_and_sync_any_version(dir.path());
        assert!(result.is_ok());
    }
}
