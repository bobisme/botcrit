//! Implementation of `crit status` and `crit diff` commands.

use anyhow::{bail, Result};
use serde::Serialize;
use std::path::Path;

use crate::cli::commands::init::{events_path, index_path, is_initialized};
use crate::jj::drift::{calculate_drift, DriftResult};
use crate::jj::JjRepo;
use crate::log::open_or_create;
use crate::output::{Formatter, OutputFormat};
use crate::projection::{sync_from_log, ProjectionDb, ThreadSummary};

/// Thread status with drift information.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadStatusEntry {
    pub thread_id: String,
    pub file_path: String,
    pub original_line: i64,
    pub current_line: Option<i64>,
    pub drift_status: String,
    pub status: String,
    pub comment_count: i64,
}

/// Review status with threads and drift information.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewStatus {
    pub review_id: String,
    pub title: String,
    pub status: String,
    pub total_threads: usize,
    pub open_threads: usize,
    pub threads_with_drift: usize,
    pub threads: Vec<ThreadStatusEntry>,
}

/// Show status of reviews with drift detection.
///
/// # Arguments
/// * `crit_root` - Path to main repo (where .crit/ lives)
/// * `workspace_root` - Path to current workspace (for jj @ resolution)
pub fn run_status(
    crit_root: &Path,
    workspace_root: &Path,
    review_id: Option<&str>,
    unresolved_only: bool,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(crit_root)?;

    let db = open_and_sync(crit_root)?;
    let jj = JjRepo::new(workspace_root);
    let current_commit = jj.get_current_commit()?;

    // Get reviews to process
    let reviews = if let Some(rid) = review_id {
        let review = db.get_review(rid)?;
        match review {
            Some(r) => vec![r],
            None => bail!("Review not found: {}", rid),
        }
    } else {
        // Get all open reviews
        db.list_reviews(Some("open"), None)?
            .into_iter()
            .filter_map(|rs| db.get_review(&rs.review_id).ok().flatten())
            .collect()
    };

    let mut statuses = Vec::new();

    for review in reviews {
        // Get threads for this review
        let status_filter = if unresolved_only { Some("open") } else { None };
        let threads = db.list_threads(&review.review_id, status_filter, None)?;

        let mut thread_entries = Vec::new();
        let mut drift_count = 0;

        for thread in &threads {
            // Calculate drift for this thread
            let thread_detail = db.get_thread(&thread.thread_id)?;
            let drift_result = if let Some(td) = &thread_detail {
                calculate_drift(
                    &jj,
                    &td.file_path,
                    td.selection_start as u32,
                    &td.commit_hash,
                    &current_commit,
                )
                .unwrap_or(DriftResult::Unchanged {
                    current_line: td.selection_start as u32,
                })
            } else {
                DriftResult::Unchanged {
                    current_line: thread.selection_start as u32,
                }
            };

            let (current_line, drift_status) = match &drift_result {
                DriftResult::Unchanged { current_line } => {
                    (Some(*current_line as i64), "unchanged".to_string())
                }
                DriftResult::Shifted {
                    current_line,
                    original_line,
                } => {
                    drift_count += 1;
                    let delta = *current_line as i64 - *original_line as i64;
                    let direction = if delta > 0 { "+" } else { "" };
                    (
                        Some(*current_line as i64),
                        format!("shifted({}{delta})", direction),
                    )
                }
                DriftResult::Modified => {
                    drift_count += 1;
                    (None, "modified".to_string())
                }
                DriftResult::Deleted => {
                    drift_count += 1;
                    (None, "deleted".to_string())
                }
            };

            thread_entries.push(ThreadStatusEntry {
                thread_id: thread.thread_id.clone(),
                file_path: thread.file_path.clone(),
                original_line: thread.selection_start,
                current_line,
                drift_status,
                status: thread.status.clone(),
                comment_count: thread.comment_count,
            });
        }

        let open_count = threads.iter().filter(|t| t.status == "open").count();

        statuses.push(ReviewStatus {
            review_id: review.review_id.clone(),
            title: review.title.clone(),
            status: review.status.clone(),
            total_threads: threads.len(),
            open_threads: open_count,
            threads_with_drift: drift_count,
            threads: thread_entries,
        });
    }

    // Build context-aware empty message
    let empty_msg = if review_id.is_some() {
        "Review has no threads"
    } else if unresolved_only {
        "No open reviews with unresolved threads"
    } else {
        "No open reviews"
    };

    let formatter = Formatter::new(format);
    formatter.print_list(&statuses, empty_msg)?;

    Ok(())
}

/// Show diff for a review.
///
/// # Arguments
/// * `crit_root` - Path to main repo (where .crit/ lives)
/// * `workspace_root` - Path to current workspace (for jj @ resolution)
pub fn run_diff(
    crit_root: &Path,
    workspace_root: &Path,
    review_id: &str,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(crit_root)?;

    let db = open_and_sync(crit_root)?;
    let jj = JjRepo::new(workspace_root);

    // Get the review
    let review = db.get_review(review_id)?;
    let review = match review {
        Some(r) => r,
        None => bail!("Review not found: {}", review_id),
    };

    // Get the base commit: parent of initial_commit
    // This shows ALL files changed in the review, not just changes since creation
    let base_commit = jj
        .get_parent_commit(&review.initial_commit)
        .unwrap_or_else(|_| review.initial_commit.clone());

    // Get current commit (target for the diff)
    let current_commit = jj.get_current_commit()?;

    // Get the diff between base and current
    let diff = jj.diff_git(&base_commit, &current_commit)?;

    // Get changed files from the diff
    let changed_files = extract_changed_files_from_diff(&diff);

    // Get threads for context
    let threads = db.list_threads(review_id, None, None)?;

    // Build structured output
    let result = serde_json::json!({
        "review_id": review_id,
        "base_commit": base_commit,
        "initial_commit": review.initial_commit,
        "current_commit": current_commit,
        "changed_files": changed_files,
        "thread_count": threads.len(),
        "threads_by_file": group_threads_by_file(&threads),
        "diff": diff,
    });

    let formatter = Formatter::new(format);
    formatter.print(&result)?;

    Ok(())
}

/// Extract file names from a git diff output.
fn extract_changed_files_from_diff(diff: &str) -> Vec<String> {
    diff.lines()
        .filter(|line| line.starts_with("diff --git"))
        .filter_map(|line| {
            // Format: diff --git a/path b/path
            let parts: Vec<&str> = line.split_whitespace().collect();
            parts.get(3).map(|s| s.trim_start_matches("b/").to_string())
        })
        .collect()
}

/// Group threads by file path.
fn group_threads_by_file(threads: &[ThreadSummary]) -> serde_json::Value {
    let mut by_file: std::collections::HashMap<String, Vec<&ThreadSummary>> =
        std::collections::HashMap::new();

    for thread in threads {
        by_file
            .entry(thread.file_path.clone())
            .or_default()
            .push(thread);
    }

    serde_json::json!(by_file
        .into_iter()
        .map(|(file, threads)| {
            serde_json::json!({
                "file": file,
                "threads": threads.iter().map(|t| serde_json::json!({
                    "thread_id": t.thread_id,
                    "line": t.selection_start,
                    "status": t.status,
                })).collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>())
}

// ============================================================================
// Helpers
// ============================================================================

fn ensure_initialized(repo_root: &Path) -> Result<()> {
    if !is_initialized(repo_root) {
        bail!("Not a crit repository. Run 'crit init' first.");
    }
    Ok(())
}

fn open_and_sync(repo_root: &Path) -> Result<ProjectionDb> {
    let db = ProjectionDb::open(&index_path(repo_root))?;
    db.init_schema()?;
    let log = open_or_create(&events_path(repo_root))?;
    sync_from_log(&db, &log)?;
    Ok(db)
}
