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
pub fn sync_from_log(db: &ProjectionDb, log: &impl AppendLog) -> Result<usize> {
    let last_line = db.get_last_sync_line()?;
    let events = log.read_from(last_line)?;

    if events.is_empty() {
        return Ok(0);
    }

    let count = events.len();

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
    let new_line = last_line + count;
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "UPDATE sync_state SET last_line_number = ?, last_sync_ts = ? WHERE id = 1",
        params![new_line as i64, now],
    )
    .context("Failed to update sync_state")?;

    tx.commit().context("Failed to commit transaction")?;

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
