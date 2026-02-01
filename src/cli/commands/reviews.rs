//! Implementation of `crit reviews` subcommands.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
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

/// Helper to create actionable "review not found" error messages.
fn review_not_found_error(review_id: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "Review not found: {}\n  To fix: crit --agent <your-name> reviews list",
        review_id
    )
}

/// Helper to create actionable "thread not found" error messages.
fn thread_not_found_error(thread_id: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "Thread not found: {}\n  To fix: crit --agent <your-name> threads list <review_id>",
        thread_id
    )
}

/// Parse a --since value into a DateTime.
/// Supports:
/// - ISO 8601 timestamps: "2026-01-27T23:00:00Z"
/// - Relative durations: "1h", "2d", "30m", "1w"
pub fn parse_since(value: &str) -> Result<DateTime<Utc>> {
    // Try ISO 8601 first
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc));
    }

    // Try relative duration
    let value = value.trim().to_lowercase();
    if let Some(num_str) = value.strip_suffix('h') {
        let hours: i64 = num_str.parse().context("Invalid hours")?;
        return Ok(Utc::now() - Duration::hours(hours));
    }
    if let Some(num_str) = value.strip_suffix('d') {
        let days: i64 = num_str.parse().context("Invalid days")?;
        return Ok(Utc::now() - Duration::days(days));
    }
    if let Some(num_str) = value.strip_suffix('m') {
        let mins: i64 = num_str.parse().context("Invalid minutes")?;
        return Ok(Utc::now() - Duration::minutes(mins));
    }
    if let Some(num_str) = value.strip_suffix('w') {
        let weeks: i64 = num_str.parse().context("Invalid weeks")?;
        return Ok(Utc::now() - Duration::weeks(weeks));
    }

    bail!(
        "Invalid --since format. Use ISO 8601 (2026-01-27T23:00:00Z) or relative (1h, 2d, 30m, 1w)"
    )
}

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
    let author = get_agent_identity(author)?;

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

    // Add next steps for TOON format (agents need guidance on what to do next)
    if format == OutputFormat::Toon {
        println!();
        println!("Next steps:");
        println!("  • Add reviewers:");
        println!("    crit --agent <your-name> reviews request {review_id} --reviewers other-agent");
        println!("  • Add comments:");
        println!("    crit --agent <your-name> comment {review_id} --file path/to/file.rs --line 10 \"feedback\"");
        println!("  • View the review:");
        println!("    crit --agent <your-name> review {review_id}");
    }

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
            return Err(review_not_found_error(&review_id));
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
        return Err(review_not_found_error(&review_id));
    }

    let reviewer_list: Vec<String> = reviewers
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if reviewer_list.is_empty() {
        bail!("No reviewers specified");
    }

    let author = get_agent_identity(author)?;
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
        None => return Err(review_not_found_error(&review_id)),
        Some(r) if r.status != "open" => {
            bail!(
                "Cannot approve review with status '{}': {}",
                r.status,
                review_id
            );
        }
        _ => {}
    }

    let author = get_agent_identity(author)?;
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
        None => return Err(review_not_found_error(&review_id)),
        Some(r) if r.status == "abandoned" => {
            bail!("Review is already abandoned: {}", review_id);
        }
        Some(r) if r.status == "merged" => {
            bail!("Cannot abandon merged review: {}", review_id);
        }
        _ => {}
    }

    let author = get_agent_identity(author)?;
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
        None => return Err(review_not_found_error(&review_id)),
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
            let author_str = get_agent_identity(author)?;
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
            "Cannot merge review with blocking votes:\n{}\n\nReviewers must change their vote with 'crit --agent <their-name> lgtm {}' before merging.",
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

    let author = get_agent_identity(author)?;
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
        None => return Err(review_not_found_error(&review_id)),
        Some(r) if r.status == "merged" => {
            bail!("Cannot vote on merged review: {}", review_id);
        }
        Some(r) if r.status == "abandoned" => {
            bail!("Cannot vote on abandoned review: {}", review_id);
        }
        _ => {}
    }

    let author = get_agent_identity(author)?;
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
/// * `since` - Optional filter to only show activity after this time
pub fn run_review(
    crit_root: &Path,
    workspace_root: &Path,
    review_id: &str,
    context_lines: u32,
    since: Option<DateTime<Utc>>,
    format: OutputFormat,
) -> Result<()> {
    use crate::jj::context::{extract_context, format_context};

    ensure_initialized(crit_root)?;

    let db = open_and_sync(crit_root)?;
    let review = db.get_review(review_id)?;

    let Some(review) = review else {
        return Err(review_not_found_error(&review_id));
    };

    let jj = JjRepo::new(workspace_root);

    // Find which workspace contains this change (if any)
    let workspace_info = jj
        .find_workspace_for_change(&review.jj_change_id)
        .ok()
        .flatten();

    // For JSON output, build a complete structure
    if matches!(format, OutputFormat::Json) {
        let threads = db.list_threads(review_id, None, None)?;
        let mut threads_with_comments = Vec::new();

        // Determine commit for context (same logic as TOON output)
        let commit_ref = review
            .final_commit
            .clone()
            .or_else(|| jj.get_commit_for_rev(&review.jj_change_id).ok())
            .unwrap_or_else(|| "@".to_string());

        for thread in threads {
            let comments = db.list_comments(&thread.thread_id)?;
            // Filter comments by since if provided
            let filtered_comments: Vec<_> = if let Some(since_dt) = since {
                comments
                    .into_iter()
                    .filter(|c| {
                        DateTime::parse_from_rfc3339(&c.created_at)
                            .map(|dt| dt.with_timezone(&Utc) >= since_dt)
                            .unwrap_or(true)
                    })
                    .collect()
            } else {
                comments
            };

            // Skip threads with no activity since the cutoff
            if since.is_some() && filtered_comments.is_empty() {
                continue;
            }

            // Extract code context for this thread
            let anchor_start = thread.selection_start as u32;
            let anchor_end = thread.selection_end.unwrap_or(thread.selection_start) as u32;

            let context_value = if context_lines > 0 {
                match extract_context(
                    &jj,
                    &thread.file_path,
                    &commit_ref,
                    anchor_start,
                    anchor_end,
                    context_lines,
                ) {
                    Ok(ctx) => serde_json::to_value(&ctx).ok(),
                    Err(_) => None,
                }
            } else {
                None
            };

            threads_with_comments.push(serde_json::json!({
                "thread_id": thread.thread_id,
                "file_path": thread.file_path,
                "selection_start": thread.selection_start,
                "selection_end": thread.selection_end,
                "status": thread.status,
                "context": context_value,
                "comments": filtered_comments,
            }));
        }

        let workspace_json = workspace_info.as_ref().map(|(name, path)| {
            serde_json::json!({
                "name": name,
                "path": path.display().to_string(),
            })
        });

        let result = serde_json::json!({
            "review": review,
            "workspace": workspace_json,
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

    // Show workspace info if the change is in a non-default workspace
    if let Some((workspace_name, workspace_path)) = &workspace_info {
        println!("  Workspace: {} ({})", workspace_name, workspace_path.display());
    }

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

    // Show filter notice if --since is active
    if let Some(since_dt) = since {
        println!(
            "\n  [Showing activity since {}]",
            since_dt.format("%Y-%m-%d %H:%M")
        );
    }

    // Group threads by file, filtering by since if provided
    let mut threads_by_file: std::collections::BTreeMap<String, Vec<_>> =
        std::collections::BTreeMap::new();
    let mut total_new_comments = 0;

    for thread in threads {
        // Get comments and filter by since
        let comments = db.list_comments(&thread.thread_id)?;
        let filtered_comments: Vec<_> = if let Some(since_dt) = since {
            comments
                .into_iter()
                .filter(|c| {
                    DateTime::parse_from_rfc3339(&c.created_at)
                        .map(|dt| dt.with_timezone(&Utc) >= since_dt)
                        .unwrap_or(true)
                })
                .collect()
        } else {
            comments
        };

        // Skip threads with no new activity when filtering
        if since.is_some() && filtered_comments.is_empty() {
            continue;
        }

        total_new_comments += filtered_comments.len();
        threads_by_file
            .entry(thread.file_path.clone())
            .or_default()
            .push((thread, filtered_comments));
    }

    if since.is_some() && threads_by_file.is_empty() {
        println!("\n  No new activity since the specified time.");
        return Ok(());
    }

    // Determine commit for context
    let commit_ref = review
        .final_commit
        .clone()
        .or_else(|| jj.get_commit_for_rev(&review.jj_change_id).ok())
        .unwrap_or_else(|| "@".to_string());

    for (file, file_threads) in threads_by_file {
        println!("\n━━━ {} ━━━", file);

        for (thread, comments) in file_threads {
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

            let new_indicator = if since.is_some() {
                format!(" [+{}]", comments.len())
            } else {
                String::new()
            };

            println!(
                "\n  {} {} ({}){}",
                status_icon, thread.thread_id, line_info, new_indicator
            );

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

            // Show comments (already filtered)
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

    if let Some(_) = since {
        println!("\n  [{} new comment(s)]", total_new_comments);
    }

    println!();
    Ok(())
}

/// Show inbox - reviews and threads needing the agent's attention.
pub fn run_inbox(repo_root: &Path, agent: &str, format: OutputFormat) -> Result<()> {
    ensure_initialized(repo_root)?;

    let db = open_and_sync(repo_root)?;
    let inbox = db.get_inbox(agent)?;

    if matches!(format, OutputFormat::Json) {
        let formatter = Formatter::new(format);
        formatter.print(&inbox)?;
        return Ok(());
    }

    // TOON output
    let total_items = inbox.reviews_awaiting_vote.len()
        + inbox.threads_with_new_responses.len()
        + inbox.open_threads_on_my_reviews.len();

    if total_items == 0 {
        println!("Inbox empty - no items need your attention");
        return Ok(());
    }

    println!("Inbox for {} ({} items)", agent, total_items);
    println!();

    // Section 1: Reviews awaiting vote
    if !inbox.reviews_awaiting_vote.is_empty() {
        println!(
            "Reviews awaiting your vote ({}):",
            inbox.reviews_awaiting_vote.len()
        );
        for r in &inbox.reviews_awaiting_vote {
            let threads_info = if r.open_thread_count > 0 {
                format!(" [{} open threads]", r.open_thread_count)
            } else {
                String::new()
            };
            println!(
                "  {} · {} by {}{}",
                r.review_id, r.title, r.author, threads_info
            );
        }
        println!();
    }

    // Section 2: Threads with new responses
    if !inbox.threads_with_new_responses.is_empty() {
        println!(
            "Threads with new responses ({}):",
            inbox.threads_with_new_responses.len()
        );
        for t in &inbox.threads_with_new_responses {
            println!(
                "  {} · {}:{} (+{} new)",
                t.thread_id, t.file_path, t.selection_start, t.new_response_count
            );
            println!("    in {} ({})", t.review_id, t.review_title);
        }
        println!();
    }

    // Section 3: Open threads on my reviews
    if !inbox.open_threads_on_my_reviews.is_empty() {
        println!(
            "Open feedback on your reviews ({}):",
            inbox.open_threads_on_my_reviews.len()
        );
        for t in &inbox.open_threads_on_my_reviews {
            let comments_info = if t.comment_count > 0 {
                format!(" ({} comments)", t.comment_count)
            } else {
                String::new()
            };
            println!(
                "  {} · {}:{} by {}{}",
                t.thread_id, t.file_path, t.selection_start, t.thread_author, comments_info
            );
            println!("    in {} ({})", t.review_id, t.review_title);
        }
        println!();
    }

    Ok(())
}

// ============================================================================
// Helpers
// ============================================================================

fn ensure_initialized(repo_root: &Path) -> Result<()> {
    if !is_initialized(repo_root) {
        bail!("Not a crit repository. Run 'crit --agent <your-name> init' first.");
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
