//! Implementation of `crit comments` subcommands.

use anyhow::{bail, Result};
use std::path::Path;

use crate::cli::commands::helpers::{
    ensure_initialized, open_and_sync, resolve_review_thread_commit, review_not_found_error,
    thread_not_found_error,
};
use crate::cli::commands::threads::parse_line_selection;
use crate::events::{
    get_agent_identity, make_comment_id, new_thread_id, CommentAdded, Event, EventEnvelope,
    ThreadCreated,
};
use crate::log::{open_or_create_review, AppendLog};
use crate::output::{Formatter, OutputFormat};
use crate::scm::ScmRepo;

/// Add a comment to a thread.
#[tracing::instrument(skip(repo_root, message, format))]
pub fn run_comments_add(
    repo_root: &Path,
    thread_id: &str,
    message: &str,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(repo_root)?;

    let db = open_and_sync(repo_root)?;

    // Verify thread exists and get its review_id
    let thread = db.get_thread(thread_id)?;
    let thread = match thread {
        None => return Err(thread_not_found_error(repo_root, thread_id)),
        Some(t) => t,
    };

    // Verify review is open
    let review = db.get_review(&thread.review_id)?;
    if let Some(r) = &review {
        if r.status != "open" {
            bail!(
                "Cannot comment on review with status '{}': {}",
                r.status,
                thread.review_id
            );
        }
    }

    // Get next comment number
    let comment_number = db
        .get_next_comment_number(thread_id)?
        .expect("thread exists but has no comment number");

    let comment_id = make_comment_id(thread_id, comment_number);
    let author = get_agent_identity(author)?;

    let event = EventEnvelope::new(
        &author,
        Event::CommentAdded(CommentAdded {
            comment_id: comment_id.clone(),
            thread_id: thread_id.to_string(),
            body: message.to_string(),
        }),
    );

    // Write to the per-review log (v2)
    let log = open_or_create_review(repo_root, &thread.review_id)?;
    log.append(&event)?;

    let result = serde_json::json!({
        "comment_id": comment_id,
        "thread_id": thread_id,
        "author": author,
        "body": message,
    });

    let formatter = Formatter::new(format);
    formatter.print(&result)?;

    Ok(())
}

/// Add a comment to a review, auto-creating a thread if needed.
///
/// This is the simplified comment workflow for agents:
/// - If a thread already exists at the file+line, adds comment to it
/// - If no thread exists, creates one and adds the comment
///
/// # Arguments
/// * `crit_root` - Path to main repo (where .crit/ lives)
/// * `workspace_root` - Path to current workspace (for jj @ resolution)
#[tracing::instrument(skip(crit_root, scm, message, format))]
pub fn run_comment(
    crit_root: &Path,
    scm: &dyn ScmRepo,
    review_id: &str,
    file: &str,
    line: &str,
    message: &str,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(crit_root)?;

    let db = open_and_sync(crit_root)?;

    // Verify review exists and is open
    let review = match db.get_review(review_id)? {
        None => return Err(review_not_found_error(crit_root, review_id)),
        Some(r) => r,
    };

    if review.status != "open" {
        bail!(
            "Cannot comment on review with status '{}': {}",
            review.status,
            review_id
        );
    }

    // Parse line selection
    let selection = parse_line_selection(line)?;
    let start_line = selection.start_line() as i64;

    // Resolve author identity once for both thread creation and comment
    let author_str = get_agent_identity(author)?;

    // Check for existing thread at this location
    let (thread_id, comment_number) =
        match db.find_thread_at_location(review_id, file, start_line)? {
            Some(existing_id) => {
                // Use existing thread - get its next comment number
                let comment_number = db
                    .get_next_comment_number(&existing_id)?
                    .expect("thread exists but has no comment number");
                (existing_id, comment_number)
            }
            None => {
                // Create new thread
                let commit_hash = resolve_review_thread_commit(scm, &review);

                // Verify file exists at the review's commit anchor
                if !scm.file_exists(&commit_hash, file)? {
                    bail!(
                        "File does not exist in review {} at {}: {}",
                        review_id,
                        commit_hash,
                        file
                    );
                }

                let new_thread_id = new_thread_id();

                let thread_event = EventEnvelope::new(
                    &author_str,
                    Event::ThreadCreated(ThreadCreated {
                        thread_id: new_thread_id.clone(),
                        review_id: review_id.to_string(),
                        file_path: file.to_string(),
                        selection: selection.clone(),
                        commit_hash,
                    }),
                );

                // Write to the per-review log (v2)
                let log = open_or_create_review(crit_root, review_id)?;
                log.append(&thread_event)?;

                // New thread starts at comment number 1
                (new_thread_id, 1)
            }
        };

    // Now add the comment to the thread
    let comment_id = make_comment_id(&thread_id, comment_number);

    let comment_event = EventEnvelope::new(
        &author_str,
        Event::CommentAdded(CommentAdded {
            comment_id: comment_id.clone(),
            thread_id: thread_id.clone(),
            body: message.to_string(),
        }),
    );

    // Write to the per-review log (v2)
    let log = open_or_create_review(crit_root, review_id)?;
    log.append(&comment_event)?;

    // Output result
    let result = serde_json::json!({
        "comment_id": comment_id,
        "thread_id": thread_id,
        "review_id": review_id,
        "file": file,
        "line": start_line,
        "author": author_str,
        "body": message,
    });

    let formatter = Formatter::new(format);
    formatter.print(&result)?;

    Ok(())
}

/// List comments for a thread.
pub fn run_comments_list(repo_root: &Path, thread_id: &str, format: OutputFormat) -> Result<()> {
    ensure_initialized(repo_root)?;

    let db = open_and_sync(repo_root)?;

    // Verify thread exists
    if db.get_thread(thread_id)?.is_none() {
        return Err(thread_not_found_error(repo_root, thread_id));
    }

    let comments = db.list_comments(thread_id)?;

    let formatter = Formatter::new(format);
    formatter.print_list(
        &comments,
        "No comments yet",
        "comments",
        &["crit reply <thread_id> \"...\""],
    )?;

    Ok(())
}
