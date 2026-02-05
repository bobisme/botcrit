//! Implementation of `crit reviews` subcommands.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use std::path::Path;

use crate::cli::commands::helpers::{ensure_initialized, open_and_sync};
use crate::critignore::{AllFilesIgnoredError, CritIgnore};
use crate::events::{
    get_agent_identity, new_review_id, Event, EventEnvelope, ReviewAbandoned, ReviewApproved,
    ReviewCreated, ReviewMerged, ReviewerVoted, ReviewersRequested, VoteType,
};
use crate::jj::JjRepo;
use crate::log::{open_or_create_review, AppendLog};
use crate::output::{Formatter, OutputFormat};
use crate::version::require_v2;

/// Helper to create actionable "review not found" error messages.
fn review_not_found_error(review_id: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "Review not found: {}\n  To fix: crit --agent <your-name> reviews list",
        review_id
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

    // Check if there are any non-ignored files to review
    let parent_commit = jj.get_parent_commit(&commit_id)?;
    let all_files = jj.changed_files_between(&parent_commit, &commit_id)?;
    let critignore = CritIgnore::load(crit_root);
    let (reviewable_files, ignored_count) = critignore.filter_files(all_files);

    if reviewable_files.is_empty() {
        if ignored_count > 0 {
            // All files were ignored
            return Err(AllFilesIgnoredError {
                ignored_count,
                has_critignore: CritIgnore::has_critignore_file(crit_root),
            }
            .into());
        }
        // No files changed at all
        bail!("No files changed in this commit. Nothing to review.");
    }

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

    // Enforce v2 format
    require_v2(crit_root)?;

    // Write to per-review event log (v2)
    let log = open_or_create_review(crit_root, &review_id)?;
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
        println!();
        println!("After reviewer feedback, re-request review:");
        println!("    crit --agent <your-name> reviews request {review_id} --reviewers <reviewer>");
        println!("  (Reviewer sees [re-review] in their inbox)");
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
    all_workspaces: bool,
    format: OutputFormat,
) -> Result<()> {
    use crate::jj::resolve_repo_root;
    use crate::projection::ReviewSummary;
    use std::collections::{HashMap, HashSet};

    ensure_initialized(crit_root)?;

    if all_workspaces {
        // Get the main repo root (not workspace-local)
        let repo_root = resolve_repo_root(crit_root)
            .context("--all-workspaces requires a jj repository")?;
        let jj = JjRepo::new(&repo_root);

        // Get all workspaces
        let workspaces = jj.list_workspaces()?;

        // Collect reviews from all workspace .crit/ directories
        let mut all_reviews: HashMap<String, (ReviewSummary, String, std::path::PathBuf)> = HashMap::new();
        let mut seen_review_ids: HashSet<String> = HashSet::new();

        for (ws_name, _ws_change_id, ws_path) in &workspaces {
            let ws_crit = ws_path.join(".crit");
            if !ws_crit.exists() {
                continue;
            }

            // Try to open and sync this workspace's .crit/
            match open_and_sync(&ws_crit.parent().unwrap_or(ws_path)) {
                Ok(db) => {
                    match db.list_reviews_filtered(status, author, needs_reviewer, has_unresolved) {
                        Ok(reviews) => {
                            for review in reviews {
                                // Only add if we haven't seen this review_id before
                                if !seen_review_ids.contains(&review.review_id) {
                                    seen_review_ids.insert(review.review_id.clone());
                                    all_reviews.insert(
                                        review.review_id.clone(),
                                        (review, ws_name.clone(), ws_path.clone()),
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Warning: failed to list reviews in workspace {}: {}", ws_name, e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Warning: failed to sync workspace {}: {}", ws_name, e);
                }
            }
        }

        if all_reviews.is_empty() {
            println!("No reviews found across workspaces");
            return Ok(());
        }

        // Group reviews by workspace, matching review's change_id to workspace's change_id
        let mut by_workspace: HashMap<String, Vec<&ReviewSummary>> = HashMap::new();
        let mut no_workspace: Vec<&ReviewSummary> = Vec::new();

        // Build workspace change_id lookup (short prefix -> workspace name)
        let ws_change_lookup: HashMap<&str, &str> = workspaces
            .iter()
            .map(|(name, change_id, _)| (change_id.as_str(), name.as_str()))
            .collect();

        for (review, _source_ws, _source_path) in all_reviews.values() {
            // Try to match review's change_id to a workspace
            let review_change_prefix = &review.jj_change_id[..8.min(review.jj_change_id.len())];
            if let Some(&ws_name) = ws_change_lookup.get(review_change_prefix) {
                by_workspace
                    .entry(ws_name.to_string())
                    .or_default()
                    .push(review);
            } else {
                no_workspace.push(review);
            }
        }

        // Output grouped by workspace
        if matches!(format, OutputFormat::Json) {
            // For JSON, output a structured object
            use serde::Serialize;
            #[derive(Serialize)]
            struct WorkspaceReviews<'a> {
                workspace: &'a str,
                path: String,
                reviews: Vec<&'a ReviewSummary>,
            }

            let mut output: Vec<WorkspaceReviews<'_>> = Vec::new();

            for (ws_name, _ws_change_id, ws_path) in &workspaces {
                if let Some(reviews) = by_workspace.get(ws_name) {
                    let rel_path = if ws_name == "default" {
                        ".".to_string()
                    } else {
                        ws_path.strip_prefix(&repo_root)
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|_| ws_path.display().to_string())
                    };
                    output.push(WorkspaceReviews {
                        workspace: ws_name,
                        path: rel_path,
                        reviews: reviews.iter().copied().collect(),
                    });
                }
            }

            if !no_workspace.is_empty() {
                output.push(WorkspaceReviews {
                    workspace: "(no active workspace)",
                    path: String::new(),
                    reviews: no_workspace,
                });
            }

            let formatter = Formatter::new(format);
            formatter.print(&output)?;
        } else {
            // TOON output - group by workspace with headers
            for (ws_name, _ws_change_id, ws_path) in &workspaces {
                if let Some(reviews) = by_workspace.get(ws_name) {
                    let rel_path = if ws_name == "default" {
                        ".".to_string()
                    } else {
                        ws_path.strip_prefix(&repo_root)
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|_| ws_path.display().to_string())
                    };
                    println!("=== {} ({}) ===", ws_name, rel_path);
                    for r in reviews {
                        println!(
                            "  {} · {} by {} [{}]",
                            r.review_id, r.title, r.author, r.status
                        );
                    }
                    println!();
                }
            }

            if !no_workspace.is_empty() {
                println!("=== (no active workspace) ===");
                for r in &no_workspace {
                    println!(
                        "  {} · {} by {} [{}]",
                        r.review_id, r.title, r.author, r.status
                    );
                }
                println!();
            }
        }
    } else {
        // Original single-workspace behavior
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
    }

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

    let log = open_or_create_review(repo_root, review_id)?;
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

    let log = open_or_create_review(repo_root, review_id)?;
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

    let log = open_or_create_review(repo_root, review_id)?;
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
            let log = open_or_create_review(crit_root, review_id)?;
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

    let log = open_or_create_review(crit_root, review_id)?;
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
    let review_status = match &review {
        None => return Err(review_not_found_error(&review_id)),
        Some(r) if r.status == "merged" => {
            bail!("Cannot vote on merged review: {}", review_id);
        }
        Some(r) if r.status == "abandoned" => {
            bail!("Cannot vote on abandoned review: {}", review_id);
        }
        Some(r) => r.status.clone(),
    };

    let author = get_agent_identity(author)?;
    let event = EventEnvelope::new(
        &author,
        Event::ReviewerVoted(ReviewerVoted {
            review_id: review_id.to_string(),
            vote: vote.clone(),
            reason: reason.clone(),
        }),
    );

    let log = open_or_create_review(repo_root, review_id)?;
    log.append(&event)?;

    // Auto-approve on LGTM if review is open and no blocking votes from others
    let auto_approved = if vote == VoteType::Lgtm && review_status == "open" {
        // Re-sync to see our newly recorded vote
        let db = open_and_sync(repo_root)?;
        let has_blocks = db.has_blocking_votes_from_others(review_id, &author)?;
        if !has_blocks {
            // Auto-approve the review
            let approve_event = EventEnvelope::new(
                &author,
                Event::ReviewApproved(ReviewApproved {
                    review_id: review_id.to_string(),
                }),
            );
            log.append(&approve_event)?;
            true
        } else {
            false
        }
    } else {
        false
    };

    let mut result = serde_json::json!({
        "review_id": review_id,
        "vote": vote.to_string(),
        "reason": reason,
        "voter": author,
    });
    if auto_approved {
        result["auto_approved"] = serde_json::json!(true);
    }

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
pub fn run_inbox(repo_root: &Path, agent: &str, all_workspaces: bool, format: OutputFormat) -> Result<()> {
    use crate::jj::resolve_repo_root;
    use crate::projection::InboxSummary;
    use std::collections::HashSet;

    ensure_initialized(repo_root)?;

    if all_workspaces {
        // Get the main repo root (not workspace-local)
        let main_repo_root = resolve_repo_root(repo_root)
            .context("--all-workspaces requires a jj repository")?;
        let jj = JjRepo::new(&main_repo_root);

        // Get all workspaces
        let workspaces = jj.list_workspaces()?;

        // Collect inbox items from all workspace .crit/ directories
        // Track seen review_ids and thread_ids to avoid duplicates
        let mut seen_reviews: HashSet<String> = HashSet::new();
        let mut seen_threads: HashSet<String> = HashSet::new();

        // Cross-workspace exclusion sets. Stale workspaces may lack recent events,
        // so a review can appear "awaiting vote" in a stale workspace even though
        // the agent already voted in the up-to-date one. We collect exclusions from
        // ALL workspaces, then post-filter the combined inbox.
        let mut voted_reviews: HashSet<String> = HashSet::new();
        let mut terminal_reviews: HashSet<String> = HashSet::new();

        let mut combined_inbox = InboxSummary {
            reviews_awaiting_vote: Vec::new(),
            threads_with_new_responses: Vec::new(),
            open_threads_on_my_reviews: Vec::new(),
        };

        // Map review_id to workspace name for annotation
        let mut review_workspace: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let mut thread_workspace: std::collections::HashMap<String, String> = std::collections::HashMap::new();

        for (ws_name, _ws_change_id, ws_path) in &workspaces {
            let ws_crit = ws_path.join(".crit");
            if !ws_crit.exists() {
                continue;
            }

            if let Ok(db) = open_and_sync(&ws_crit.parent().unwrap_or(ws_path)) {
                // Collect exclusions from every workspace
                if let Ok(v) = db.get_voted_reviews(agent) {
                    voted_reviews.extend(v);
                }
                if let Ok(t) = db.get_terminal_reviews() {
                    terminal_reviews.extend(t);
                }

                if let Ok(inbox) = db.get_inbox(agent) {
                    for r in inbox.reviews_awaiting_vote {
                        if !seen_reviews.contains(&r.review_id) {
                            seen_reviews.insert(r.review_id.clone());
                            review_workspace.insert(r.review_id.clone(), ws_name.clone());
                            combined_inbox.reviews_awaiting_vote.push(r);
                        }
                    }
                    for t in inbox.threads_with_new_responses {
                        if !seen_threads.contains(&t.thread_id) {
                            seen_threads.insert(t.thread_id.clone());
                            thread_workspace.insert(t.thread_id.clone(), ws_name.clone());
                            combined_inbox.threads_with_new_responses.push(t);
                        }
                    }
                    for t in inbox.open_threads_on_my_reviews {
                        let key = format!("{}:{}", t.review_id, t.thread_id);
                        if !seen_threads.contains(&key) {
                            seen_threads.insert(key);
                            thread_workspace.insert(t.thread_id.clone(), ws_name.clone());
                            combined_inbox.open_threads_on_my_reviews.push(t);
                        }
                    }
                }
            }
        }

        // Filter out reviews the agent already voted on (in any workspace)
        // or that reached terminal status in any workspace
        combined_inbox.reviews_awaiting_vote.retain(|r| {
            !voted_reviews.contains(&r.review_id) && !terminal_reviews.contains(&r.review_id)
        });
        // Filter thread items for reviews in terminal status
        combined_inbox.threads_with_new_responses.retain(|t| !terminal_reviews.contains(&t.review_id));
        combined_inbox.open_threads_on_my_reviews.retain(|t| !terminal_reviews.contains(&t.review_id));

        let inbox = combined_inbox;

        if matches!(format, OutputFormat::Json) {
            let formatter = Formatter::new(format);
            formatter.print(&inbox)?;
            return Ok(());
        }

        // TOON output with workspace annotations
        let total_items = inbox.reviews_awaiting_vote.len()
            + inbox.threads_with_new_responses.len()
            + inbox.open_threads_on_my_reviews.len();

        if total_items == 0 {
            println!("Inbox empty - no items need your attention (across all workspaces)");
            return Ok(());
        }

        println!("Inbox for {} ({} items, all workspaces)", agent, total_items);
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
                let status_indicator = if r.request_status == "re-review" {
                    " [re-review]"
                } else {
                    ""
                };
                let ws_name = review_workspace.get(&r.review_id).map(|s| s.as_str()).unwrap_or("?");
                println!(
                    "  {} · {} by {}{}{} [{}]",
                    r.review_id, r.title, r.author, threads_info, status_indicator, ws_name
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
                let ws_name = thread_workspace.get(&t.thread_id).map(|s| s.as_str()).unwrap_or("?");
                println!(
                    "  {} · {}:{} (+{} new) [{}]",
                    t.thread_id, t.file_path, t.selection_start, t.new_response_count, ws_name
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
                let ws_name = thread_workspace.get(&t.thread_id).map(|s| s.as_str()).unwrap_or("?");
                println!(
                    "  {} · {}:{} by {}{} [{}]",
                    t.thread_id, t.file_path, t.selection_start, t.thread_author, comments_info, ws_name
                );
                println!("    in {} ({})", t.review_id, t.review_title);
            }
            println!();
        }
    } else {
        // Original single-workspace behavior
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
                let status_indicator = if r.request_status == "re-review" {
                    " [re-review]"
                } else {
                    ""
                };
                println!(
                    "  {} · {} by {}{}{}",
                    r.review_id, r.title, r.author, threads_info, status_indicator
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
    }

    Ok(())
}

