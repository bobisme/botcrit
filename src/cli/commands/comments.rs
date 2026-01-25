//! Implementation of `crit comments` subcommands.

use anyhow::{bail, Result};
use std::path::Path;

use crate::cli::commands::init::{events_path, index_path, is_initialized};
use crate::events::{get_agent_identity, new_comment_id, CommentAdded, Event, EventEnvelope};
use crate::log::{open_or_create, AppendLog};
use crate::output::{Formatter, OutputFormat};
use crate::projection::{sync_from_log, ProjectionDb};

/// Add a comment to a thread.
pub fn run_comments_add(
    repo_root: &Path,
    thread_id: &str,
    message: &str,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(repo_root)?;

    let db = open_and_sync(repo_root)?;

    // Verify thread exists
    let thread = db.get_thread(thread_id)?;
    if thread.is_none() {
        bail!("Thread not found: {}", thread_id);
    }

    let comment_id = new_comment_id();
    let author = get_agent_identity(author);

    let event = EventEnvelope::new(
        &author,
        Event::CommentAdded(CommentAdded {
            comment_id: comment_id.clone(),
            thread_id: thread_id.to_string(),
            body: message.to_string(),
        }),
    );

    let log = open_or_create(&events_path(repo_root))?;
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

/// List comments for a thread.
pub fn run_comments_list(repo_root: &Path, thread_id: &str, format: OutputFormat) -> Result<()> {
    ensure_initialized(repo_root)?;

    let db = open_and_sync(repo_root)?;

    // Verify thread exists
    if db.get_thread(thread_id)?.is_none() {
        bail!("Thread not found: {}", thread_id);
    }

    let comments = db.list_comments(thread_id)?;

    let formatter = Formatter::new(format);
    formatter.print_list(&comments, "No comments yet")?;

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
