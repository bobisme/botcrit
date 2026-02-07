//! Projection engine for botcrit.
//!
//! Projects events from the append-only log into a queryable database.
//! The database is ephemeral and can be rebuilt from the event log at any time.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::doc_markdown)]

mod query;

pub use query::{
    Comment, InboxSummary, OpenThreadOnMyReview, ReviewAwaitingVote, ReviewDetail, ReviewSummary,
    ReviewerVote, ThreadDetail, ThreadSummary, ThreadWithNewResponses,
};

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::events::{
    CodeSelection, CommentAdded, Event, EventEnvelope, ReviewAbandoned, ReviewApproved,
    ReviewCreated, ReviewMerged, ReviewerVoted, ReviewersRequested, ThreadCreated, ThreadReopened,
    ThreadResolved,
};
use crate::log::{read_all_reviews, AppendLog};

/// Database for projected state from events.
pub struct ProjectionDb {
    conn: Connection,
}

impl ProjectionDb {
    /// Open or create a projection database at the given path.
    ///
    /// Creates parent directories if they don't exist.
    pub fn open(path: &Path) -> Result<Self> {
        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create parent directories: {}", parent.display())
                })?;
            }
        }

        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open database: {}", path.display()))?;

        // Enable foreign keys for integrity (optional, see schema notes)
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .context("Failed to enable foreign keys")?;

        Ok(Self { conn })
    }

    /// Create an in-memory projection database.
    ///
    /// Used for temporary merged projections (e.g., `inbox --all-workspaces`)
    /// and in tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("Failed to open in-memory database")?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .context("Failed to enable foreign keys")?;
        Ok(Self { conn })
    }

    /// Initialize the database schema.
    ///
    /// Creates all tables, indexes, and views if they don't exist.
    /// Also runs any necessary migrations for schema changes.
    pub fn init_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(SCHEMA_SQL)
            .context("Failed to initialize schema")?;
        self.migrate_schema()?;
        Ok(())
    }

    /// Run schema migrations for any changes since the database was created.
    ///
    /// SQLite's CREATE TABLE IF NOT EXISTS doesn't add new columns to existing
    /// tables. This migration adds any columns that were added after the initial
    /// schema was created.
    fn migrate_schema(&self) -> Result<()> {
        // Check if next_comment_number column exists in threads table
        let has_column: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('threads') WHERE name = 'next_comment_number'",
                [],
                |row| row.get(0),
            )
            .context("Failed to check for next_comment_number column")?;

        if !has_column {
            self.conn
                .execute(
                    "ALTER TABLE threads ADD COLUMN next_comment_number INTEGER NOT NULL DEFAULT 1",
                    [],
                )
                .context("Failed to add next_comment_number column to threads")?;
        }

        Ok(())
    }

    /// Get the last successfully processed line number from the event log.
    ///
    /// Returns 0 if no events have been processed yet.
    pub fn get_last_sync_line(&self) -> Result<usize> {
        let line: Option<i64> = self
            .conn
            .query_row(
                "SELECT last_line_number FROM sync_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to query sync_state")?;

        Ok(line.map_or(0, |l| l as usize))
    }

    /// Get the stored content hash of the event log prefix.
    ///
    /// Returns `None` if no hash has been stored yet.
    pub fn get_events_file_hash(&self) -> Result<Option<String>> {
        let hash: Option<String> = self
            .conn
            .query_row(
                "SELECT events_file_hash FROM sync_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to query events_file_hash")?
            .flatten();

        Ok(hash)
    }

    /// Update the last successfully processed line number.
    pub fn set_last_sync_line(&self, line: usize) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn
            .execute(
                "UPDATE sync_state SET last_line_number = ?, last_sync_ts = ? WHERE id = 1",
                params![line as i64, now],
            )
            .context("Failed to update sync_state")?;
        Ok(())
    }

    /// Get a reference to the underlying connection (for advanced queries).
    #[must_use]
    pub const fn conn(&self) -> &Connection {
        &self.conn
    }
}

/// Sync the projection database from the event log.
///
/// Reads events starting from the last processed line and applies them
/// to the database. Returns the number of events processed.
///
/// Detects file replacement (e.g., from jj working copy restoration) using
/// two checks:
/// 1. **Truncation**: `last_sync_line > total_lines()` — file got shorter.
/// 2. **Content hash**: prefix hash of lines 0..last_sync_line changed —
///    file was replaced with same-or-longer content but different history.
///
/// Either triggers a full rebuild from scratch.
pub fn sync_from_log(db: &ProjectionDb, log: &impl AppendLog) -> Result<usize> {
    sync_from_log_with_backup(db, log, None)
}

/// Sync the projection database with optional orphan backup on truncation.
///
/// When `crit_dir` is provided and truncation/mismatch is detected, saves
/// orphaned review IDs to `.crit/orphaned-reviews-{timestamp}.json` before
/// rebuilding. This allows recovery of lost reviews from jj workspace history.
pub fn sync_from_log_with_backup(
    db: &ProjectionDb,
    log: &impl AppendLog,
    crit_dir: Option<&Path>,
) -> Result<usize> {
    let last_line = db.get_last_sync_line()?;

    if last_line > 0 {
        let total = log.total_lines()?;

        // Check 1: Truncation — file has fewer lines than our sync cursor.
        if last_line > total {
            eprintln!(
                "WARNING: events.jsonl truncated (expected >={} lines, found {}). Rebuilding projection.",
                last_line, total
            );
            return rebuild_projection_with_orphan_detection(db, log, crit_dir);
        }

        // Check 2: Content hash — the prefix we already processed changed.
        let stored_hash = db.get_events_file_hash()?;
        if let Some(ref expected) = stored_hash {
            if let Some(ref actual) = log.prefix_hash(last_line)? {
                if expected != actual {
                    eprintln!(
                        "WARNING: events.jsonl content changed (hash mismatch at line {}). Rebuilding projection.",
                        last_line
                    );
                    return rebuild_projection_with_orphan_detection(db, log, crit_dir);
                }
            }
        }
    }

    let events = log.read_from(last_line)?;

    if events.is_empty() {
        // Even if no new events, store hash if we don't have one yet
        // (backfill for databases created before hash tracking).
        if last_line > 0 && db.get_events_file_hash()?.is_none() {
            if let Some(hash) = log.prefix_hash(last_line)? {
                db.conn
                    .execute(
                        "UPDATE sync_state SET events_file_hash = ? WHERE id = 1",
                        params![hash],
                    )
                    .context("Failed to backfill events_file_hash")?;
            }
        }
        return Ok(0);
    }

    let count = events.len();
    let new_line = last_line + count;

    // Compute new prefix hash covering all processed lines
    let new_hash = log.prefix_hash(new_line)?;

    // Process events in a transaction for atomicity
    let tx = db
        .conn
        .unchecked_transaction()
        .context("Failed to begin transaction")?;

    for event in &events {
        apply_event_inner(&tx, event).with_context(|| {
            format!(
                "Failed to apply event at line {} (type: {:?})",
                last_line,
                event_type_name(&event.event)
            )
        })?;
    }

    // Update sync state to point past all processed events
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "UPDATE sync_state SET last_line_number = ?, last_sync_ts = ?, events_file_hash = ? WHERE id = 1",
        params![new_line as i64, now, new_hash],
    )
    .context("Failed to update sync_state")?;

    tx.commit().context("Failed to commit transaction")?;

    Ok(count)
}

/// Rebuild the projection from scratch by wiping all data and re-applying
/// all events from the log.
///
/// Called when file replacement is detected — either truncation
/// (last_sync_line > total file lines) or content hash mismatch
/// (same line count but different content).
#[allow(dead_code)]
fn rebuild_projection(db: &ProjectionDb, log: &impl AppendLog) -> Result<usize> {
    let events = log.read_all()?;
    let count = events.len();

    // Compute hash of the full file for the new sync state
    let new_hash = log.prefix_hash(count)?;

    let tx = db
        .conn
        .unchecked_transaction()
        .context("Failed to begin rebuild transaction")?;

    // Wipe all projection data (order matters for foreign keys)
    tx.execute_batch(
        "DELETE FROM comments;
         DELETE FROM threads;
         DELETE FROM reviewer_votes;
         DELETE FROM review_reviewers;
         DELETE FROM reviews;",
    )
    .context("Failed to wipe projection tables")?;

    // Re-apply all events
    for event in &events {
        apply_event_inner(&tx, event)
            .with_context(|| format!("Failed to apply event during rebuild: {:?}", event_type_name(&event.event)))?;
    }

    // Update sync state with line count and content hash
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "UPDATE sync_state SET last_line_number = ?, last_sync_ts = ?, events_file_hash = ? WHERE id = 1",
        params![count as i64, now, new_hash],
    )
    .context("Failed to update sync_state after rebuild")?;

    tx.commit().context("Failed to commit rebuild")?;

    Ok(count)
}

/// Rebuild projection with orphan detection and backup.
///
/// Before wiping the projection, extracts current review IDs from the database.
/// After rebuilding from the new file, identifies which reviews were lost (orphaned)
/// and writes them to a timestamped backup file in the crit directory.
fn rebuild_projection_with_orphan_detection(
    db: &ProjectionDb,
    log: &impl AppendLog,
    crit_dir: Option<&Path>,
) -> Result<usize> {
    // Step 1: Capture current review IDs before rebuild
    let old_review_ids: Vec<String> = db
        .conn
        .prepare("SELECT review_id FROM reviews")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("Failed to query existing review IDs")?;

    // Step 2: Read new events and compute hash
    let events = log.read_all()?;
    let count = events.len();
    let new_hash = log.prefix_hash(count)?;

    // Step 3: Find which review IDs will exist after rebuild
    let new_review_ids: std::collections::HashSet<String> = events
        .iter()
        .filter_map(|e| match &e.event {
            Event::ReviewCreated(rc) => Some(rc.review_id.clone()),
            _ => None,
        })
        .collect();

    // Step 4: Identify orphaned reviews (in old DB but not in new file)
    let orphaned: Vec<&String> = old_review_ids
        .iter()
        .filter(|id| !new_review_ids.contains(*id))
        .collect();

    // Step 5: If orphans exist and crit_dir provided, write backup
    if !orphaned.is_empty() {
        eprintln!(
            "WARNING: {} review(s) will be lost: {}",
            orphaned.len(),
            orphaned
                .iter()
                .take(5)
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        if orphaned.len() > 5 {
            eprintln!("  ... and {} more", orphaned.len() - 5);
        }

        if let Some(dir) = crit_dir {
            let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
            let backup_path = dir.join(format!("orphaned-reviews-{}.json", timestamp));

            // Gather detailed info about orphaned reviews
            let mut orphan_details: Vec<serde_json::Value> = Vec::new();
            for id in &orphaned {
                let detail: Option<(String, String, String)> = db
                    .conn
                    .query_row(
                        "SELECT title, author, status FROM reviews WHERE review_id = ?",
                        params![id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()?;

                if let Some((title, author, status)) = detail {
                    orphan_details.push(serde_json::json!({
                        "review_id": id,
                        "title": title,
                        "author": author,
                        "status": status,
                    }));
                }
            }

            let backup = serde_json::json!({
                "timestamp": Utc::now().to_rfc3339(),
                "reason": "events.jsonl truncation or content mismatch detected",
                "orphaned_reviews": orphan_details,
                "recovery_hint": "These reviews were in index.db but not in the restored events.jsonl. Check jj history for older versions of events.jsonl using: jj file annotate .crit/events.jsonl"
            });

            std::fs::write(&backup_path, serde_json::to_string_pretty(&backup)?)?;
            eprintln!("Orphaned review details saved to: {}", backup_path.display());
        } else {
            eprintln!(
                "HINT: Run 'jj file annotate .crit/events.jsonl' to find older versions"
            );
        }
    }

    // Step 6: Proceed with normal rebuild
    let tx = db
        .conn
        .unchecked_transaction()
        .context("Failed to begin rebuild transaction")?;

    tx.execute_batch(
        "DELETE FROM comments;
         DELETE FROM threads;
         DELETE FROM reviewer_votes;
         DELETE FROM review_reviewers;
         DELETE FROM reviews;",
    )
    .context("Failed to wipe projection tables")?;

    for event in &events {
        apply_event_inner(&tx, event)
            .with_context(|| format!("Failed to apply event during rebuild: {:?}", event_type_name(&event.event)))?;
    }

    let now = Utc::now().to_rfc3339();
    tx.execute(
        "UPDATE sync_state SET last_line_number = ?, last_sync_ts = ?, events_file_hash = ? WHERE id = 1",
        params![count as i64, now, new_hash],
    )
    .context("Failed to update sync_state after rebuild")?;

    tx.commit().context("Failed to commit rebuild")?;

    Ok(count)
}

// ============================================================================
// Orphaned event detection (bd-2ys)
// ============================================================================

/// Extract review_id from an event, if it directly carries one.
fn event_review_id(event: &Event) -> Option<&str> {
    match event {
        Event::ReviewCreated(e) => Some(&e.review_id),
        Event::ReviewersRequested(e) => Some(&e.review_id),
        Event::ReviewerVoted(e) => Some(&e.review_id),
        Event::ReviewApproved(e) => Some(&e.review_id),
        Event::ReviewMerged(e) => Some(&e.review_id),
        Event::ReviewAbandoned(e) => Some(&e.review_id),
        Event::ThreadCreated(e) => Some(&e.review_id),
        // These only carry thread_id:
        Event::ThreadResolved(_) | Event::ThreadReopened(_) | Event::CommentAdded(_) => None,
    }
}

/// Extract thread_id from an event, if it carries one.
fn event_thread_id(event: &Event) -> Option<&str> {
    match event {
        Event::ThreadCreated(e) => Some(&e.thread_id),
        Event::ThreadResolved(e) => Some(&e.thread_id),
        Event::ThreadReopened(e) => Some(&e.thread_id),
        Event::CommentAdded(e) => Some(&e.thread_id),
        _ => None,
    }
}

/// Filter out orphaned events that reference reviews without a ReviewCreated event.
///
/// When a workspace is destroyed after review creation, the ReviewCreated event
/// may be lost while ThreadCreated/CommentAdded events remain. This function
/// detects and removes such orphaned events, printing a warning.
///
/// `extra_known_reviews` allows callers to supply review IDs known to exist in
/// other workspaces, preventing false-positive orphan detection when syncing
/// cross-workspace events (e.g., `inbox --all-workspaces`).
///
/// Returns the filtered events and the count of skipped events.
fn filter_orphaned_events(
    events: Vec<EventEnvelope>,
    extra_known_reviews: Option<&HashSet<String>>,
) -> (Vec<EventEnvelope>, usize) {
    // Pass 1: collect known review_ids (from ReviewCreated) and thread→review map
    let mut known_reviews: HashSet<String> = HashSet::new();
    let mut thread_to_review: HashMap<String, String> = HashMap::new();

    // Include review IDs known from other workspaces
    if let Some(extra) = extra_known_reviews {
        known_reviews.extend(extra.iter().cloned());
    }

    for env in &events {
        if let Event::ReviewCreated(e) = &env.event {
            known_reviews.insert(e.review_id.clone());
        }
        if let Event::ThreadCreated(e) = &env.event {
            thread_to_review.insert(e.thread_id.clone(), e.review_id.clone());
        }
    }

    // Pass 2: identify orphaned review_ids
    let mut orphaned_reviews: HashSet<String> = HashSet::new();
    for env in &events {
        // Check events that carry review_id directly
        if let Some(rid) = event_review_id(&env.event) {
            if !known_reviews.contains(rid) {
                orphaned_reviews.insert(rid.to_string());
            }
        }
        // Check thread-only events via thread→review map
        if event_review_id(&env.event).is_none() {
            if let Some(tid) = event_thread_id(&env.event) {
                if let Some(rid) = thread_to_review.get(tid) {
                    if !known_reviews.contains(rid.as_str()) {
                        orphaned_reviews.insert(rid.clone());
                    }
                }
                // If thread_id not in map at all, that thread's ThreadCreated
                // is also missing — it will be caught as an FK error on threads
                // table, so also treat it as orphaned.
                if !thread_to_review.contains_key(tid) {
                    orphaned_reviews.insert(format!("unknown-thread:{tid}"));
                }
            }
        }
    }

    if orphaned_reviews.is_empty() {
        return (events, 0);
    }

    // Warn about orphaned reviews
    let real_orphans: Vec<&String> = orphaned_reviews
        .iter()
        .filter(|r| !r.starts_with("unknown-thread:"))
        .collect();
    if !real_orphans.is_empty() {
        eprintln!(
            "WARNING: {} review(s) have events but no ReviewCreated — skipping orphaned events",
            real_orphans.len()
        );
        for rid in &real_orphans {
            eprintln!("  {rid}");
        }
        eprintln!("  To find lost reviews: crit reviews list --all-workspaces");
    }

    // Pass 3: filter out orphaned events
    let mut skipped = 0;
    let filtered: Vec<EventEnvelope> = events
        .into_iter()
        .filter(|env| {
            // Check direct review_id
            if let Some(rid) = event_review_id(&env.event) {
                if orphaned_reviews.contains(rid) {
                    skipped += 1;
                    return false;
                }
            }
            // Check thread-only events
            if event_review_id(&env.event).is_none() {
                if let Some(tid) = event_thread_id(&env.event) {
                    let is_orphan = match thread_to_review.get(tid) {
                        Some(rid) => orphaned_reviews.contains(rid.as_str()),
                        None => orphaned_reviews.contains(&format!("unknown-thread:{tid}")),
                    };
                    if is_orphan {
                        skipped += 1;
                        return false;
                    }
                }
            }
            true
        })
        .collect();

    (filtered, skipped)
}

// ============================================================================
// v2: Per-review event log sync
// ============================================================================

/// Sync the projection from per-review event logs (v2 format).
///
/// In v2, each review has its own event log at `.crit/reviews/{review_id}/events.jsonl`.
/// This function reads all events from all review logs and applies them.
///
/// The sync strategy is timestamp-based:
/// 1. Read `last_sync_ts` from sync_state
/// 2. Read all events from all review logs
/// 3. Filter to events with `ts > last_sync_ts`
/// 4. Apply events in timestamp order
///
/// If no events have been synced yet (new database), applies all events.
pub fn sync_from_review_logs(db: &ProjectionDb, crit_root: &Path) -> Result<usize> {
    sync_from_review_logs_inner(db, crit_root, None)
}

/// Sync from review logs, with extra known review IDs from other workspaces.
///
/// Used by `inbox --all-workspaces` to prevent false-positive orphan detection
/// when a review's ReviewCreated lives in a different workspace than the
/// ReviewersRequested/ReviewerVoted events being synced.
pub fn sync_from_review_logs_with_known(
    db: &ProjectionDb,
    crit_root: &Path,
    extra_known_reviews: &HashSet<String>,
) -> Result<usize> {
    sync_from_review_logs_inner(db, crit_root, Some(extra_known_reviews))
}

fn sync_from_review_logs_inner(
    db: &ProjectionDb,
    crit_root: &Path,
    extra_known_reviews: Option<&HashSet<String>>,
) -> Result<usize> {
    // Get last sync timestamp
    let last_sync_ts: Option<String> = db
        .conn
        .query_row("SELECT last_sync_ts FROM sync_state WHERE id = 1", [], |row| {
            row.get(0)
        })
        .optional()
        .context("Failed to query last_sync_ts")?
        .flatten();

    let last_ts: Option<DateTime<Utc>> = last_sync_ts
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    // Read all events from all review logs
    let all_events = read_all_reviews(crit_root)?;

    // Filter to events newer than last sync
    let new_events: Vec<_> = if let Some(ts) = last_ts {
        all_events.into_iter().filter(|e| e.ts > ts).collect()
    } else {
        all_events
    };

    if new_events.is_empty() {
        return Ok(0);
    }

    // Filter out orphaned events (bd-2ys)
    let (new_events, orphaned_count) = filter_orphaned_events(new_events, extra_known_reviews);
    if new_events.is_empty() {
        return Ok(orphaned_count);
    }

    let count = new_events.len();

    // Find the latest timestamp for sync state
    let max_ts = new_events
        .iter()
        .map(|e| &e.ts)
        .max()
        .expect("new_events is not empty");

    // Process events in a transaction
    let tx = db
        .conn
        .unchecked_transaction()
        .context("Failed to begin transaction")?;

    for event in &new_events {
        apply_event_inner(&tx, event).with_context(|| {
            format!(
                "Failed to apply event (type: {:?})",
                event_type_name(&event.event)
            )
        })?;
    }

    // Update sync state
    tx.execute(
        "UPDATE sync_state SET last_sync_ts = ? WHERE id = 1",
        params![max_ts.to_rfc3339()],
    )
    .context("Failed to update sync_state")?;

    tx.commit().context("Failed to commit transaction")?;

    Ok(count + orphaned_count)
}

/// Rebuild the projection from per-review event logs (v2 format).
///
/// Wipes all data and re-applies all events from all review logs.
pub fn rebuild_from_review_logs(db: &ProjectionDb, crit_root: &Path) -> Result<usize> {
    // Read all events from all review logs
    let all_events = read_all_reviews(crit_root)?;

    if all_events.is_empty() {
        return Ok(0);
    }

    // Filter out orphaned events (bd-2ys)
    let (all_events, _orphaned_count) = filter_orphaned_events(all_events, None);

    if all_events.is_empty() {
        return Ok(0);
    }

    // Find the latest timestamp for sync state
    let max_ts = all_events
        .iter()
        .map(|e| &e.ts)
        .max()
        .expect("all_events is not empty");

    let tx = db
        .conn
        .unchecked_transaction()
        .context("Failed to begin rebuild transaction")?;

    // Wipe all projection data (order matters for foreign keys)
    tx.execute_batch(
        "DELETE FROM comments;
         DELETE FROM threads;
         DELETE FROM reviewer_votes;
         DELETE FROM review_reviewers;
         DELETE FROM reviews;",
    )
    .context("Failed to wipe projection tables")?;

    // Apply all events
    for event in &all_events {
        apply_event_inner(&tx, event).with_context(|| {
            format!(
                "Failed to apply event during rebuild (type: {:?})",
                event_type_name(&event.event)
            )
        })?;
    }

    // Update sync state
    tx.execute(
        "UPDATE sync_state SET last_line_number = 0, last_sync_ts = ?, events_file_hash = NULL WHERE id = 1",
        params![max_ts.to_rfc3339()],
    )
    .context("Failed to update sync_state after rebuild")?;

    tx.commit().context("Failed to commit rebuild")?;

    Ok(all_events.len())
}

/// Build a projection from pre-collected events (no file I/O).
///
/// Used by `inbox --all-workspaces` to merge events from multiple workspaces
/// into a single in-memory DB. Events should be sorted by timestamp.
/// Orphan filtering is applied to the merged set, so cross-workspace events
/// (e.g., ReviewCreated in workspace A, ReviewersRequested in root) are
/// handled correctly.
pub fn rebuild_from_events(db: &ProjectionDb, events: Vec<EventEnvelope>) -> Result<usize> {
    if events.is_empty() {
        return Ok(0);
    }

    let (events, _orphaned_count) = filter_orphaned_events(events, None);
    if events.is_empty() {
        return Ok(0);
    }

    let max_ts = events
        .iter()
        .map(|e| &e.ts)
        .max()
        .expect("events is not empty");

    let tx = db
        .conn
        .unchecked_transaction()
        .context("Failed to begin transaction")?;

    for event in &events {
        apply_event_inner(&tx, event).with_context(|| {
            format!(
                "Failed to apply event (type: {:?})",
                event_type_name(&event.event)
            )
        })?;
    }

    tx.execute(
        "UPDATE sync_state SET last_sync_ts = ? WHERE id = 1",
        params![max_ts.to_rfc3339()],
    )
    .context("Failed to update sync_state")?;

    tx.commit().context("Failed to commit")?;

    Ok(events.len())
}

/// Apply a single event to the projection database.
pub fn apply_event(db: &ProjectionDb, event: &EventEnvelope) -> Result<()> {
    apply_event_inner(&db.conn, event)
}

/// Internal event application using a generic connection/transaction.
fn apply_event_inner(conn: &Connection, envelope: &EventEnvelope) -> Result<()> {
    let ts = &envelope.ts;
    let author = &envelope.author;

    match &envelope.event {
        Event::ReviewCreated(e) => apply_review_created(conn, e, author, ts),
        Event::ReviewersRequested(e) => apply_reviewers_requested(conn, e, author, ts),
        Event::ReviewerVoted(e) => apply_reviewer_voted(conn, e, author, ts),
        Event::ReviewApproved(e) => apply_review_approved(conn, e, author, ts),
        Event::ReviewMerged(e) => apply_review_merged(conn, e, author, ts),
        Event::ReviewAbandoned(e) => apply_review_abandoned(conn, e, author, ts),
        Event::ThreadCreated(e) => apply_thread_created(conn, e, author, ts),
        Event::ThreadResolved(e) => apply_thread_resolved(conn, e, author, ts),
        Event::ThreadReopened(e) => apply_thread_reopened(conn, e, author, ts),
        Event::CommentAdded(e) => apply_comment_added(conn, e, author, ts),
    }
}

// ============================================================================
// Review Event Handlers
// ============================================================================

fn apply_review_created(
    conn: &Connection,
    event: &ReviewCreated,
    author: &str,
    ts: &DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO reviews (
            review_id, jj_change_id, initial_commit, title, description,
            author, created_at, status
        ) VALUES (?, ?, ?, ?, ?, ?, ?, 'open')",
        params![
            event.review_id,
            event.jj_change_id,
            event.initial_commit,
            event.title,
            event.description,
            author,
            ts.to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn apply_reviewers_requested(
    conn: &Connection,
    event: &ReviewersRequested,
    author: &str,
    ts: &DateTime<Utc>,
) -> Result<()> {
    let ts_str = ts.to_rfc3339();
    for reviewer in &event.reviewers {
        conn.execute(
            "INSERT INTO review_reviewers (
                review_id, reviewer, requested_at, requested_by
            ) VALUES (?, ?, ?, ?)
            ON CONFLICT (review_id, reviewer) DO UPDATE SET
                requested_at = excluded.requested_at,
                requested_by = excluded.requested_by",
            params![event.review_id, reviewer, ts_str, author],
        )?;
    }
    Ok(())
}

fn apply_reviewer_voted(
    conn: &Connection,
    event: &ReviewerVoted,
    author: &str,
    ts: &DateTime<Utc>,
) -> Result<()> {
    // Insert or replace vote (a reviewer can change their vote)
    conn.execute(
        "INSERT INTO reviewer_votes (review_id, reviewer, vote, reason, voted_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (review_id, reviewer) DO UPDATE SET
             vote = excluded.vote,
             reason = excluded.reason,
             voted_at = excluded.voted_at",
        params![
            event.review_id,
            author,
            event.vote.to_string(),
            event.reason,
            ts.to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn apply_review_approved(
    conn: &Connection,
    event: &ReviewApproved,
    author: &str,
    ts: &DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "UPDATE reviews SET
            status = 'approved',
            status_changed_at = ?,
            status_changed_by = ?
        WHERE review_id = ? AND status = 'open'",
        params![ts.to_rfc3339(), author, event.review_id],
    )?;
    Ok(())
}

fn apply_review_merged(
    conn: &Connection,
    event: &ReviewMerged,
    author: &str,
    ts: &DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "UPDATE reviews SET
            status = 'merged',
            final_commit = ?,
            status_changed_at = ?,
            status_changed_by = ?
        WHERE review_id = ? AND status IN ('open', 'approved')",
        params![event.final_commit, ts.to_rfc3339(), author, event.review_id],
    )?;
    Ok(())
}

fn apply_review_abandoned(
    conn: &Connection,
    event: &ReviewAbandoned,
    author: &str,
    ts: &DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "UPDATE reviews SET
            status = 'abandoned',
            status_changed_at = ?,
            status_changed_by = ?,
            abandon_reason = ?
        WHERE review_id = ? AND status IN ('open', 'approved')",
        params![ts.to_rfc3339(), author, event.reason, event.review_id],
    )?;
    Ok(())
}

// ============================================================================
// Thread Event Handlers
// ============================================================================

fn apply_thread_created(
    conn: &Connection,
    event: &ThreadCreated,
    author: &str,
    ts: &DateTime<Utc>,
) -> Result<()> {
    let (selection_type, selection_start, selection_end) = match &event.selection {
        CodeSelection::Line { line } => ("line", *line, None),
        CodeSelection::Range { start, end } => ("range", *start, Some(*end)),
    };

    conn.execute(
        "INSERT OR IGNORE INTO threads (
            thread_id, review_id, file_path,
            selection_type, selection_start, selection_end,
            commit_hash, author, created_at, status
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'open')",
        params![
            event.thread_id,
            event.review_id,
            event.file_path,
            selection_type,
            selection_start,
            selection_end,
            event.commit_hash,
            author,
            ts.to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn apply_thread_resolved(
    conn: &Connection,
    event: &ThreadResolved,
    author: &str,
    ts: &DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "UPDATE threads SET
            status = 'resolved',
            status_changed_at = ?,
            status_changed_by = ?,
            resolve_reason = ?
        WHERE thread_id = ? AND status = 'open'",
        params![ts.to_rfc3339(), author, event.reason, event.thread_id],
    )?;
    Ok(())
}

fn apply_thread_reopened(
    conn: &Connection,
    event: &ThreadReopened,
    author: &str,
    ts: &DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "UPDATE threads SET
            status = 'open',
            status_changed_at = ?,
            status_changed_by = ?,
            reopen_reason = ?
        WHERE thread_id = ? AND status = 'resolved'",
        params![ts.to_rfc3339(), author, event.reason, event.thread_id],
    )?;
    Ok(())
}

// ============================================================================
// Comment Event Handlers
// ============================================================================

fn apply_comment_added(
    conn: &Connection,
    event: &CommentAdded,
    author: &str,
    ts: &DateTime<Utc>,
) -> Result<()> {
    // Insert the comment
    conn.execute(
        "INSERT OR IGNORE INTO comments (
            comment_id, thread_id, body, author, created_at
        ) VALUES (?, ?, ?, ?, ?)",
        params![
            event.comment_id,
            event.thread_id,
            event.body,
            author,
            ts.to_rfc3339(),
        ],
    )?;
    // Increment the thread's next_comment_number for future comments
    conn.execute(
        "UPDATE threads SET next_comment_number = next_comment_number + 1 WHERE thread_id = ?",
        params![event.thread_id],
    )?;
    Ok(())
}

// ============================================================================
// Helpers
// ============================================================================

const fn event_type_name(event: &Event) -> &'static str {
    match event {
        Event::ReviewCreated(_) => "ReviewCreated",
        Event::ReviewersRequested(_) => "ReviewersRequested",
        Event::ReviewerVoted(_) => "ReviewerVoted",
        Event::ReviewApproved(_) => "ReviewApproved",
        Event::ReviewMerged(_) => "ReviewMerged",
        Event::ReviewAbandoned(_) => "ReviewAbandoned",
        Event::ThreadCreated(_) => "ThreadCreated",
        Event::ThreadResolved(_) => "ThreadResolved",
        Event::ThreadReopened(_) => "ThreadReopened",
        Event::CommentAdded(_) => "CommentAdded",
    }
}

// ============================================================================
// Schema SQL
// ============================================================================

const SCHEMA_SQL: &str = r"
-- SYNC STATE
CREATE TABLE IF NOT EXISTS sync_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_line_number INTEGER NOT NULL DEFAULT 0,
    last_sync_ts TEXT,
    events_file_hash TEXT
);

INSERT OR IGNORE INTO sync_state (id, last_line_number) VALUES (1, 0);

-- REVIEWS
CREATE TABLE IF NOT EXISTS reviews (
    review_id TEXT PRIMARY KEY,
    jj_change_id TEXT NOT NULL,
    initial_commit TEXT NOT NULL,
    final_commit TEXT,
    title TEXT NOT NULL,
    description TEXT,
    author TEXT NOT NULL,
    created_at TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'approved', 'merged', 'abandoned')),
    status_changed_at TEXT,
    status_changed_by TEXT,
    abandon_reason TEXT
);

CREATE INDEX IF NOT EXISTS idx_reviews_status ON reviews(status);
CREATE INDEX IF NOT EXISTS idx_reviews_author ON reviews(author);
CREATE INDEX IF NOT EXISTS idx_reviews_change_id ON reviews(jj_change_id);

-- REVIEWERS
CREATE TABLE IF NOT EXISTS review_reviewers (
    review_id TEXT NOT NULL REFERENCES reviews(review_id),
    reviewer TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    requested_by TEXT NOT NULL,
    PRIMARY KEY (review_id, reviewer)
);

CREATE INDEX IF NOT EXISTS idx_reviewers_reviewer ON review_reviewers(reviewer);

-- REVIEWER VOTES
CREATE TABLE IF NOT EXISTS reviewer_votes (
    review_id TEXT NOT NULL REFERENCES reviews(review_id),
    reviewer TEXT NOT NULL,
    vote TEXT NOT NULL CHECK (vote IN ('lgtm', 'block')),
    reason TEXT,
    voted_at TEXT NOT NULL,
    PRIMARY KEY (review_id, reviewer)
);

CREATE INDEX IF NOT EXISTS idx_votes_review ON reviewer_votes(review_id);
CREATE INDEX IF NOT EXISTS idx_votes_vote ON reviewer_votes(vote);

-- THREADS
CREATE TABLE IF NOT EXISTS threads (
    thread_id TEXT PRIMARY KEY,
    review_id TEXT NOT NULL REFERENCES reviews(review_id),
    file_path TEXT NOT NULL,
    selection_type TEXT NOT NULL CHECK (selection_type IN ('line', 'range')),
    selection_start INTEGER NOT NULL,
    selection_end INTEGER,
    commit_hash TEXT NOT NULL,
    author TEXT NOT NULL,
    created_at TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'resolved')),
    status_changed_at TEXT,
    status_changed_by TEXT,
    resolve_reason TEXT,
    reopen_reason TEXT,
    next_comment_number INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX IF NOT EXISTS idx_threads_review_id ON threads(review_id);
CREATE INDEX IF NOT EXISTS idx_threads_status ON threads(status);
CREATE INDEX IF NOT EXISTS idx_threads_review_file ON threads(review_id, file_path);
CREATE INDEX IF NOT EXISTS idx_threads_review_status ON threads(review_id, status);

-- COMMENTS
CREATE TABLE IF NOT EXISTS comments (
    comment_id TEXT PRIMARY KEY,
    thread_id TEXT NOT NULL REFERENCES threads(thread_id),
    body TEXT NOT NULL,
    author TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_comments_thread_id ON comments(thread_id);

-- VIEWS
-- Note: open_thread_count only counts threads that are truly actionable.
-- Threads on merged/abandoned reviews are NOT counted as open, even if
-- they were never explicitly resolved (they're effectively resolved by
-- the review being completed).
CREATE VIEW IF NOT EXISTS v_reviews_summary AS
SELECT
    r.review_id,
    r.title,
    r.author,
    r.status,
    r.jj_change_id,
    r.created_at,
    COUNT(DISTINCT t.thread_id) AS thread_count,
    COUNT(DISTINCT CASE
        WHEN t.status = 'open' AND r.status NOT IN ('merged', 'abandoned')
        THEN t.thread_id
    END) AS open_thread_count
FROM reviews r
LEFT JOIN threads t ON t.review_id = r.review_id
GROUP BY r.review_id;

-- v_threads_detail includes an effective_status that considers parent review state.
-- If the review is merged/abandoned, threads are effectively 'resolved' even if
-- they were never explicitly resolved.
CREATE VIEW IF NOT EXISTS v_threads_detail AS
SELECT
    t.*,
    r.title AS review_title,
    r.status AS review_status,
    COUNT(c.comment_id) AS comment_count,
    CASE
        WHEN t.status = 'resolved' THEN 'resolved'
        WHEN r.status IN ('merged', 'abandoned') THEN 'resolved'
        ELSE 'open'
    END AS effective_status
FROM threads t
JOIN reviews r ON r.review_id = t.review_id
LEFT JOIN comments c ON c.thread_id = t.thread_id
GROUP BY t.thread_id;
";

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{CodeSelection, Event};
    use crate::log::{open_or_create, AppendLog};
    use tempfile::tempdir;

    fn make_review_created(review_id: &str) -> EventEnvelope {
        EventEnvelope::new(
            "test_author",
            Event::ReviewCreated(ReviewCreated {
                review_id: review_id.to_string(),
                jj_change_id: "change123".to_string(),
                initial_commit: "commit456".to_string(),
                title: format!("Review {review_id}"),
                description: Some("Test description".to_string()),
            }),
        )
    }

    fn make_thread_created(thread_id: &str, review_id: &str) -> EventEnvelope {
        EventEnvelope::new(
            "test_author",
            Event::ThreadCreated(ThreadCreated {
                thread_id: thread_id.to_string(),
                review_id: review_id.to_string(),
                file_path: "src/main.rs".to_string(),
                selection: CodeSelection::range(10, 20),
                commit_hash: "abc123".to_string(),
            }),
        )
    }

    #[test]
    fn test_open_and_init_schema() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let db = ProjectionDb::open(&db_path).unwrap();
        db.init_schema().unwrap();

        // Verify sync_state was initialized
        let line = db.get_last_sync_line().unwrap();
        assert_eq!(line, 0);
    }

    #[test]
    fn test_sync_state_roundtrip() {
        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        assert_eq!(db.get_last_sync_line().unwrap(), 0);

        db.set_last_sync_line(42).unwrap();
        assert_eq!(db.get_last_sync_line().unwrap(), 42);

        db.set_last_sync_line(100).unwrap();
        assert_eq!(db.get_last_sync_line().unwrap(), 100);
    }

    #[test]
    fn test_apply_review_created() {
        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        let event = make_review_created("cr-001");
        apply_event(&db, &event).unwrap();

        // Verify review was inserted
        let (title, status): (String, String) = db
            .conn()
            .query_row(
                "SELECT title, status FROM reviews WHERE review_id = ?",
                params!["cr-001"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(title, "Review cr-001");
        assert_eq!(status, "open");
    }

    #[test]
    fn test_apply_review_lifecycle() {
        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        // Create review
        apply_event(&db, &make_review_created("cr-001")).unwrap();

        // Request reviewers
        apply_event(
            &db,
            &EventEnvelope::new(
                "requester",
                Event::ReviewersRequested(ReviewersRequested {
                    review_id: "cr-001".to_string(),
                    reviewers: vec!["alice".to_string(), "bob".to_string()],
                }),
            ),
        )
        .unwrap();

        // Verify reviewers
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM review_reviewers WHERE review_id = ?",
                params!["cr-001"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);

        // Approve review
        apply_event(
            &db,
            &EventEnvelope::new(
                "alice",
                Event::ReviewApproved(ReviewApproved {
                    review_id: "cr-001".to_string(),
                }),
            ),
        )
        .unwrap();

        let status: String = db
            .conn()
            .query_row(
                "SELECT status FROM reviews WHERE review_id = ?",
                params!["cr-001"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "approved");

        // Merge review
        apply_event(
            &db,
            &EventEnvelope::new(
                "merger",
                Event::ReviewMerged(ReviewMerged {
                    review_id: "cr-001".to_string(),
                    final_commit: "final789".to_string(),
                }),
            ),
        )
        .unwrap();

        let (status, final_commit): (String, String) = db
            .conn()
            .query_row(
                "SELECT status, final_commit FROM reviews WHERE review_id = ?",
                params!["cr-001"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "merged");
        assert_eq!(final_commit, "final789");
    }

    #[test]
    fn test_apply_review_abandoned() {
        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        apply_event(&db, &make_review_created("cr-001")).unwrap();

        apply_event(
            &db,
            &EventEnvelope::new(
                "abandoner",
                Event::ReviewAbandoned(ReviewAbandoned {
                    review_id: "cr-001".to_string(),
                    reason: Some("No longer needed".to_string()),
                }),
            ),
        )
        .unwrap();

        let (status, reason): (String, Option<String>) = db
            .conn()
            .query_row(
                "SELECT status, abandon_reason FROM reviews WHERE review_id = ?",
                params!["cr-001"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "abandoned");
        assert_eq!(reason, Some("No longer needed".to_string()));
    }

    #[test]
    fn test_apply_thread_lifecycle() {
        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        // Create review first (for FK)
        apply_event(&db, &make_review_created("cr-001")).unwrap();

        // Create thread
        apply_event(&db, &make_thread_created("th-001", "cr-001")).unwrap();

        let (status, sel_type, sel_start, sel_end): (String, String, i64, Option<i64>) = db
            .conn()
            .query_row(
                "SELECT status, selection_type, selection_start, selection_end 
                 FROM threads WHERE thread_id = ?",
                params!["th-001"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(status, "open");
        assert_eq!(sel_type, "range");
        assert_eq!(sel_start, 10);
        assert_eq!(sel_end, Some(20));

        // Resolve thread
        apply_event(
            &db,
            &EventEnvelope::new(
                "resolver",
                Event::ThreadResolved(ThreadResolved {
                    thread_id: "th-001".to_string(),
                    reason: Some("Fixed".to_string()),
                }),
            ),
        )
        .unwrap();

        let status: String = db
            .conn()
            .query_row(
                "SELECT status FROM threads WHERE thread_id = ?",
                params!["th-001"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "resolved");

        // Reopen thread
        apply_event(
            &db,
            &EventEnvelope::new(
                "reopener",
                Event::ThreadReopened(ThreadReopened {
                    thread_id: "th-001".to_string(),
                    reason: Some("Not actually fixed".to_string()),
                }),
            ),
        )
        .unwrap();

        let status: String = db
            .conn()
            .query_row(
                "SELECT status FROM threads WHERE thread_id = ?",
                params!["th-001"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "open");
    }

    #[test]
    fn test_sync_from_log_empty() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");
        let log = open_or_create(&log_path).unwrap();

        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        let count = sync_from_log(&db, &log).unwrap();
        assert_eq!(count, 0);
        assert_eq!(db.get_last_sync_line().unwrap(), 0);
    }

    #[test]
    fn test_sync_from_log_full() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");
        let log = open_or_create(&log_path).unwrap();

        // Add events to log
        log.append(&make_review_created("cr-001")).unwrap();
        log.append(&make_review_created("cr-002")).unwrap();

        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        // First sync
        let count = sync_from_log(&db, &log).unwrap();
        assert_eq!(count, 2);
        assert_eq!(db.get_last_sync_line().unwrap(), 2);

        // Verify reviews exist
        let review_count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM reviews", [], |row| row.get(0))
            .unwrap();
        assert_eq!(review_count, 2);

        // Second sync (no new events)
        let count = sync_from_log(&db, &log).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_sync_from_log_incremental() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");
        let log = open_or_create(&log_path).unwrap();

        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        // Add first batch
        log.append(&make_review_created("cr-001")).unwrap();
        let count = sync_from_log(&db, &log).unwrap();
        assert_eq!(count, 1);
        assert_eq!(db.get_last_sync_line().unwrap(), 1);

        // Add second batch
        log.append(&make_review_created("cr-002")).unwrap();
        log.append(&make_review_created("cr-003")).unwrap();
        let count = sync_from_log(&db, &log).unwrap();
        assert_eq!(count, 2);
        assert_eq!(db.get_last_sync_line().unwrap(), 3);

        // Verify all reviews
        let review_count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM reviews", [], |row| row.get(0))
            .unwrap();
        assert_eq!(review_count, 3);
    }

    #[test]
    fn test_sync_with_file_persistence() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");
        let db_path = dir.path().join("index.db");

        let log = open_or_create(&log_path).unwrap();
        log.append(&make_review_created("cr-001")).unwrap();
        log.append(&make_review_created("cr-002")).unwrap();

        // First sync
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let count = sync_from_log(&db, &log).unwrap();
            assert_eq!(count, 2);
        }

        // Add more events
        log.append(&make_review_created("cr-003")).unwrap();

        // Reopen database and sync
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            // Schema already exists, init is idempotent
            db.init_schema().unwrap();

            // Should only sync new event
            let count = sync_from_log(&db, &log).unwrap();
            assert_eq!(count, 1);
            assert_eq!(db.get_last_sync_line().unwrap(), 3);

            let review_count: i64 = db
                .conn()
                .query_row("SELECT COUNT(*) FROM reviews", [], |row| row.get(0))
                .unwrap();
            assert_eq!(review_count, 3);
        }
    }

    #[test]
    fn test_idempotent_event_application() {
        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        let event = make_review_created("cr-001");

        // Apply same event twice (simulates replay)
        apply_event(&db, &event).unwrap();
        apply_event(&db, &event).unwrap();

        // Should only have one review (INSERT OR IGNORE)
        let count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM reviews", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_thread_single_line_selection() {
        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        apply_event(&db, &make_review_created("cr-001")).unwrap();

        let event = EventEnvelope::new(
            "test_author",
            Event::ThreadCreated(ThreadCreated {
                thread_id: "th-001".to_string(),
                review_id: "cr-001".to_string(),
                file_path: "src/lib.rs".to_string(),
                selection: CodeSelection::line(42),
                commit_hash: "abc123".to_string(),
            }),
        );
        apply_event(&db, &event).unwrap();

        let (sel_type, sel_start, sel_end): (String, i64, Option<i64>) = db
            .conn()
            .query_row(
                "SELECT selection_type, selection_start, selection_end 
                 FROM threads WHERE thread_id = ?",
                params!["th-001"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(sel_type, "line");
        assert_eq!(sel_start, 42);
        assert_eq!(sel_end, None);
    }

    #[test]
    fn test_views_work() {
        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        apply_event(&db, &make_review_created("cr-001")).unwrap();
        apply_event(&db, &make_thread_created("th-001", "cr-001")).unwrap();
        apply_event(&db, &make_thread_created("th-002", "cr-001")).unwrap();

        // Resolve one thread
        apply_event(
            &db,
            &EventEnvelope::new(
                "resolver",
                Event::ThreadResolved(ThreadResolved {
                    thread_id: "th-001".to_string(),
                    reason: None,
                }),
            ),
        )
        .unwrap();

        // Query the summary view
        let (thread_count, open_count): (i64, i64) = db
            .conn()
            .query_row(
                "SELECT thread_count, open_thread_count FROM v_reviews_summary WHERE review_id = ?",
                params!["cr-001"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(thread_count, 2);
        assert_eq!(open_count, 1);
    }

    // ========================================================================
    // bd-1s1 reproduction tests: LGTM vote doesn't override block in index
    // ========================================================================
    //
    // These tests simulate the exact CLI command sync pattern from the R4 eval
    // to reproduce the reported bug where `crit lgtm` succeeded but
    // `crit review` still showed a blocking vote.
    //
    // Hypotheses tested:
    // 1. Empty lines in events.jsonl cause sync offset drift
    // 2. Incremental sync with on-disk DB persistence loses vote state
    // 3. The eval's exact event sequence triggers the bug

    use crate::events::{ReviewerVoted, ReviewersRequested, VoteType};
    use std::io::Write;

    /// Helper: query the current vote for a reviewer on a review.
    fn query_vote(db: &ProjectionDb, review_id: &str, reviewer: &str) -> Option<String> {
        db.conn()
            .query_row(
                "SELECT vote FROM reviewer_votes WHERE review_id = ? AND reviewer = ?",
                params![review_id, reviewer],
                |row| row.get(0),
            )
            .optional()
            .unwrap()
    }

    /// Helper: write raw string content to a file (no FileLog, exact bytes).
    fn write_raw(path: &std::path::Path, content: &str) {
        std::fs::write(path, content).unwrap();
    }

    /// Helper: append raw string content to a file.
    fn append_raw(path: &std::path::Path, content: &str) {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file.flush().unwrap();
    }

    /// Helper: make a block vote event.
    fn make_block_vote(review_id: &str, reviewer: &str, reason: &str) -> EventEnvelope {
        EventEnvelope::new(
            reviewer,
            Event::ReviewerVoted(ReviewerVoted {
                review_id: review_id.to_string(),
                vote: VoteType::Block,
                reason: Some(reason.to_string()),
            }),
        )
    }

    /// Helper: make an lgtm vote event.
    fn make_lgtm_vote(review_id: &str, reviewer: &str) -> EventEnvelope {
        EventEnvelope::new(
            reviewer,
            Event::ReviewerVoted(ReviewerVoted {
                review_id: review_id.to_string(),
                vote: VoteType::Lgtm,
                reason: Some("Looks good".to_string()),
            }),
        )
    }

    /// Baseline: incremental sync with block → lgtm votes, no empty lines.
    /// Simulates: crit block, then crit lgtm, then crit review.
    #[test]
    fn test_bd_1s1_baseline_incremental_vote_override() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");
        let log = open_or_create(&log_path).unwrap();

        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        // Command 1: crit reviews create
        log.append(&make_review_created("cr-001")).unwrap();
        sync_from_log(&db, &log).unwrap();

        // Command 2: crit block
        log.append(&make_block_vote("cr-001", "reviewer-a", "Needs fixes")).unwrap();
        sync_from_log(&db, &log).unwrap();
        assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), Some("block".to_string()));
        assert!(db.has_blocking_votes("cr-001").unwrap());

        // Command 3: crit lgtm
        log.append(&make_lgtm_vote("cr-001", "reviewer-a")).unwrap();
        // The lgtm command does NOT re-sync; the NEXT command does.

        // Command 4: crit review (syncs the lgtm event)
        sync_from_log(&db, &log).unwrap();
        assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), Some("lgtm".to_string()));
        assert!(!db.has_blocking_votes("cr-001").unwrap());
    }

    /// Test sync offset drift when empty line exists between block and lgtm.
    /// Verifies that empty lines cause re-processing but correct final state.
    #[test]
    fn test_bd_1s1_empty_line_between_votes() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");
        let log = open_or_create(&log_path).unwrap();

        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        // Write review + block vote normally
        log.append(&make_review_created("cr-001")).unwrap();
        log.append(&make_block_vote("cr-001", "reviewer-a", "Needs fixes")).unwrap();
        sync_from_log(&db, &log).unwrap();
        assert_eq!(db.get_last_sync_line().unwrap(), 2);
        assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), Some("block".to_string()));

        // Inject empty line, then lgtm vote (bypassing FileLog::append)
        append_raw(&log_path, "\n"); // empty line at idx 2
        let lgtm = make_lgtm_vote("cr-001", "reviewer-a");
        let lgtm_json = lgtm.to_json_line().unwrap();
        append_raw(&log_path, &format!("{}\n", lgtm_json)); // lgtm at idx 3

        // Sync: should pick up lgtm despite empty line
        let count = sync_from_log(&db, &log).unwrap();
        assert_eq!(count, 1, "Should process exactly 1 new event (lgtm)");
        assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), Some("lgtm".to_string()));
        assert!(!db.has_blocking_votes("cr-001").unwrap());

        // Check for sync offset drift: last_sync should be 3 (not 4)
        // because empty line was skipped in count but occupies a file line
        let sync_line = db.get_last_sync_line().unwrap();
        let actual_lines: usize = std::fs::read_to_string(&log_path)
            .unwrap()
            .lines()
            .count();
        // Drift detected: sync_line < actual_lines
        assert_eq!(sync_line, 3, "Sync offset should be 3 (drift: missed the empty line)");
        assert_eq!(actual_lines, 4, "File should have 4 lines (including empty)");

        // Subsequent sync: re-reads lgtm due to drift (harmless)
        let count = sync_from_log(&db, &log).unwrap();
        assert_eq!(count, 1, "Re-processes lgtm due to offset drift");
        assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), Some("lgtm".to_string()));
        assert!(!db.has_blocking_votes("cr-001").unwrap());
    }

    /// Test with trailing empty line (matches eval data: 20 events + empty line 21).
    /// Trailing empty lines do NOT cause drift because they're at the end:
    /// read_from(N) skips them and returns empty, so last_sync stays correct.
    #[test]
    fn test_bd_1s1_trailing_empty_line() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");
        let log = open_or_create(&log_path).unwrap();

        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        // Write review + block + lgtm normally
        log.append(&make_review_created("cr-001")).unwrap();
        log.append(&make_block_vote("cr-001", "reviewer-a", "Needs fixes")).unwrap();
        log.append(&make_lgtm_vote("cr-001", "reviewer-a")).unwrap();

        // Add trailing empty line (as seen in eval data)
        append_raw(&log_path, "\n");

        // Full sync from scratch
        let count = sync_from_log(&db, &log).unwrap();
        assert_eq!(count, 3);
        assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), Some("lgtm".to_string()));
        assert!(!db.has_blocking_votes("cr-001").unwrap());

        // Sync line is 3 — this is correct because events 0,1,2 were processed
        let sync_line = db.get_last_sync_line().unwrap();
        assert_eq!(sync_line, 3);

        // Trailing empty line does NOT cause re-processing:
        // read_from(3) hits idx 3 (empty) → skip → no events → count=0
        let count = sync_from_log(&db, &log).unwrap();
        assert_eq!(count, 0, "Trailing empty line should NOT cause re-processing");
        assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), Some("lgtm".to_string()));
    }

    /// Test with on-disk DB persistence (closer to real CLI behavior).
    /// Each "command" opens a fresh DB connection, syncs, then closes.
    #[test]
    fn test_bd_1s1_ondisk_persistence_vote_override() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");
        let db_path = dir.path().join("index.db");
        let log = open_or_create(&log_path).unwrap();

        // Command 1: create review (opens fresh DB)
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            // No events yet, nothing to sync
            sync_from_log(&db, &log).unwrap();
        }
        log.append(&make_review_created("cr-001")).unwrap();

        // Command 2: block vote
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            sync_from_log(&db, &log).unwrap(); // syncs ReviewCreated
        }
        log.append(&make_block_vote("cr-001", "reviewer-a", "Needs fixes")).unwrap();

        // Command 3: lgtm vote (syncs block, then writes lgtm)
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            sync_from_log(&db, &log).unwrap(); // syncs block vote
            // At this point, DB shows block
            assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), Some("block".to_string()));
        }
        log.append(&make_lgtm_vote("cr-001", "reviewer-a")).unwrap();

        // Command 4: crit review (syncs lgtm, then reads)
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            sync_from_log(&db, &log).unwrap(); // syncs lgtm vote
            // Should show lgtm
            assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), Some("lgtm".to_string()));
            assert!(!db.has_blocking_votes("cr-001").unwrap());
        }
    }

    /// Simulate the EXACT R4 eval pattern using raw event JSON lines.
    /// Uses the actual event data from /tmp/tmp.iyprC50GHo/.crit/events.jsonl.
    #[test]
    fn test_bd_1s1_eval_data_incremental_sync() {
        // Raw event lines from R4 eval (actual JSON from events.jsonl)
        let events: Vec<&str> = vec![
            r#"{"ts":"2026-01-31T17:18:32.592763788Z","author":"mystic-birch","event":"ReviewCreated","data":{"review_id":"cr-fjf9","jj_change_id":"lsrxqntnrzlyulstznuytqollqwpmnur","initial_commit":"ab7ef0a9cf8f1d01e7cd3c635429ba7195cf5e73","title":"feat: add GET /files/:name endpoint","description":"Adds file serving endpoint that reads from ./data"}}"#,
            r#"{"ts":"2026-01-31T17:18:36.109469869Z","author":"mystic-birch","event":"ReviewersRequested","data":{"review_id":"cr-fjf9","reviewers":["jasper-lattice"]}}"#,
            r#"{"ts":"2026-01-31T17:20:01.874064552Z","author":"jasper-lattice","event":"ThreadCreated","data":{"thread_id":"th-ooz8","review_id":"cr-fjf9","file_path":"src/main.rs","selection":{"type":"Line","line":22},"commit_hash":"517b3f84fcdba505957a74f79316ed5338911600"}}"#,
            r#"{"ts":"2026-01-31T17:20:01.874131748Z","author":"jasper-lattice","event":"CommentAdded","data":{"comment_id":"c-m9xz","thread_id":"th-ooz8","body":"CRITICAL: Path traversal vulnerability."}}"#,
            r#"{"ts":"2026-01-31T17:20:07.776996959Z","author":"jasper-lattice","event":"ThreadCreated","data":{"thread_id":"th-fj4b","review_id":"cr-fjf9","file_path":"src/main.rs","selection":{"type":"Line","line":24},"commit_hash":"e4c6aff98c3a5b4c133576d14818136f9d99b966"}}"#,
            r#"{"ts":"2026-01-31T17:20:07.777046382Z","author":"jasper-lattice","event":"CommentAdded","data":{"comment_id":"c-80oc","thread_id":"th-fj4b","body":"HIGH: Using synchronous filesystem I/O in async handler."}}"#,
            r#"{"ts":"2026-01-31T17:20:13.705333684Z","author":"jasper-lattice","event":"CommentAdded","data":{"comment_id":"c-ltji","thread_id":"th-fj4b","body":"HIGH: Unbounded memory consumption."}}"#,
            r#"{"ts":"2026-01-31T17:20:19.305357862Z","author":"jasper-lattice","event":"ThreadCreated","data":{"thread_id":"th-azov","review_id":"cr-fjf9","file_path":"src/main.rs","selection":{"type":"Line","line":29},"commit_hash":"c851779e0f7efbe3a9e3c2d6575c9c4d645eba37"}}"#,
            r#"{"ts":"2026-01-31T17:20:19.305407595Z","author":"jasper-lattice","event":"CommentAdded","data":{"comment_id":"c-3zdn","thread_id":"th-azov","body":"MEDIUM: Missing error information."}}"#,
            r#"{"ts":"2026-01-31T17:20:24.988565444Z","author":"jasper-lattice","event":"ThreadCreated","data":{"thread_id":"th-xgby","review_id":"cr-fjf9","file_path":"src/main.rs","selection":{"type":"Line","line":16},"commit_hash":"2dc3be9e8140dcc8b18b8aead907ca88a0a9bf0f"}}"#,
            r#"{"ts":"2026-01-31T17:20:24.988629895Z","author":"jasper-lattice","event":"CommentAdded","data":{"comment_id":"c-c8z0","thread_id":"th-xgby","body":"MEDIUM: Using unwrap() on production server startup."}}"#,
            r#"{"ts":"2026-01-31T17:20:31.713659068Z","author":"jasper-lattice","event":"ThreadCreated","data":{"thread_id":"th-a8ha","review_id":"cr-fjf9","file_path":"src/main.rs","selection":{"type":"Line","line":14},"commit_hash":"7240b65595aabcfc731f594b31fb37458c5ab58a"}}"#,
            r#"{"ts":"2026-01-31T17:20:31.713722667Z","author":"jasper-lattice","event":"CommentAdded","data":{"comment_id":"c-xla3","thread_id":"th-a8ha","body":"LOW: Binding to 0.0.0.0 exposes service to all network interfaces."}}"#,
            r#"{"ts":"2026-01-31T17:20:36.558747769Z","author":"jasper-lattice","event":"ReviewerVoted","data":{"review_id":"cr-fjf9","vote":"block","reason":"CRITICAL path traversal vulnerability allows reading arbitrary files."}}"#,
            r#"{"ts":"2026-01-31T17:27:06.693854670Z","author":"jasper-lattice","event":"ReviewerVoted","data":{"review_id":"cr-fjf9","vote":"lgtm","reason":"All issues resolved."}}"#,
            r#"{"ts":"2026-01-31T17:27:30.337341106Z","author":"jasper-lattice","event":"ReviewerVoted","data":{"review_id":"cr-fjf9","vote":"lgtm"}}"#,
            r#"{"ts":"2026-01-31T17:27:39.741734997Z","author":"jasper-lattice","event":"ReviewerVoted","data":{"review_id":"cr-fjf9","vote":"lgtm","reason":"All security issues resolved"}}"#,
            r#"{"ts":"2026-01-31T17:28:03.984462867Z","author":"jasper-lattice","event":"ReviewerVoted","data":{"review_id":"cr-fjf9","vote":"lgtm"}}"#,
            r#"{"ts":"2026-01-31T17:28:15.551617462Z","author":"jasper-lattice","event":"ReviewApproved","data":{"review_id":"cr-fjf9"}}"#,
            r#"{"ts":"2026-01-31T17:29:25.525546004Z","author":"mystic-birch","event":"ReviewMerged","data":{"review_id":"cr-fjf9","final_commit":"72f4e86c4ab0ca0c48150a6c26bfeeeb4e6b9d98"}}"#,
        ];

        // Simulate CLI command batches (which events are written per command):
        // Each tuple: (events_written_this_batch, description)
        let batches: Vec<(usize, &str)> = vec![
            (1, "crit reviews create"),             // event 0
            (1, "crit reviews request-reviewers"),   // event 1
            (2, "crit comment (thread+comment)"),    // events 2-3
            (2, "crit comment (thread+comment)"),    // events 4-5
            (1, "crit reply"),                       // event 6
            (2, "crit comment (thread+comment)"),    // events 7-8
            (2, "crit comment (thread+comment)"),    // events 9-10
            (2, "crit comment (thread+comment)"),    // events 11-12
            (1, "crit block"),                       // event 13
            (1, "crit lgtm (attempt 1)"),            // event 14
            (1, "crit lgtm (attempt 2)"),            // event 15
            (1, "crit lgtm (attempt 3)"),            // event 16
            (1, "crit lgtm (attempt 4)"),            // event 17
            (1, "crit reviews approve"),             // event 18
            (1, "crit reviews mark-merged"),         // event 19
        ];

        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");
        let db_path = dir.path().join("index.db");

        // Create empty events file
        std::fs::write(&log_path, "").unwrap();

        let mut event_idx = 0;
        for (batch_size, desc) in &batches {
            // Each CLI command: open DB, sync, close DB
            {
                let db = ProjectionDb::open(&db_path).unwrap();
                db.init_schema().unwrap();
                let log = crate::log::FileLog::new(&log_path);
                sync_from_log(&db, &log).unwrap();

                // After syncing block vote, check state
                if *desc == "crit lgtm (attempt 1)" {
                    // DB should have block vote at this point
                    assert_eq!(
                        query_vote(&db, "cr-fjf9", "jasper-lattice"),
                        Some("block".to_string()),
                        "Before first lgtm write, DB should show block"
                    );
                }

                // After syncing first lgtm, check state
                if *desc == "crit lgtm (attempt 2)" {
                    // DB should have lgtm vote (from first lgtm at line 14)
                    let vote = query_vote(&db, "cr-fjf9", "jasper-lattice");
                    assert_eq!(
                        vote,
                        Some("lgtm".to_string()),
                        "After syncing first lgtm, DB should show lgtm (was: {:?}) [{}]",
                        vote,
                        desc
                    );
                    assert!(
                        !db.has_blocking_votes("cr-fjf9").unwrap(),
                        "No blocking votes after lgtm synced"
                    );
                }
            }

            // Then write this batch's events to the log
            for _ in 0..*batch_size {
                append_raw(&log_path, &format!("{}\n", events[event_idx]));
                event_idx += 1;
            }
        }

        // Final check: open DB, sync remaining events, verify merged state
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            sync_from_log(&db, &log).unwrap();

            assert_eq!(
                query_vote(&db, "cr-fjf9", "jasper-lattice"),
                Some("lgtm".to_string()),
                "Final state should be lgtm"
            );
            assert!(!db.has_blocking_votes("cr-fjf9").unwrap());

            // Verify review was merged
            let status: String = db
                .conn()
                .query_row(
                    "SELECT status FROM reviews WHERE review_id = ?",
                    params!["cr-fjf9"],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(status, "merged");
        }
    }

    /// Same as eval data test but with trailing empty line (as found in eval).
    /// Trailing empty lines do NOT cause drift — only middle empty lines do.
    #[test]
    fn test_bd_1s1_eval_data_with_trailing_empty_line() {
        let events: Vec<&str> = vec![
            r#"{"ts":"2026-01-31T17:18:32.592763788Z","author":"mystic-birch","event":"ReviewCreated","data":{"review_id":"cr-fjf9","jj_change_id":"lsrxqntnrzlyulstznuytqollqwpmnur","initial_commit":"ab7ef0a9cf8f1d01e7cd3c635429ba7195cf5e73","title":"feat: add GET /files/:name endpoint","description":"Adds file serving endpoint that reads from ./data"}}"#,
            r#"{"ts":"2026-01-31T17:18:36.109469869Z","author":"mystic-birch","event":"ReviewersRequested","data":{"review_id":"cr-fjf9","reviewers":["jasper-lattice"]}}"#,
            r#"{"ts":"2026-01-31T17:20:36.558747769Z","author":"jasper-lattice","event":"ReviewerVoted","data":{"review_id":"cr-fjf9","vote":"block","reason":"CRITICAL vulnerability"}}"#,
            r#"{"ts":"2026-01-31T17:27:06.693854670Z","author":"jasper-lattice","event":"ReviewerVoted","data":{"review_id":"cr-fjf9","vote":"lgtm","reason":"All issues resolved."}}"#,
        ];

        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");

        // Write all events + trailing empty line (matches eval file)
        let mut content = String::new();
        for event in &events {
            content.push_str(event);
            content.push('\n');
        }
        content.push('\n'); // trailing empty line
        write_raw(&log_path, &content);

        // Full sync from fresh DB
        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();
        let log = crate::log::FileLog::new(&log_path);

        let count = sync_from_log(&db, &log).unwrap();
        assert_eq!(count, 4, "Should process 4 events (skip empty line)");

        assert_eq!(
            query_vote(&db, "cr-fjf9", "jasper-lattice"),
            Some("lgtm".to_string()),
            "Full sync should show lgtm"
        );
        assert!(!db.has_blocking_votes("cr-fjf9").unwrap());

        // Sync line is 4 (correct: 4 events processed)
        assert_eq!(
            db.get_last_sync_line().unwrap(),
            4,
            "Sync line should be 4"
        );

        // Trailing empty line does NOT cause re-processing
        let count = sync_from_log(&db, &log).unwrap();
        assert_eq!(count, 0, "Trailing empty line should NOT cause re-processing");
        assert_eq!(
            query_vote(&db, "cr-fjf9", "jasper-lattice"),
            Some("lgtm".to_string()),
            "Vote should still be lgtm after re-sync"
        );
    }

    /// Test that multiple empty lines cause proportionally more drift.
    /// With enough empty lines, sync could re-read the block vote.
    #[test]
    fn test_bd_1s1_multiple_empty_lines_drift() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");

        let review = make_review_created("cr-001");
        let block = make_block_vote("cr-001", "reviewer-a", "Needs fixes");
        let lgtm = make_lgtm_vote("cr-001", "reviewer-a");

        // Write: review, empty, empty, block, empty, empty, empty, lgtm
        let mut content = String::new();
        content.push_str(&review.to_json_line().unwrap());
        content.push('\n');
        content.push('\n'); // empty at idx 1
        content.push('\n'); // empty at idx 2
        content.push_str(&block.to_json_line().unwrap());
        content.push('\n'); // block at idx 3
        content.push('\n'); // empty at idx 4
        content.push('\n'); // empty at idx 5
        content.push('\n'); // empty at idx 6
        content.push_str(&lgtm.to_json_line().unwrap());
        content.push('\n'); // lgtm at idx 7
        write_raw(&log_path, &content);

        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();
        let log = crate::log::FileLog::new(&log_path);

        // Full sync: processes review, block, lgtm (3 events from 8 lines)
        let count = sync_from_log(&db, &log).unwrap();
        assert_eq!(count, 3, "Should process 3 events, skipping 5 empty lines");

        assert_eq!(
            query_vote(&db, "cr-001", "reviewer-a"),
            Some("lgtm".to_string()),
            "Full sync should show lgtm"
        );

        // Drift: last_sync = 3 but file has 8 lines
        let sync_line = db.get_last_sync_line().unwrap();
        assert_eq!(sync_line, 3, "Major drift: sync at 3, file has 8 lines");

        // Re-sync: reads from idx 3 (block), processes block + lgtm
        let count = sync_from_log(&db, &log).unwrap();
        assert!(count > 0, "Drift causes re-processing");

        // CRITICAL: does the re-processing cause vote regression?
        // The block at idx 3 is re-read, but lgtm at idx 7 is also re-read.
        // Since they're in the same batch, lgtm overwrites block.
        assert_eq!(
            query_vote(&db, "cr-001", "reviewer-a"),
            Some("lgtm".to_string()),
            "Vote should remain lgtm despite block re-processing (same batch)"
        );

        // Keep re-syncing until stable
        let mut iterations = 0;
        loop {
            let count = sync_from_log(&db, &log).unwrap();
            if count == 0 {
                break;
            }
            iterations += 1;
            assert!(iterations < 10, "Should converge, not loop forever");
            assert_eq!(
                query_vote(&db, "cr-001", "reviewer-a"),
                Some("lgtm".to_string()),
                "Vote must remain lgtm on iteration {iterations}"
            );
        }

        // Final sync line should eventually reach 8
        assert_eq!(
            db.get_last_sync_line().unwrap(),
            8,
            "Should eventually converge to correct offset"
        );
    }

    /// Test the dangerous scenario: block is re-processed in isolation
    /// (without lgtm in the same batch) due to empty lines.
    ///
    /// This tests whether progressive drift accumulation can cause the block
    /// to be replayed WITHOUT the subsequent lgtm, which would regress the vote.
    #[test]
    fn test_bd_1s1_progressive_drift_vote_regression() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");
        let db_path = dir.path().join("index.db");

        let review = make_review_created("cr-001");
        let block = make_block_vote("cr-001", "reviewer-a", "Needs fixes");
        let lgtm = make_lgtm_vote("cr-001", "reviewer-a");

        // Step 1: Write review + block, sync
        write_raw(&log_path, "");
        append_raw(&log_path, &format!("{}\n", review.to_json_line().unwrap()));
        append_raw(&log_path, &format!("{}\n", block.to_json_line().unwrap()));

        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            sync_from_log(&db, &log).unwrap();
            assert_eq!(db.get_last_sync_line().unwrap(), 2);
            assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), Some("block".to_string()));
        }

        // Step 2: Inject empty line + lgtm, sync
        append_raw(&log_path, "\n"); // empty at idx 2
        append_raw(&log_path, &format!("{}\n", lgtm.to_json_line().unwrap())); // lgtm at idx 3

        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            sync_from_log(&db, &log).unwrap();
            // last_sync = 2 + 1 = 3 (drift: should be 4)
            assert_eq!(db.get_last_sync_line().unwrap(), 3);
            assert_eq!(
                query_vote(&db, "cr-001", "reviewer-a"),
                Some("lgtm".to_string()),
                "After syncing lgtm, vote should be lgtm"
            );
        }

        // Step 3: No new events, but sync again (simulates crit review)
        // Due to drift (last_sync=3, lgtm is at idx 3), it re-reads lgtm
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            let count = sync_from_log(&db, &log).unwrap();

            // Should re-process lgtm (harmless)
            assert_eq!(count, 1, "Re-processes lgtm due to drift");
            assert_eq!(
                query_vote(&db, "cr-001", "reviewer-a"),
                Some("lgtm".to_string()),
                "Vote must remain lgtm after re-sync"
            );
            // Now last_sync = 4, drift resolved
            assert_eq!(db.get_last_sync_line().unwrap(), 4);
        }

        // Step 4: Final sync should be clean (no drift)
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            let count = sync_from_log(&db, &log).unwrap();
            assert_eq!(count, 0, "No more drift, clean sync");
        }
    }

    /// jj working copy restoration causes events.jsonl to revert to an older
    /// version while index.db retains a stale last_sync_line.
    ///
    /// With the truncation detection fix, sync_from_log detects that
    /// last_sync_line > total file lines and rebuilds the projection.
    #[test]
    fn test_bd_1s1_jj_restore_triggers_rebuild() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");
        let db_path = dir.path().join("index.db");

        // === Phase 1: Normal operation — create review + block vote ===
        let review = make_review_created("cr-001");
        let reviewers = EventEnvelope::new(
            "author",
            Event::ReviewersRequested(ReviewersRequested {
                review_id: "cr-001".to_string(),
                reviewers: vec!["reviewer-a".to_string()],
            }),
        );
        let block = make_block_vote("cr-001", "reviewer-a", "Needs fixes");

        // Write 3 events
        write_raw(&log_path, "");
        for event in [&review, &reviewers, &block] {
            append_raw(&log_path, &format!("{}\n", event.to_json_line().unwrap()));
        }

        // Simulate CLI: open DB, sync all 3 events
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            let count = sync_from_log(&db, &log).unwrap();
            assert_eq!(count, 3);
            assert_eq!(db.get_last_sync_line().unwrap(), 3);
            assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), Some("block".to_string()));
        }

        // === Phase 2: jj restores events.jsonl to an older version ===
        let restored_content = format!("{}\n", review.to_json_line().unwrap());
        write_raw(&log_path, &restored_content);
        // File: 1 event. DB: last_sync_line=3, block vote present.

        // === Phase 3: crit lgtm — sync detects truncation, rebuilds ===
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            let count = sync_from_log(&db, &log).unwrap();
            // Truncation detected (last_sync=3 > total_lines=1) → rebuild from file
            assert_eq!(count, 1, "Rebuild replays the 1 event in the restored file");
            assert_eq!(db.get_last_sync_line().unwrap(), 1);

            // Block vote is GONE — it's not in the restored file
            assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), None);
            assert!(!db.has_blocking_votes("cr-001").unwrap());
        }

        // Append lgtm vote
        let lgtm = make_lgtm_vote("cr-001", "reviewer-a");
        append_raw(&log_path, &format!("{}\n", lgtm.to_json_line().unwrap()));

        // === Phase 4: crit review — picks up lgtm normally ===
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            let count = sync_from_log(&db, &log).unwrap();
            assert_eq!(count, 1, "Syncs the new lgtm event");

            // FIXED: vote is lgtm!
            assert_eq!(
                query_vote(&db, "cr-001", "reviewer-a"),
                Some("lgtm".to_string()),
                "Vote should be lgtm after rebuild + sync"
            );
            assert!(!db.has_blocking_votes("cr-001").unwrap());
        }
    }

    /// Variant: jj restores events.jsonl to a version with MORE events
    /// than index.db has seen (e.g., workspace merge brings in new events).
    /// This should work correctly.
    #[test]
    fn test_bd_1s1_jj_restore_with_more_events() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");
        let db_path = dir.path().join("index.db");

        // Write just ReviewCreated, sync
        let review = make_review_created("cr-001");
        write_raw(&log_path, &format!("{}\n", review.to_json_line().unwrap()));

        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            sync_from_log(&db, &log).unwrap();
            assert_eq!(db.get_last_sync_line().unwrap(), 1);
        }

        // jj restore brings in a version with MORE events (block + lgtm)
        let block = make_block_vote("cr-001", "reviewer-a", "Needs fixes");
        let lgtm = make_lgtm_vote("cr-001", "reviewer-a");
        let mut content = format!("{}\n", review.to_json_line().unwrap());
        content.push_str(&format!("{}\n", block.to_json_line().unwrap()));
        content.push_str(&format!("{}\n", lgtm.to_json_line().unwrap()));
        write_raw(&log_path, &content);

        // Sync should pick up new events from line 1 onwards
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            let count = sync_from_log(&db, &log).unwrap();
            assert_eq!(count, 2, "Should pick up block + lgtm");
            assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), Some("lgtm".to_string()));
        }
    }

    /// bd-oum: jj restores events.jsonl to a version with the SAME number
    /// of lines but DIFFERENT content. The truncation check (line count) passes,
    /// but the content hash detects the replacement and triggers a rebuild.
    #[test]
    fn test_bd_oum_same_length_content_replacement() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");
        let db_path = dir.path().join("index.db");

        // === Phase 1: Create review cr-001 with a block vote, sync ===
        let review1 = make_review_created("cr-001");
        let block1 = make_block_vote("cr-001", "reviewer-a", "Needs fixes");
        write_raw(&log_path, "");
        for event in [&review1, &block1] {
            append_raw(&log_path, &format!("{}\n", event.to_json_line().unwrap()));
        }

        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            let count = sync_from_log(&db, &log).unwrap();
            assert_eq!(count, 2);
            assert_eq!(db.get_last_sync_line().unwrap(), 2);
            assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), Some("block".to_string()));

            // Hash should be stored
            assert!(db.get_events_file_hash().unwrap().is_some(), "Hash should be stored after sync");
        }

        // === Phase 2: jj replaces file with SAME line count, DIFFERENT content ===
        // A different review (cr-002) with an lgtm vote — same 2 lines, different content.
        let review2 = make_review_created("cr-002");
        let lgtm2 = make_lgtm_vote("cr-002", "reviewer-b");
        let replaced = format!(
            "{}\n{}\n",
            review2.to_json_line().unwrap(),
            lgtm2.to_json_line().unwrap()
        );
        write_raw(&log_path, &replaced);

        // File still has 2 lines — truncation check won't catch this.
        let line_count = std::fs::read_to_string(&log_path)
            .unwrap()
            .lines()
            .count();
        assert_eq!(line_count, 2, "File should still have 2 lines");

        // === Phase 3: sync detects hash mismatch, rebuilds ===
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            let count = sync_from_log(&db, &log).unwrap();

            // Rebuild replays the 2 events from the replaced file
            assert_eq!(count, 2, "Rebuild replays all events from replaced file");
            assert_eq!(db.get_last_sync_line().unwrap(), 2);

            // cr-001 block vote is GONE (not in replaced file)
            assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), None);

            // cr-002 lgtm vote is present (from replaced file)
            assert_eq!(query_vote(&db, "cr-002", "reviewer-b"), Some("lgtm".to_string()));
        }
    }

    /// bd-oum: Verify that the hash is backfilled on existing databases
    /// that were created before hash tracking was added.
    #[test]
    fn test_bd_oum_hash_backfill_on_noop_sync() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");
        let db_path = dir.path().join("index.db");

        // Write 2 events and sync
        let review = make_review_created("cr-001");
        let block = make_block_vote("cr-001", "reviewer-a", "Needs fixes");
        write_raw(&log_path, "");
        for event in [&review, &block] {
            append_raw(&log_path, &format!("{}\n", event.to_json_line().unwrap()));
        }

        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            sync_from_log(&db, &log).unwrap();
            assert!(db.get_events_file_hash().unwrap().is_some());

            // Simulate a pre-hash database by clearing the hash
            db.conn().execute(
                "UPDATE sync_state SET events_file_hash = NULL WHERE id = 1",
                [],
            ).unwrap();
            assert!(db.get_events_file_hash().unwrap().is_none());
        }

        // No-op sync should backfill the hash
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            let count = sync_from_log(&db, &log).unwrap();
            assert_eq!(count, 0, "No new events to process");
            assert!(db.get_events_file_hash().unwrap().is_some(), "Hash should be backfilled");
        }

        // Now same-length replacement should be detected
        let review2 = make_review_created("cr-002");
        let lgtm2 = make_lgtm_vote("cr-002", "reviewer-b");
        let replaced = format!(
            "{}\n{}\n",
            review2.to_json_line().unwrap(),
            lgtm2.to_json_line().unwrap()
        );
        write_raw(&log_path, &replaced);

        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            let count = sync_from_log(&db, &log).unwrap();
            assert_eq!(count, 2, "Should rebuild from replaced file");
            assert_eq!(query_vote(&db, "cr-001", "reviewer-a"), None);
            assert_eq!(query_vote(&db, "cr-002", "reviewer-b"), Some("lgtm".to_string()));
        }
    }

    // ========================================================================
    // End of bd-1s1 reproduction tests
    // ========================================================================

    #[test]
    fn test_reviewer_vote_replacement() {
        use crate::events::VoteType;

        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        // Create a review
        apply_event(&db, &make_review_created("cr-001")).unwrap();

        // Reviewer casts block vote
        apply_event(
            &db,
            &EventEnvelope::new(
                "reviewer",
                Event::ReviewerVoted(ReviewerVoted {
                    review_id: "cr-001".to_string(),
                    vote: VoteType::Block,
                    reason: Some("Needs fixes".to_string()),
                }),
            ),
        )
        .unwrap();

        // Verify block vote exists
        let (vote, reason): (String, Option<String>) = db
            .conn()
            .query_row(
                "SELECT vote, reason FROM reviewer_votes WHERE review_id = ? AND reviewer = ?",
                params!["cr-001", "reviewer"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(vote, "block");
        assert_eq!(reason, Some("Needs fixes".to_string()));

        // Reviewer changes to LGTM vote
        apply_event(
            &db,
            &EventEnvelope::new(
                "reviewer",
                Event::ReviewerVoted(ReviewerVoted {
                    review_id: "cr-001".to_string(),
                    vote: VoteType::Lgtm,
                    reason: Some("Looks good now".to_string()),
                }),
            ),
        )
        .unwrap();

        // Verify LGTM vote replaced block vote
        let (vote, reason): (String, Option<String>) = db
            .conn()
            .query_row(
                "SELECT vote, reason FROM reviewer_votes WHERE review_id = ? AND reviewer = ?",
                params!["cr-001", "reviewer"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(vote, "lgtm");
        assert_eq!(reason, Some("Looks good now".to_string()));

        // Verify only one vote row exists
        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM reviewer_votes WHERE review_id = ? AND reviewer = ?",
                params!["cr-001", "reviewer"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Verify has_blocking_votes returns false
        let has_blocks = db.conn()
            .query_row(
                "SELECT COUNT(*) FROM reviewer_votes WHERE review_id = ? AND vote = 'block'",
                params!["cr-001"],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(has_blocks, 0);
    }

    /// bd-2m6: Test orphan detection saves lost reviews to backup file
    /// when truncation is detected and crit_dir is provided.
    #[test]
    fn test_bd_2m6_orphan_detection_backup() {
        let dir = tempdir().unwrap();
        let log_path = dir.path().join("events.jsonl");
        let db_path = dir.path().join("index.db");
        let crit_dir = dir.path();

        // === Phase 1: Create two reviews and sync ===
        let review1 = make_review_created("cr-001");
        let review2 = make_review_created("cr-002");
        write_raw(&log_path, "");
        append_raw(&log_path, &format!("{}\n", review1.to_json_line().unwrap()));
        append_raw(&log_path, &format!("{}\n", review2.to_json_line().unwrap()));

        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);
            let count = sync_from_log_with_backup(&db, &log, Some(crit_dir)).unwrap();
            assert_eq!(count, 2);

            // Both reviews should exist
            assert!(db.get_review("cr-001").unwrap().is_some());
            assert!(db.get_review("cr-002").unwrap().is_some());
        }

        // === Phase 2: Truncate file to only contain cr-002 ===
        // This simulates jj restoring an older version
        let truncated = format!("{}\n", review2.to_json_line().unwrap());
        write_raw(&log_path, &truncated);

        // === Phase 3: Sync with backup enabled ===
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap();
            let log = crate::log::FileLog::new(&log_path);

            // This should detect truncation and create a backup file
            let count = sync_from_log_with_backup(&db, &log, Some(crit_dir)).unwrap();
            assert_eq!(count, 1, "Rebuild from truncated file");

            // cr-001 is now gone (orphaned)
            assert!(db.get_review("cr-001").unwrap().is_none());
            // cr-002 still exists
            assert!(db.get_review("cr-002").unwrap().is_some());
        }

        // === Phase 4: Verify backup file was created ===
        let backup_files: Vec<_> = std::fs::read_dir(crit_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("orphaned-reviews-"))
            .collect();

        assert_eq!(backup_files.len(), 1, "Should have created one backup file");

        let backup_content = std::fs::read_to_string(backup_files[0].path()).unwrap();
        let backup: serde_json::Value = serde_json::from_str(&backup_content).unwrap();

        // Verify backup contains cr-001
        let orphaned = backup["orphaned_reviews"].as_array().unwrap();
        assert_eq!(orphaned.len(), 1);
        assert_eq!(orphaned[0]["review_id"], "cr-001");
    }

    // ========================================================================
    // bd-13r: Schema migration tests
    // ========================================================================

    #[test]
    fn test_migrate_schema_adds_next_comment_number() {
        // Simulate an old database that was created before next_comment_number
        // column was added to the threads table.
        //
        // This test creates a database with the full schema but manually
        // removes the next_comment_number column to simulate upgrading from
        // an old version.
        let tmp_dir = tempfile::tempdir().unwrap();
        let db_path = tmp_dir.path().join("test.db");

        // Create an old-style database without next_comment_number
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            // Old threads table schema (matches current schema except no next_comment_number)
            // We need to create all the tables since SCHEMA_SQL creates indexes and views
            // that reference them.
            conn.execute_batch(
                "CREATE TABLE sync_state (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    last_line_number INTEGER NOT NULL DEFAULT 0,
                    last_event_time TEXT
                );
                INSERT INTO sync_state (id) VALUES (1);

                CREATE TABLE reviews (
                    review_id TEXT PRIMARY KEY,
                    jj_change_id TEXT NOT NULL,
                    initial_commit TEXT NOT NULL,
                    final_commit TEXT,
                    title TEXT NOT NULL,
                    description TEXT,
                    author TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'open',
                    status_changed_at TEXT,
                    status_changed_by TEXT,
                    abandon_reason TEXT
                );

                -- Old threads table without next_comment_number
                CREATE TABLE threads (
                    thread_id TEXT PRIMARY KEY,
                    review_id TEXT NOT NULL REFERENCES reviews(review_id),
                    file_path TEXT NOT NULL,
                    selection_type TEXT NOT NULL CHECK (selection_type IN ('line', 'range')),
                    selection_start INTEGER NOT NULL,
                    selection_end INTEGER,
                    commit_hash TEXT NOT NULL,
                    author TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'open'
                        CHECK (status IN ('open', 'resolved')),
                    status_changed_at TEXT,
                    status_changed_by TEXT,
                    resolve_reason TEXT,
                    reopen_reason TEXT
                    -- NOTE: next_comment_number column is intentionally MISSING
                );

                CREATE TABLE comments (
                    comment_id TEXT PRIMARY KEY,
                    thread_id TEXT NOT NULL REFERENCES threads(thread_id),
                    body TEXT NOT NULL,
                    author TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );

                CREATE TABLE reviewer_requests (
                    review_id TEXT NOT NULL REFERENCES reviews(review_id),
                    reviewer TEXT NOT NULL,
                    requested_at TEXT NOT NULL,
                    requested_by TEXT NOT NULL,
                    PRIMARY KEY (review_id, reviewer)
                );

                CREATE TABLE reviewer_votes (
                    review_id TEXT NOT NULL REFERENCES reviews(review_id),
                    reviewer TEXT NOT NULL,
                    vote TEXT NOT NULL,
                    reason TEXT,
                    voted_at TEXT NOT NULL,
                    PRIMARY KEY (review_id, reviewer)
                );

                -- Add test data: a review and thread
                INSERT INTO reviews (review_id, jj_change_id, initial_commit, title, author, created_at)
                VALUES ('cr-001', 'abc123', 'def456', 'Test Review', 'test', '2026-01-01T00:00:00Z');

                INSERT INTO threads (thread_id, review_id, file_path, selection_type,
                    selection_start, commit_hash, author, created_at)
                VALUES ('th-001', 'cr-001', 'src/lib.rs', 'line', 42, 'abc123',
                    'test', '2026-01-01T00:00:00Z');",
            )
            .unwrap();

            // Verify no next_comment_number column exists
            let has_column: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM pragma_table_info('threads') WHERE name = 'next_comment_number'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(!has_column, "Old database should not have next_comment_number");
        }

        // Now open with ProjectionDb which should run migration
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            // init_schema should run migrate_schema which adds the column
            db.init_schema().unwrap();

            // Verify column now exists
            let has_column: bool = db
                .conn()
                .query_row(
                    "SELECT COUNT(*) > 0 FROM pragma_table_info('threads') WHERE name = 'next_comment_number'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(has_column, "Migration should have added next_comment_number");

            // Verify existing row got default value
            let next_num: i64 = db
                .conn()
                .query_row(
                    "SELECT next_comment_number FROM threads WHERE thread_id = ?",
                    params!["th-001"],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(next_num, 1, "Existing row should have default value 1");
        }

        // Verify migration is idempotent (can run again without error)
        {
            let db = ProjectionDb::open(&db_path).unwrap();
            db.init_schema().unwrap(); // Should not fail on second run
        }
    }

    // ========================================================================
    // bd-2ys: Orphaned event filtering tests
    // ========================================================================

    fn make_comment_added(comment_id: &str, thread_id: &str) -> EventEnvelope {
        EventEnvelope::new(
            "test_author",
            Event::CommentAdded(CommentAdded {
                comment_id: comment_id.to_string(),
                thread_id: thread_id.to_string(),
                body: "Test comment".to_string(),
            }),
        )
    }

    fn make_thread_resolved(thread_id: &str) -> EventEnvelope {
        EventEnvelope::new(
            "test_author",
            Event::ThreadResolved(ThreadResolved {
                thread_id: thread_id.to_string(),
                reason: Some("Fixed".to_string()),
            }),
        )
    }

    #[test]
    fn test_bd_2ys_filter_orphaned_skips_events_without_review_created() {
        // Simulate: ThreadCreated + CommentAdded for cr-orphan (no ReviewCreated)
        let events = vec![
            make_thread_created("th-001", "cr-orphan"),
            make_comment_added("th-001.1", "th-001"),
        ];

        let (filtered, skipped) = filter_orphaned_events(events, None);
        assert_eq!(skipped, 2);
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_bd_2ys_filter_orphaned_keeps_valid_events() {
        // cr-good has ReviewCreated; cr-orphan does not
        let events = vec![
            make_review_created("cr-good"),
            make_thread_created("th-001", "cr-good"),
            make_comment_added("th-001.1", "th-001"),
            make_thread_created("th-002", "cr-orphan"),
            make_comment_added("th-002.1", "th-002"),
        ];

        let (filtered, skipped) = filter_orphaned_events(events, None);
        assert_eq!(skipped, 2, "should skip 2 orphaned events");
        assert_eq!(filtered.len(), 3, "should keep 3 valid events");
    }

    #[test]
    fn test_bd_2ys_filter_orphaned_handles_thread_only_events() {
        // ThreadResolved references a thread whose review is orphaned
        let events = vec![
            make_review_created("cr-good"),
            make_thread_created("th-001", "cr-good"),
            make_thread_created("th-002", "cr-orphan"),
            make_thread_resolved("th-002"),
            make_comment_added("th-002.1", "th-002"),
        ];

        let (filtered, skipped) = filter_orphaned_events(events, None);
        assert_eq!(skipped, 3, "should skip ThreadCreated + ThreadResolved + CommentAdded for orphan");
        assert_eq!(filtered.len(), 2, "should keep ReviewCreated + ThreadCreated for cr-good");
    }

    #[test]
    fn test_bd_2ys_filter_orphaned_no_orphans_passes_through() {
        let events = vec![
            make_review_created("cr-001"),
            make_thread_created("th-001", "cr-001"),
            make_comment_added("th-001.1", "th-001"),
        ];

        let (filtered, skipped) = filter_orphaned_events(events, None);
        assert_eq!(skipped, 0);
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn test_bd_2ys_orphaned_events_dont_crash_sync() {
        // Integration test: orphaned events should not cause FK error during sync
        let dir = tempdir().unwrap();
        let crit_root = dir.path();

        // Write events WITHOUT ReviewCreated (simulating destroyed workspace)
        let log = crate::log::ReviewLog::new(crit_root, "cr-orphan");
        log.append(&make_thread_created("th-001", "cr-orphan")).unwrap();
        log.append(&make_comment_added("th-001.1", "th-001")).unwrap();

        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        // This should NOT fail with FK constraint error
        let result = sync_from_review_logs(&db, crit_root);
        assert!(result.is_ok(), "sync should succeed even with orphaned events: {:?}", result.err());
    }

    #[test]
    fn test_bd_2ys_orphaned_events_dont_crash_rebuild() {
        // Integration test: orphaned events should not cause FK error during rebuild
        let dir = tempdir().unwrap();
        let crit_root = dir.path();

        // Write a valid review
        let good_log = crate::log::ReviewLog::new(crit_root, "cr-good");
        good_log.append(&make_review_created("cr-good")).unwrap();
        good_log.append(&make_thread_created("th-good", "cr-good")).unwrap();

        // Write orphaned events (no ReviewCreated)
        let orphan_log = crate::log::ReviewLog::new(crit_root, "cr-orphan");
        orphan_log.append(&make_thread_created("th-orphan", "cr-orphan")).unwrap();
        orphan_log.append(&make_comment_added("th-orphan.1", "th-orphan")).unwrap();

        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        // Rebuild should succeed, applying only cr-good events
        let count = rebuild_from_review_logs(&db, crit_root).unwrap();
        assert_eq!(count, 2, "should apply 2 events from cr-good");

        // Verify cr-good was indexed
        let title: String = db
            .conn()
            .query_row(
                "SELECT title FROM reviews WHERE review_id = ?",
                params!["cr-good"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(title, "Review cr-good");

        // Verify cr-orphan was NOT indexed
        let orphan_exists: bool = db
            .conn()
            .query_row(
                "SELECT COUNT(*) > 0 FROM reviews WHERE review_id = ?",
                params!["cr-orphan"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!orphan_exists, "orphaned review should not be in the projection");
    }

    // ========================================================================
    // bd-2qa: Cross-workspace orphan detection tests
    // ========================================================================

    #[test]
    fn test_bd_2qa_extra_known_reviews_prevents_false_orphan() {
        // Simulate: root .crit/ has ReviewersRequested for cr-ws (no ReviewCreated),
        // but cr-ws is known to exist in another workspace. With extra_known_reviews,
        // the event should NOT be treated as orphaned.
        let events = vec![EventEnvelope::new(
            "test_author",
            Event::ReviewersRequested(ReviewersRequested {
                review_id: "cr-ws".to_string(),
                reviewers: vec!["security-bot".to_string()],
            }),
        )];

        // Without extra known reviews: should be orphaned
        let (filtered, skipped) = filter_orphaned_events(events.clone(), None);
        assert_eq!(skipped, 1, "without extra known, event should be orphaned");
        assert!(filtered.is_empty());

        // With extra known reviews: should NOT be orphaned
        let mut known = HashSet::new();
        known.insert("cr-ws".to_string());
        let (filtered, skipped) = filter_orphaned_events(events, Some(&known));
        assert_eq!(skipped, 0, "with extra known, event should be kept");
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn test_bd_2qa_extra_known_reviews_still_filters_real_orphans() {
        // Extra known reviews should not prevent filtering of truly orphaned events.
        // cr-ws is known from another workspace, but cr-gone is truly orphaned.
        let events = vec![
            EventEnvelope::new(
                "test_author",
                Event::ReviewersRequested(ReviewersRequested {
                    review_id: "cr-ws".to_string(),
                    reviewers: vec!["security-bot".to_string()],
                }),
            ),
            make_thread_created("th-orphan", "cr-gone"),
            make_comment_added("th-orphan.1", "th-orphan"),
        ];

        let mut known = HashSet::new();
        known.insert("cr-ws".to_string());
        let (filtered, skipped) = filter_orphaned_events(events, Some(&known));
        assert_eq!(skipped, 2, "cr-gone events should still be orphaned");
        assert_eq!(filtered.len(), 1, "only cr-ws event should survive");
    }

    #[test]
    fn test_bd_2qa_rebuild_from_events_cross_workspace() {
        // Full integration test: simulate the cross-workspace scenario.
        // "root" dir has only ReviewersRequested for cr-ws.
        // "workspace" dir has the full ReviewCreated.
        // Merging all events and rebuilding should handle cross-workspace events.

        let root_dir = tempdir().unwrap();
        let ws_dir = tempdir().unwrap();

        // Write ReviewCreated in workspace .crit/
        let ws_log = crate::log::ReviewLog::new(ws_dir.path(), "cr-ws");
        ws_log
            .append(&make_review_created("cr-ws"))
            .unwrap();

        // Write ReviewersRequested in root .crit/ (no ReviewCreated here)
        let root_log = crate::log::ReviewLog::new(root_dir.path(), "cr-ws");
        root_log
            .append(&EventEnvelope::new(
                "test_author",
                Event::ReviewersRequested(ReviewersRequested {
                    review_id: "cr-ws".to_string(),
                    reviewers: vec!["security-bot".to_string()],
                }),
            ))
            .unwrap();

        // Collect all events from both "workspaces" (simulating inbox --all-workspaces)
        let mut all_events = crate::log::read_all_reviews(ws_dir.path()).unwrap();
        all_events.extend(crate::log::read_all_reviews(root_dir.path()).unwrap());
        all_events.sort_by(|a, b| a.ts.cmp(&b.ts));

        // Build merged projection — should NOT fail with FK error
        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();

        let count = rebuild_from_events(&db, all_events).unwrap();
        assert_eq!(count, 2, "should apply both ReviewCreated and ReviewersRequested");

        // Verify the review exists with reviewers
        let review_exists: bool = db
            .conn()
            .query_row(
                "SELECT COUNT(*) > 0 FROM reviews WHERE review_id = ?",
                params!["cr-ws"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(review_exists, "review should exist in projection");

        let has_reviewer: bool = db
            .conn()
            .query_row(
                "SELECT COUNT(*) > 0 FROM review_reviewers WHERE review_id = ?",
                params!["cr-ws"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_reviewer, "reviewer should be recorded");
    }

    #[test]
    fn test_bd_2qa_merged_events_inbox_shows_review() {
        // End-to-end test: verify that after merging events from multiple
        // workspaces, get_inbox correctly returns the review for the reviewer.

        let root_dir = tempdir().unwrap();
        let ws_dir = tempdir().unwrap();

        // ReviewCreated in workspace
        let ws_log = crate::log::ReviewLog::new(ws_dir.path(), "cr-ws");
        ws_log.append(&make_review_created("cr-ws")).unwrap();

        // ReviewersRequested in root (reviewer = "security-bot")
        let root_log = crate::log::ReviewLog::new(root_dir.path(), "cr-ws");
        root_log
            .append(&EventEnvelope::new(
                "test_author",
                Event::ReviewersRequested(ReviewersRequested {
                    review_id: "cr-ws".to_string(),
                    reviewers: vec!["security-bot".to_string()],
                }),
            ))
            .unwrap();

        // Merge events and build projection
        let mut all_events = crate::log::read_all_reviews(ws_dir.path()).unwrap();
        all_events.extend(crate::log::read_all_reviews(root_dir.path()).unwrap());
        all_events.sort_by(|a, b| a.ts.cmp(&b.ts));

        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();
        rebuild_from_events(&db, all_events).unwrap();

        // Check inbox for security-bot — should show the review
        let inbox = db.get_inbox("security-bot").unwrap();
        assert_eq!(
            inbox.reviews_awaiting_vote.len(),
            1,
            "security-bot should see 1 review awaiting vote"
        );
        assert_eq!(inbox.reviews_awaiting_vote[0].review_id, "cr-ws");
    }
}
