//! Implementation of `crit reviews` subcommands.

use anyhow::{bail, Context, Result};
use std::path::Path;

use crate::cli::commands::init::{events_path, index_path, is_initialized};
use crate::events::{
    get_agent_identity, new_review_id, Event, EventEnvelope, ReviewAbandoned, ReviewApproved,
    ReviewCreated, ReviewersRequested,
};
use crate::jj::JjRepo;
use crate::log::{open_or_create, AppendLog};
use crate::output::{Formatter, OutputFormat};
use crate::projection::{sync_from_log, ProjectionDb};

/// Create a new review for the current jj change.
pub fn run_reviews_create(
    repo_root: &Path,
    title: String,
    description: Option<String>,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(repo_root)?;

    let jj = JjRepo::new(repo_root);
    let change_id = jj
        .get_current_change_id()
        .context("Failed to get current change ID")?;
    let commit_id = jj
        .get_current_commit()
        .context("Failed to get current commit")?;

    let review_id = new_review_id();
    let author = get_agent_identity(author);

    let event = EventEnvelope::new(
        &author,
        Event::ReviewCreated(ReviewCreated {
            review_id: review_id.clone(),
            jj_change_id: change_id.clone(),
            initial_commit: commit_id.clone(),
            title: title.clone(),
            description: description.clone(),
        }),
    );

    let log = open_or_create(&events_path(repo_root))?;
    log.append(&event)?;

    // Output the result
    let result = serde_json::json!({
        "review_id": review_id,
        "jj_change_id": change_id,
        "initial_commit": commit_id,
        "title": title,
        "author": author,
    });

    let formatter = Formatter::new(format);
    formatter.print(&result)?;

    Ok(())
}

/// List reviews with optional filters.
pub fn run_reviews_list(
    repo_root: &Path,
    status: Option<&str>,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(repo_root)?;

    let db = open_and_sync(repo_root)?;
    let reviews = db.list_reviews(status, author)?;

    let formatter = Formatter::new(format);
    formatter.print(&reviews)?;

    Ok(())
}

/// Show details for a specific review.
pub fn run_reviews_show(repo_root: &Path, review_id: &str, format: OutputFormat) -> Result<()> {
    ensure_initialized(repo_root)?;

    let db = open_and_sync(repo_root)?;
    let review = db.get_review(review_id)?;

    match review {
        Some(r) => {
            let formatter = Formatter::new(format);
            formatter.print(&r)?;
        }
        None => {
            bail!("Review not found: {}", review_id);
        }
    }

    Ok(())
}

/// Request reviewers for a review.
pub fn run_reviews_request(
    repo_root: &Path,
    review_id: &str,
    reviewers: &str,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(repo_root)?;

    // Verify review exists
    let db = open_and_sync(repo_root)?;
    if db.get_review(review_id)?.is_none() {
        bail!("Review not found: {}", review_id);
    }

    let reviewer_list: Vec<String> = reviewers
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if reviewer_list.is_empty() {
        bail!("No reviewers specified");
    }

    let author = get_agent_identity(author);
    let event = EventEnvelope::new(
        &author,
        Event::ReviewersRequested(ReviewersRequested {
            review_id: review_id.to_string(),
            reviewers: reviewer_list.clone(),
        }),
    );

    let log = open_or_create(&events_path(repo_root))?;
    log.append(&event)?;

    let result = serde_json::json!({
        "review_id": review_id,
        "reviewers": reviewer_list,
    });

    let formatter = Formatter::new(format);
    formatter.print(&result)?;

    Ok(())
}

/// Approve a review.
pub fn run_reviews_approve(
    repo_root: &Path,
    review_id: &str,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(repo_root)?;

    // Verify review exists and is open
    let db = open_and_sync(repo_root)?;
    let review = db.get_review(review_id)?;
    match &review {
        None => bail!("Review not found: {}", review_id),
        Some(r) if r.status != "open" => {
            bail!(
                "Cannot approve review with status '{}': {}",
                r.status,
                review_id
            );
        }
        _ => {}
    }

    let author = get_agent_identity(author);
    let event = EventEnvelope::new(
        &author,
        Event::ReviewApproved(ReviewApproved {
            review_id: review_id.to_string(),
        }),
    );

    let log = open_or_create(&events_path(repo_root))?;
    log.append(&event)?;

    let result = serde_json::json!({
        "review_id": review_id,
        "status": "approved",
    });

    let formatter = Formatter::new(format);
    formatter.print(&result)?;

    Ok(())
}

/// Abandon a review.
pub fn run_reviews_abandon(
    repo_root: &Path,
    review_id: &str,
    reason: Option<String>,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(repo_root)?;

    // Verify review exists and is not already abandoned/merged
    let db = open_and_sync(repo_root)?;
    let review = db.get_review(review_id)?;
    match &review {
        None => bail!("Review not found: {}", review_id),
        Some(r) if r.status == "abandoned" => {
            bail!("Review is already abandoned: {}", review_id);
        }
        Some(r) if r.status == "merged" => {
            bail!("Cannot abandon merged review: {}", review_id);
        }
        _ => {}
    }

    let author = get_agent_identity(author);
    let event = EventEnvelope::new(
        &author,
        Event::ReviewAbandoned(ReviewAbandoned {
            review_id: review_id.to_string(),
            reason: reason.clone(),
        }),
    );

    let log = open_or_create(&events_path(repo_root))?;
    log.append(&event)?;

    let result = serde_json::json!({
        "review_id": review_id,
        "status": "abandoned",
        "reason": reason,
    });

    let formatter = Formatter::new(format);
    formatter.print(&result)?;

    Ok(())
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
