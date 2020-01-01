//! Shared helpers for CLI commands.
//!
//! Provides centralized, version-aware database operations for all commands.

use anyhow::{bail, Result};
use std::path::Path;

use crate::cli::commands::init::{index_path, is_initialized, CRIT_DIR};
use crit_core::projection::{sync_from_review_logs, ProjectionDb, ReviewDetail, ThreadDetail};
use crit_core::scm::ScmRepo;
use crit_core::version::{detect_version, require_v2, DataVersion};

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
            use crit_core::log::open_or_create;
            use crit_core::projection::sync_from_log_with_backup;

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

/// Get a review by ID, returning an error if not found.
pub fn get_review(crit_root: &Path, review_id: &str) -> Result<ReviewDetail> {
    let db = open_and_sync(crit_root)?;
    db.get_review(review_id)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Review not found: {}\n  To fix: crit --agent <your-name> reviews list",
            review_id
        )
    })
}

/// Require a review to exist (for operations that need the review).
///
/// Returns the review if found, or an error with helpful message if not.
pub fn require_local_review(crit_root: &Path, review_id: &str) -> Result<ReviewDetail> {
    get_review(crit_root, review_id)
}

/// Get a thread by ID, returning an error if not found.
pub fn get_thread(crit_root: &Path, thread_id: &str) -> Result<ThreadDetail> {
    let db = open_and_sync(crit_root)?;
    db.get_thread(thread_id)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Thread not found: {}\n  To fix: crit --agent <your-name> threads list <review_id>",
            thread_id
        )
    })
}

/// Resolve the best commit hash to anchor new review thread creation.
///
/// Priority order:
/// 1. `final_commit` if present
/// 2. Resolved commit for `scm_anchor`
/// 3. Resolved commit for legacy `jj_change_id`
/// 4. `initial_commit`
pub fn resolve_review_thread_commit(scm: &dyn ScmRepo, review: &ReviewDetail) -> String {
    review
        .final_commit
        .clone()
        .or_else(|| scm.commit_for_anchor(&review.scm_anchor).ok())
        .or_else(|| scm.commit_for_anchor(&review.jj_change_id).ok())
        .unwrap_or_else(|| review.initial_commit.clone())
}

/// Create a "review not found" error.
pub fn review_not_found_error(_crit_root: &Path, review_id: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "Review not found: {}\n  To fix: crit --agent <your-name> reviews list",
        review_id
    )
}

/// Create a "thread not found" error.
pub fn thread_not_found_error(_crit_root: &Path, thread_id: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "Thread not found: {}\n  To fix: crit --agent <your-name> threads list <review_id>",
        thread_id
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
