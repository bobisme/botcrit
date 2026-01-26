//! Implementation of `crit threads` subcommands.

use anyhow::{bail, Context, Result};
use std::path::Path;

use crate::cli::commands::init::{events_path, index_path, is_initialized};
use crate::events::{
    get_agent_identity, new_thread_id, CodeSelection, Event, EventEnvelope, ThreadCreated,
    ThreadReopened, ThreadResolved,
};
use crate::jj::context::{extract_context, format_context};
use crate::jj::JjRepo;
use crate::log::{open_or_create, AppendLog};
use crate::output::{Formatter, OutputFormat};
use crate::projection::{sync_from_log, ProjectionDb};

/// Create a new comment thread on a file.
///
/// # Arguments
/// * `crit_root` - Path to main repo (where .crit/ lives)
/// * `workspace_root` - Path to current workspace (for jj @ resolution)
pub fn run_threads_create(
    crit_root: &Path,
    workspace_root: &Path,
    review_id: &str,
    file: &str,
    lines: &str,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(crit_root)?;

    // Verify review exists
    let db = open_and_sync(crit_root)?;
    let review = db.get_review(review_id)?;
    match &review {
        None => bail!("Review not found: {}", review_id),
        Some(r) if r.status != "open" => {
            bail!(
                "Cannot create thread on review with status '{}': {}",
                r.status,
                review_id
            );
        }
        _ => {}
    }

    // Parse line selection
    let selection = parse_line_selection(lines)?;

    // Get current commit for this review (use workspace for jj context)
    let jj = JjRepo::new(workspace_root);
    let commit_hash = jj
        .get_current_commit()
        .context("Failed to get current commit")?;

    // Verify file exists
    if !jj.file_exists(&commit_hash, file)? {
        bail!("File does not exist: {}", file);
    }

    let thread_id = new_thread_id();
    let author = get_agent_identity(author);

    let event = EventEnvelope::new(
        &author,
        Event::ThreadCreated(ThreadCreated {
            thread_id: thread_id.clone(),
            review_id: review_id.to_string(),
            file_path: file.to_string(),
            selection: selection.clone(),
            commit_hash: commit_hash.clone(),
        }),
    );

    let log = open_or_create(&events_path(crit_root))?;
    log.append(&event)?;

    // Output result
    let result = serde_json::json!({
        "thread_id": thread_id,
        "review_id": review_id,
        "file_path": file,
        "selection_start": selection.start_line(),
        "selection_end": selection.end_line(),
        "commit_hash": commit_hash,
        "author": author,
    });

    let formatter = Formatter::new(format);
    formatter.print(&result)?;

    Ok(())
}

/// List threads for a review with optional filters.
pub fn run_threads_list(
    repo_root: &Path,
    review_id: &str,
    status: Option<&str>,
    file: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(repo_root)?;

    let db = open_and_sync(repo_root)?;

    // Verify review exists
    if db.get_review(review_id)?.is_none() {
        bail!("Review not found: {}", review_id);
    }

    let threads = db.list_threads(review_id, status, file)?;

    // Build context-aware empty message
    let empty_msg = if status.is_some() || file.is_some() {
        "No threads match the filters"
    } else {
        "No threads yet"
    };

    let formatter = Formatter::new(format);
    formatter.print_list(&threads, empty_msg)?;

    Ok(())
}

/// Show details for a specific thread with optional context.
///
/// # Arguments
/// * `crit_root` - Path to main repo (where .crit/ lives)
/// * `workspace_root` - Path to current workspace (for jj @ resolution)
pub fn run_threads_show(
    crit_root: &Path,
    workspace_root: &Path,
    thread_id: &str,
    context_lines: u32,
    use_current: bool,
    conversation: bool,
    use_color: bool,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(crit_root)?;

    let db = open_and_sync(crit_root)?;
    let thread = db.get_thread(thread_id)?;

    match thread {
        Some(t) => {
            // If context requested, extract it (use workspace for jj context)
            let code_context = if context_lines > 0 {
                let jj = JjRepo::new(workspace_root);
                let anchor_start = t.selection_start as u32;
                let anchor_end = t.selection_end.unwrap_or(t.selection_start) as u32;

                // Use current commit or original commit based on flag
                let commit_ref = if use_current {
                    "@".to_string()
                } else {
                    t.commit_hash.clone()
                };

                match extract_context(
                    &jj,
                    &t.file_path,
                    &commit_ref,
                    anchor_start,
                    anchor_end,
                    context_lines,
                ) {
                    Ok(ctx) => Some(ctx),
                    Err(e) => {
                        // Context extraction failed, but we can still show the thread
                        eprintln!("Warning: could not extract context: {}", e);
                        None
                    }
                }
            } else {
                None
            };

            // Build output based on format
            if conversation {
                // Conversation format: human-readable with timestamps
                print_conversation(&t, code_context.as_ref(), use_color);
            } else if matches!(format, OutputFormat::Json) {
                // For JSON, include context as structured data
                let mut result = serde_json::to_value(&t)?;
                if let Some(ctx) = code_context {
                    result["code_context"] = serde_json::to_value(&ctx)?;
                }
                let formatter = Formatter::new(format);
                formatter.print(&result)?;
            } else {
                // For TOON, print thread details then context
                let formatter = Formatter::new(format);
                formatter.print(&t)?;

                if let Some(ctx) = code_context {
                    println!("\nCode context:");
                    print!("{}", format_context(&ctx));
                }
            }
        }
        None => {
            bail!("Thread not found: {}", thread_id);
        }
    }

    Ok(())
}

/// ANSI color codes for terminal output
mod colors {
    pub const RESET: &str = "\x1b[0m";
    pub const BOLD: &str = "\x1b[1m";
    pub const DIM: &str = "\x1b[2m";
    pub const GREEN: &str = "\x1b[32m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const BLUE: &str = "\x1b[34m";
    pub const MAGENTA: &str = "\x1b[35m";
    pub const CYAN: &str = "\x1b[36m";
}

/// Format and print a thread as a human-readable conversation.
fn print_conversation(
    thread: &crate::projection::ThreadDetail,
    code_context: Option<&crate::jj::context::CodeContext>,
    use_color: bool,
) {
    // Color helpers
    let c = |color: &str, text: &str| -> String {
        if use_color {
            format!("{}{}{}", color, text, colors::RESET)
        } else {
            text.to_string()
        }
    };

    let bold = |text: &str| -> String {
        if use_color {
            format!("{}{}{}", colors::BOLD, text, colors::RESET)
        } else {
            text.to_string()
        }
    };

    // Header with thread info
    let status_indicator = if thread.status == "resolved" {
        c(colors::GREEN, "[RESOLVED]")
    } else {
        c(colors::YELLOW, "[OPEN]")
    };

    let line_range = match thread.selection_end {
        Some(end) if end != thread.selection_start => {
            format!("lines {}-{}", thread.selection_start, end)
        }
        _ => format!("line {}", thread.selection_start),
    };

    println!(
        "{} {} on {} ({})",
        bold(&format!("Thread {}", thread.thread_id)),
        status_indicator,
        c(colors::CYAN, &thread.file_path),
        c(colors::DIM, &line_range)
    );
    println!("{}", "=".repeat(60));

    // Show code context if available
    if let Some(ctx) = code_context {
        println!();
        print!("{}", format_context(ctx));
        println!("{}", "-".repeat(60));
    }

    // Thread creation
    println!(
        "\n{} started this thread ({})",
        c(colors::BLUE, &thread.author),
        c(colors::DIM, &format_timestamp(&thread.created_at))
    );

    // Comments as conversation
    for comment in &thread.comments {
        println!();
        println!(
            "{} ({})",
            c(colors::MAGENTA, &comment.author),
            c(colors::DIM, &format_timestamp(&comment.created_at))
        );
        // Indent the body for readability
        for line in comment.body.lines() {
            println!("  {}", line);
        }
    }

    // Status changes
    if thread.status == "resolved" {
        if let Some(ref changed_by) = thread.status_changed_by {
            println!();
            let timestamp = thread
                .status_changed_at
                .as_deref()
                .map(format_timestamp)
                .unwrap_or_else(|| "unknown time".to_string());
            print!(
                "{} {} ({})",
                c(colors::GREEN, changed_by),
                c(colors::GREEN, "resolved this thread"),
                c(colors::DIM, &timestamp)
            );
            if let Some(ref reason) = thread.resolve_reason {
                print!(": {}", reason);
            }
            println!();
        }
    }

    println!();
}

/// Format an ISO timestamp to a more readable form.
fn format_timestamp(iso_timestamp: &str) -> String {
    // Parse ISO 8601 format: 2026-01-25T12:34:56.789Z or 2026-01-25T12:34:56Z
    // Return a more readable format: "Jan 25, 12:34"
    if let Some(datetime_part) = iso_timestamp.split('T').nth(1) {
        if let Some(time_part) = datetime_part.split('.').next() {
            // Get just HH:MM
            let time_short = time_part.split(':').take(2).collect::<Vec<_>>().join(":");
            // Get the date part
            if let Some(date_part) = iso_timestamp.split('T').next() {
                let parts: Vec<&str> = date_part.split('-').collect();
                if parts.len() == 3 {
                    let month = match parts[1] {
                        "01" => "Jan",
                        "02" => "Feb",
                        "03" => "Mar",
                        "04" => "Apr",
                        "05" => "May",
                        "06" => "Jun",
                        "07" => "Jul",
                        "08" => "Aug",
                        "09" => "Sep",
                        "10" => "Oct",
                        "11" => "Nov",
                        "12" => "Dec",
                        _ => parts[1],
                    };
                    let day = parts[2].trim_start_matches('0');
                    return format!("{} {}, {}", month, day, time_short);
                }
            }
        }
    }
    // Fallback: return as-is
    iso_timestamp.to_string()
}

/// Resolve a thread (or all threads matching criteria).
/// Supports batch resolve: pass multiple thread IDs to resolve them all at once.
pub fn run_threads_resolve(
    repo_root: &Path,
    thread_ids: &[String],
    all: bool,
    file: Option<&str>,
    reason: Option<String>,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(repo_root)?;

    if !all && thread_ids.is_empty() {
        bail!("Either specify thread_id(s) or use --all");
    }

    if all && !thread_ids.is_empty() {
        bail!("Cannot specify both thread_id(s) and --all");
    }

    let db = open_and_sync(repo_root)?;
    let author = get_agent_identity(author);
    let log = open_or_create(&events_path(repo_root))?;

    let mut resolved_count = 0;
    let mut resolved_ids = Vec::new();

    if all {
        // Resolve all open threads, optionally filtered by file
        // The CLI doesn't pass review_id to resolve, so --all resolves across all reviews.

        // Get all threads and filter to open ones
        let all_reviews = db.list_reviews(None, None)?;
        for review in all_reviews {
            let threads = db.list_threads(&review.review_id, Some("open"), file)?;
            for thread in threads {
                let event = EventEnvelope::new(
                    &author,
                    Event::ThreadResolved(ThreadResolved {
                        thread_id: thread.thread_id.clone(),
                        reason: reason.clone(),
                    }),
                );
                log.append(&event)?;
                resolved_ids.push(thread.thread_id);
                resolved_count += 1;
            }
        }
    } else {
        // Resolve one or more threads by ID
        for tid in thread_ids {
            let thread = db.get_thread(tid)?;
            match &thread {
                None => bail!("Thread not found: {}", tid),
                Some(t) if t.status == "resolved" => {
                    bail!("Thread is already resolved: {}", tid);
                }
                _ => {}
            }

            let event = EventEnvelope::new(
                &author,
                Event::ThreadResolved(ThreadResolved {
                    thread_id: tid.to_string(),
                    reason: reason.clone(),
                }),
            );
            log.append(&event)?;
            resolved_ids.push(tid.to_string());
            resolved_count += 1;
        }
    }

    let result = serde_json::json!({
        "resolved_count": resolved_count,
        "thread_ids": resolved_ids,
        "reason": reason,
    });

    let formatter = Formatter::new(format);
    formatter.print(&result)?;

    Ok(())
}

/// Reopen a resolved thread.
pub fn run_threads_reopen(
    repo_root: &Path,
    thread_id: &str,
    reason: Option<String>,
    author: Option<&str>,
    format: OutputFormat,
) -> Result<()> {
    ensure_initialized(repo_root)?;

    let db = open_and_sync(repo_root)?;
    let thread = db.get_thread(thread_id)?;

    match &thread {
        None => bail!("Thread not found: {}", thread_id),
        Some(t) if t.status != "resolved" => {
            bail!(
                "Cannot reopen thread with status '{}': {}",
                t.status,
                thread_id
            );
        }
        _ => {}
    }

    let author = get_agent_identity(author);
    let event = EventEnvelope::new(
        &author,
        Event::ThreadReopened(ThreadReopened {
            thread_id: thread_id.to_string(),
            reason: reason.clone(),
        }),
    );

    let log = open_or_create(&events_path(repo_root))?;
    log.append(&event)?;

    let result = serde_json::json!({
        "thread_id": thread_id,
        "status": "open",
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

/// Parse a line selection string like "42" or "10-20".
pub fn parse_line_selection(lines: &str) -> Result<CodeSelection> {
    if lines.contains('-') {
        let parts: Vec<&str> = lines.split('-').collect();
        if parts.len() != 2 {
            bail!(
                "Invalid line range format: '{}'. Expected 'start-end'",
                lines
            );
        }
        let start: u32 = parts[0]
            .trim()
            .parse()
            .with_context(|| format!("Invalid start line: '{}'", parts[0]))?;
        let end: u32 = parts[1]
            .trim()
            .parse()
            .with_context(|| format!("Invalid end line: '{}'", parts[1]))?;

        if start == 0 || end == 0 {
            bail!("Line numbers must be 1-based");
        }
        if start > end {
            bail!("Start line ({}) must be <= end line ({})", start, end);
        }

        Ok(CodeSelection::range(start, end))
    } else {
        let line: u32 = lines
            .trim()
            .parse()
            .with_context(|| format!("Invalid line number: '{}'", lines))?;

        if line == 0 {
            bail!("Line numbers must be 1-based");
        }

        Ok(CodeSelection::line(line))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_line_selection_single() {
        let sel = parse_line_selection("42").unwrap();
        assert_eq!(sel.start_line(), 42);
        assert_eq!(sel.end_line(), 42);
    }

    #[test]
    fn test_parse_line_selection_range() {
        let sel = parse_line_selection("10-20").unwrap();
        assert_eq!(sel.start_line(), 10);
        assert_eq!(sel.end_line(), 20);
    }

    #[test]
    fn test_parse_line_selection_range_with_spaces() {
        let sel = parse_line_selection("10 - 20").unwrap();
        assert_eq!(sel.start_line(), 10);
        assert_eq!(sel.end_line(), 20);
    }

    #[test]
    fn test_parse_line_selection_invalid_zero() {
        assert!(parse_line_selection("0").is_err());
        assert!(parse_line_selection("0-10").is_err());
        assert!(parse_line_selection("10-0").is_err());
    }

    #[test]
    fn test_parse_line_selection_invalid_range() {
        assert!(parse_line_selection("20-10").is_err());
    }

    #[test]
    fn test_parse_line_selection_invalid_format() {
        assert!(parse_line_selection("abc").is_err());
        assert!(parse_line_selection("10-20-30").is_err());
        assert!(parse_line_selection("").is_err());
    }
}
