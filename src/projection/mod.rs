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

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::events::{
    CodeSelection, CommentAdded, Event, EventEnvelope, ReviewAbandoned, ReviewApproved,
    ReviewCreated, ReviewMerged, ReviewerVoted, ReviewersRequested, ThreadCreated, ThreadReopened,
    ThreadResolved,
};
use crate::log::AppendLog;

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

    /// Create an in-memory projection database (for testing).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("Failed to open in-memory database")?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .context("Failed to enable foreign keys")?;
        Ok(Self { conn })
    }

    /// Initialize the database schema.
    ///
    /// Creates all tables, indexes, and views if they don't exist.
    pub fn init_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(SCHEMA_SQL)
            .context("Failed to initialize schema")?;
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
    let last_line = db.get_last_sync_line()?;

    if last_line > 0 {
        let total = log.total_lines()?;

        // Check 1: Truncation — file has fewer lines than our sync cursor.
        if last_line > total {
            eprintln!(
                "WARNING: events.jsonl truncated (expected >={} lines, found {}). Rebuilding projection.",
                last_line, total
            );
            return rebuild_projection(db, log);
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
                    return rebuild_projection(db, log);
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
            "INSERT OR IGNORE INTO review_reviewers (
                review_id, reviewer, requested_at, requested_by
            ) VALUES (?, ?, ?, ?)",
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
    reopen_reason TEXT
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
CREATE VIEW IF NOT EXISTS v_reviews_summary AS
SELECT
    r.review_id,
    r.title,
    r.author,
    r.status,
    r.jj_change_id,
    r.created_at,
    COUNT(DISTINCT t.thread_id) AS thread_count,
    COUNT(DISTINCT CASE WHEN t.status = 'open' THEN t.thread_id END) AS open_thread_count
FROM reviews r
LEFT JOIN threads t ON t.review_id = r.review_id
GROUP BY r.review_id;

CREATE VIEW IF NOT EXISTS v_threads_detail AS
SELECT
    t.*,
    r.title AS review_title,
    COUNT(c.comment_id) AS comment_count
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
            (1, "crit reviews merge"),               // event 19
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
}
