//! Implementation of `crit reviews` subcommands.

use anyhow::{bail, Context, Result};
use std::path::Path;

use crate::cli::commands::init::{events_path, index_path, is_initialized};
use crate::events::{
    get_agent_identity, new_review_id, Event, EventEnvelope, ReviewAbandoned, ReviewApproved,
    ReviewCreated, ReviewMerged, ReviewerVoted, ReviewersRequested, VoteType,
};
use crate::jj::JjRepo;
use crate::log::{open_or_create, AppendLog};
use crate::output::{Formatter, OutputFormat};
use crate::projection::{sync_from_log, ProjectionDb};

/// Create a new review for the current jj change.
///
/// # Arguments
/// * `crit_root` - Path to main repo (where .crit/ lives)
/// * `workspace_root` - Path to current workspace (for jj @ resolution)
pub fn run_reviews_create(
    crit_root: &Path,
    workspace_root: &Path,
    title: String,
    description: Option<String>,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(crit_root)?;

    // Use workspace_root for jj commands so @ resolves to the workspace's working copy
    let jj = JjRepo::new(workspace_root);
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

    // Use crit_root for storage
    let log = open_or_create(&events_path(crit_root))?;
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
    crit_root: &Path,
    status: Option<&str>,
    author: Option<&str>,
    needs_reviewer: Option<&str>,
    has_unresolved: bool,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(crit_root)?;

    let db = open_and_sync(crit_root)?;
    let reviews = db.list_reviews_filtered(status, author, needs_reviewer, has_unresolved)?;

    // Build context-aware empty message
    let empty_msg = if needs_reviewer.is_some() {
        "No reviews need your attention"
    } else if has_unresolved {
        "No reviews have unresolved threads"
    } else if status.is_some() || author.is_some() {
        "No reviews match the filters"
    } else {
        "No reviews yet"
    };

    let formatter = Formatter::new(format);
    formatter.print_list(&reviews, empty_msg)?;

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

/// Mark a review as merged.
///
/// # Arguments
/// * `crit_root` - Path to main repo (where .crit/ lives)
/// * `workspace_root` - Path to current workspace (for jj @ resolution)
/// * `self_approve` - If true, auto-approve open reviews before merging
pub fn run_reviews_merge(
    crit_root: &Path,
    workspace_root: &Path,
    review_id: &str,
    commit: Option<String>,
    self_approve: bool,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(crit_root)?;

    // Verify review exists
    let db = open_and_sync(crit_root)?;
    let review = db.get_review(review_id)?;
    match &review {
        None => bail!("Review not found: {}", review_id),
        Some(r) if r.status == "merged" => {
            bail!("Review is already merged: {}", review_id);
        }
        Some(r) if r.status == "abandoned" => {
            bail!("Cannot merge abandoned review: {}", review_id);
        }
        Some(r) if r.status == "open" && !self_approve => {
            bail!(
                "Cannot merge unapproved review: {}. Approve it first, or use --self-approve.",
                review_id
            );
        }
        Some(r) if r.status == "open" && self_approve => {
            // Auto-approve the review first
            let author_str = get_agent_identity(author);
            let approve_event = EventEnvelope::new(
                &author_str,
                Event::ReviewApproved(ReviewApproved {
                    review_id: review_id.to_string(),
                }),
            );
            let log = open_or_create(&events_path(crit_root))?;
            log.append(&approve_event)?;
        }
        _ => {}
    }

    // Check for blocking votes
    if db.has_blocking_votes(review_id)? {
        let votes = db.get_votes(review_id)?;
        let blockers: Vec<_> = votes
            .iter()
            .filter(|v| v.vote == "block")
            .map(|v| {
                if let Some(reason) = &v.reason {
                    format!("  - {} ({})", v.reviewer, reason)
                } else {
                    format!("  - {}", v.reviewer)
                }
            })
            .collect();

        bail!(
            "Cannot merge review with blocking votes:\n{}\n\nReviewers must change their vote with 'crit lgtm {}' before merging.",
            blockers.join("\n"),
            review_id
        );
    }

    // Get final commit hash - either provided or auto-detect from @
    // Use workspace_root for jj commands so @ resolves correctly
    let jj = JjRepo::new(workspace_root);
    let final_commit = match commit {
        Some(c) => c,
        None => jj
            .get_current_commit()
            .context("Failed to get current commit for merge")?,
    };

    let author = get_agent_identity(author);
    let event = EventEnvelope::new(
        &author,
        Event::ReviewMerged(ReviewMerged {
            review_id: review_id.to_string(),
            final_commit: final_commit.clone(),
        }),
    );

    let log = open_or_create(&events_path(crit_root))?;
    log.append(&event)?;

    let result = serde_json::json!({
        "review_id": review_id,
        "status": "merged",
        "final_commit": final_commit,
    });

    let formatter = Formatter::new(format);
    formatter.print(&result)?;

    Ok(())
}

/// Vote LGTM on a review.
pub fn run_lgtm(
    repo_root: &Path,
    review_id: &str,
    message: Option<String>,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    run_vote(
        repo_root,
        review_id,
        VoteType::Lgtm,
        message,
        author,
        format,
    )
}

/// Block a review (request changes).
pub fn run_block(
    repo_root: &Path,
    review_id: &str,
    reason: String,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    run_vote(
        repo_root,
        review_id,
        VoteType::Block,
        Some(reason),
        author,
        format,
    )
}

/// Internal vote handler.
fn run_vote(
    repo_root: &Path,
    review_id: &str,
    vote: VoteType,
    reason: Option<String>,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(repo_root)?;

    // Verify review exists and is open or approved (approved reviews can still
    // receive votes, e.g., to change a block to lgtm after issues are fixed)
    let db = open_and_sync(repo_root)?;
    let review = db.get_review(review_id)?;
    match &review {
        None => bail!("Review not found: {}", review_id),
        Some(r) if r.status == "merged" => {
            bail!("Cannot vote on merged review: {}", review_id);
        }
        Some(r) if r.status == "abandoned" => {
            bail!("Cannot vote on abandoned review: {}", review_id);
        }
        _ => {}
    }

    let author = get_agent_identity(author);
    let event = EventEnvelope::new(
        &author,
        Event::ReviewerVoted(ReviewerVoted {
            review_id: review_id.to_string(),
            vote,
            reason: reason.clone(),
        }),
    );

    let log = open_or_create(&events_path(repo_root))?;
    log.append(&event)?;

    let result = serde_json::json!({
        "review_id": review_id,
        "vote": vote.to_string(),
        "reason": reason,
        "voter": author,
    });

    let formatter = Formatter::new(format);
    formatter.print(&result)?;

    Ok(())
}

/// Show full review with all threads and comments.
///
/// # Arguments
/// * `crit_root` - Path to main repo (where .crit/ lives)
/// * `workspace_root` - Path to current workspace (for jj @ resolution)
pub fn run_review(
    crit_root: &Path,
    workspace_root: &Path,
    review_id: &str,
    context_lines: u32,
    format: OutputFormat,
) -> Result<()> {
    use crate::jj::context::{extract_context, format_context};

    ensure_initialized(crit_root)?;

    let db = open_and_sync(crit_root)?;
    let review = db.get_review(review_id)?;

    let Some(review) = review else {
        bail!("Review not found: {}", review_id);
    };

    let jj = JjRepo::new(workspace_root);

    // For JSON output, build a complete structure
    if matches!(format, OutputFormat::Json) {
        let threads = db.list_threads(review_id, None, None)?;
        let mut threads_with_comments = Vec::new();

        for thread in threads {
            let comments = db.list_comments(&thread.thread_id)?;
            threads_with_comments.push(serde_json::json!({
                "thread_id": thread.thread_id,
                "file_path": thread.file_path,
                "selection_start": thread.selection_start,
                "selection_end": thread.selection_end,
                "status": thread.status,
                "comments": comments,
            }));
        }

        let result = serde_json::json!({
            "review": review,
            "threads": threads_with_comments,
        });

        let formatter = Formatter::new(format);
        formatter.print(&result)?;
        return Ok(());
    }

    // TOON output: human-readable format
    let status_symbol = match review.status.as_str() {
        "open" => "○",
        "approved" => "◐",
        "merged" => "●",
        "abandoned" => "✗",
        _ => "?",
    };

    println!("{} {} · {}", status_symbol, review.review_id, review.title);
    println!(
        "  Status: {} | Author: {} | Created: {}",
        review.status,
        review.author,
        &review.created_at[..10]
    );

    if let Some(desc) = &review.description {
        println!("\n  {}", desc);
    }

    // Show votes if any
    if !review.votes.is_empty() {
        println!("\n  Votes:");
        for vote in &review.votes {
            let icon = if vote.vote == "lgtm" { "✓" } else { "✗" };
            let reason = vote.reason.as_deref().unwrap_or("");
            if reason.is_empty() {
                println!("    {} {} ({})", icon, vote.reviewer, vote.vote);
            } else {
                println!("    {} {} ({}): {}", icon, vote.reviewer, vote.vote, reason);
            }
        }
    }

    // Get threads grouped by file
    let threads = db.list_threads(review_id, None, None)?;

    if threads.is_empty() {
        println!("\n  No threads.");
        return Ok(());
    }

    // Group threads by file
    let mut threads_by_file: std::collections::BTreeMap<String, Vec<_>> =
        std::collections::BTreeMap::new();
    for thread in threads {
        threads_by_file
            .entry(thread.file_path.clone())
            .or_default()
            .push(thread);
    }

    // Determine commit for context
    let commit_ref = review
        .final_commit
        .clone()
        .or_else(|| jj.get_commit_for_rev(&review.jj_change_id).ok())
        .unwrap_or_else(|| "@".to_string());

    for (file, file_threads) in threads_by_file {
        println!("\n━━━ {} ━━━", file);

        for thread in file_threads {
            let status_icon = if thread.status == "open" {
                "○"
            } else {
                "✓"
            };
            let line_info = match thread.selection_end {
                Some(end) if end != thread.selection_start => {
                    format!("lines {}-{}", thread.selection_start, end)
                }
                _ => format!("line {}", thread.selection_start),
            };

            println!("\n  {} {} ({})", status_icon, thread.thread_id, line_info);

            // Show code context if requested
            if context_lines > 0 {
                let anchor_start = thread.selection_start as u32;
                let anchor_end = thread.selection_end.unwrap_or(thread.selection_start) as u32;

                if let Ok(ctx) = extract_context(
                    &jj,
                    &file,
                    &commit_ref,
                    anchor_start,
                    anchor_end,
                    context_lines,
                ) {
                    // Indent the context
                    for line in format_context(&ctx).lines() {
                        println!("  {}", line);
                    }
                }
            }

            // Show comments
            let comments = db.list_comments(&thread.thread_id)?;
            for comment in comments {
                println!(
                    "\n    ▸ {} ({}):",
                    comment.author,
                    &comment.created_at[..10]
                );
                for line in comment.body.lines() {
                    println!("       {}", line);
                }
            }
        }
    }

    println!();
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
