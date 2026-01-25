//! Query API for the projection database.
//!
//! Provides structured access to reviews, threads, and comments
//! with optional filtering. All result types implement Serialize
//! for TOON/JSON output.

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;

use super::ProjectionDb;

// ============================================================================
// Query Result Types
// ============================================================================

/// Summary of a review for list views.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewSummary {
    pub review_id: String,
    pub title: String,
    pub author: String,
    pub status: String,
    pub thread_count: i64,
    pub open_thread_count: i64,
}

/// Full details of a review.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewDetail {
    pub review_id: String,
    pub jj_change_id: String,
    pub initial_commit: String,
    pub final_commit: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub author: String,
    pub created_at: String,
    pub status: String,
    pub status_changed_at: Option<String>,
    pub status_changed_by: Option<String>,
    pub abandon_reason: Option<String>,
    pub thread_count: i64,
    pub open_thread_count: i64,
    pub reviewers: Vec<String>,
}

/// Summary of a thread for list views.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadSummary {
    pub thread_id: String,
    pub file_path: String,
    pub selection_start: i64,
    pub selection_end: Option<i64>,
    pub status: String,
    pub comment_count: i64,
}

/// Full details of a thread with comments.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadDetail {
    pub thread_id: String,
    pub review_id: String,
    pub file_path: String,
    pub selection_type: String,
    pub selection_start: i64,
    pub selection_end: Option<i64>,
    pub commit_hash: String,
    pub author: String,
    pub created_at: String,
    pub status: String,
    pub status_changed_at: Option<String>,
    pub status_changed_by: Option<String>,
    pub resolve_reason: Option<String>,
    pub reopen_reason: Option<String>,
    pub comments: Vec<Comment>,
}

/// A single comment in a thread.
#[derive(Debug, Clone, Serialize)]
pub struct Comment {
    pub comment_id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

// ============================================================================
// Query Functions
// ============================================================================

impl ProjectionDb {
    /// List reviews with optional filtering.
    ///
    /// Returns reviews sorted by creation date (newest first).
    pub fn list_reviews(
        &self,
        status: Option<&str>,
        author: Option<&str>,
    ) -> Result<Vec<ReviewSummary>> {
        self.list_reviews_filtered(status, author, None, false)
    }

    /// List reviews with extended filtering options.
    ///
    /// - `needs_reviewer`: Only return reviews where this agent is a requested reviewer
    /// - `has_unresolved`: Only return reviews with open_thread_count > 0
    pub fn list_reviews_filtered(
        &self,
        status: Option<&str>,
        author: Option<&str>,
        needs_reviewer: Option<&str>,
        has_unresolved: bool,
    ) -> Result<Vec<ReviewSummary>> {
        let mut sql = String::from(
            "SELECT DISTINCT v.review_id, v.title, v.author, v.status, v.thread_count, v.open_thread_count
             FROM v_reviews_summary v",
        );
        let mut param_values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        // Join with review_reviewers if filtering by reviewer
        if needs_reviewer.is_some() {
            sql.push_str(" JOIN review_reviewers rr ON v.review_id = rr.review_id");
        }

        sql.push_str(" WHERE 1=1");

        if let Some(s) = status {
            sql.push_str(" AND v.status = ?");
            param_values.push(Box::new(s.to_string()));
        }
        if let Some(a) = author {
            sql.push_str(" AND v.author = ?");
            param_values.push(Box::new(a.to_string()));
        }
        if let Some(r) = needs_reviewer {
            sql.push_str(" AND rr.reviewer = ?");
            param_values.push(Box::new(r.to_string()));
        }
        if has_unresolved {
            sql.push_str(" AND v.open_thread_count > 0");
        }

        sql.push_str(" ORDER BY v.created_at DESC");

        let params: Vec<&dyn rusqlite::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();

        let mut stmt = self
            .conn
            .prepare(&sql)
            .context("Failed to prepare list_reviews query")?;

        let rows = stmt
            .query_map(params.as_slice(), |row| {
                Ok(ReviewSummary {
                    review_id: row.get(0)?,
                    title: row.get(1)?,
                    author: row.get(2)?,
                    status: row.get(3)?,
                    thread_count: row.get(4)?,
                    open_thread_count: row.get(5)?,
                })
            })
            .context("Failed to execute list_reviews query")?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.context("Failed to read review row")?);
        }
        Ok(results)
    }

    /// Get detailed information about a single review.
    ///
    /// Returns `None` if the review doesn't exist.
    pub fn get_review(&self, review_id: &str) -> Result<Option<ReviewDetail>> {
        // Get the review with thread counts
        let review_row: Option<ReviewDetailRow> = self
            .conn
            .query_row(
                "SELECT 
                    r.review_id, r.jj_change_id, r.initial_commit, r.final_commit,
                    r.title, r.description, r.author, r.created_at, r.status,
                    r.status_changed_at, r.status_changed_by, r.abandon_reason,
                    COALESCE(s.thread_count, 0), COALESCE(s.open_thread_count, 0)
                 FROM reviews r
                 LEFT JOIN v_reviews_summary s ON s.review_id = r.review_id
                 WHERE r.review_id = ?",
                params![review_id],
                |row| ReviewDetailRow::from_row(row),
            )
            .optional()
            .context("Failed to query review")?;

        let Some(row) = review_row else {
            return Ok(None);
        };

        // Get the reviewers
        let mut stmt = self
            .conn
            .prepare(
                "SELECT reviewer FROM review_reviewers WHERE review_id = ? ORDER BY requested_at",
            )
            .context("Failed to prepare reviewers query")?;

        let reviewers: Vec<String> = stmt
            .query_map(params![review_id], |row| row.get(0))
            .context("Failed to query reviewers")?
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to read reviewers")?;

        Ok(Some(ReviewDetail {
            review_id: row.review_id,
            jj_change_id: row.jj_change_id,
            initial_commit: row.initial_commit,
            final_commit: row.final_commit,
            title: row.title,
            description: row.description,
            author: row.author,
            created_at: row.created_at,
            status: row.status,
            status_changed_at: row.status_changed_at,
            status_changed_by: row.status_changed_by,
            abandon_reason: row.abandon_reason,
            thread_count: row.thread_count,
            open_thread_count: row.open_thread_count,
            reviewers,
        }))
    }

    /// List threads for a review with optional filtering.
    ///
    /// Returns threads sorted by file path, then line number.
    pub fn list_threads(
        &self,
        review_id: &str,
        status: Option<&str>,
        file: Option<&str>,
    ) -> Result<Vec<ThreadSummary>> {
        let mut sql = String::from(
            "SELECT thread_id, file_path, selection_start, selection_end, status, comment_count
             FROM v_threads_detail
             WHERE review_id = ?",
        );
        let mut param_values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        param_values.push(Box::new(review_id.to_string()));

        if let Some(s) = status {
            sql.push_str(" AND status = ?");
            param_values.push(Box::new(s.to_string()));
        }
        if let Some(f) = file {
            sql.push_str(" AND file_path = ?");
            param_values.push(Box::new(f.to_string()));
        }

        sql.push_str(" ORDER BY file_path, selection_start");

        let params: Vec<&dyn rusqlite::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();

        let mut stmt = self
            .conn
            .prepare(&sql)
            .context("Failed to prepare list_threads query")?;

        let rows = stmt
            .query_map(params.as_slice(), |row| {
                Ok(ThreadSummary {
                    thread_id: row.get(0)?,
                    file_path: row.get(1)?,
                    selection_start: row.get(2)?,
                    selection_end: row.get(3)?,
                    status: row.get(4)?,
                    comment_count: row.get(5)?,
                })
            })
            .context("Failed to execute list_threads query")?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.context("Failed to read thread row")?);
        }
        Ok(results)
    }

    /// Get detailed information about a single thread with its comments.
    ///
    /// Returns `None` if the thread doesn't exist.
    pub fn get_thread(&self, thread_id: &str) -> Result<Option<ThreadDetail>> {
        let thread_row: Option<ThreadDetailRow> = self
            .conn
            .query_row(
                "SELECT 
                    thread_id, review_id, file_path, selection_type,
                    selection_start, selection_end, commit_hash, author,
                    created_at, status, status_changed_at, status_changed_by,
                    resolve_reason, reopen_reason
                 FROM threads
                 WHERE thread_id = ?",
                params![thread_id],
                |row| ThreadDetailRow::from_row(row),
            )
            .optional()
            .context("Failed to query thread")?;

        let Some(row) = thread_row else {
            return Ok(None);
        };

        // Get comments for this thread
        let comments = self.list_comments(thread_id)?;

        Ok(Some(ThreadDetail {
            thread_id: row.thread_id,
            review_id: row.review_id,
            file_path: row.file_path,
            selection_type: row.selection_type,
            selection_start: row.selection_start,
            selection_end: row.selection_end,
            commit_hash: row.commit_hash,
            author: row.author,
            created_at: row.created_at,
            status: row.status,
            status_changed_at: row.status_changed_at,
            status_changed_by: row.status_changed_by,
            resolve_reason: row.resolve_reason,
            reopen_reason: row.reopen_reason,
            comments,
        }))
    }

    /// List all comments for a thread.
    ///
    /// Returns comments sorted by creation time (oldest first).
    pub fn list_comments(&self, thread_id: &str) -> Result<Vec<Comment>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT comment_id, author, body, created_at
                 FROM comments
                 WHERE thread_id = ?
                 ORDER BY created_at ASC",
            )
            .context("Failed to prepare list_comments query")?;

        let rows = stmt
            .query_map(params![thread_id], |row| {
                Ok(Comment {
                    comment_id: row.get(0)?,
                    author: row.get(1)?,
                    body: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .context("Failed to execute list_comments query")?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.context("Failed to read comment row")?);
        }
        Ok(results)
    }
}

// ============================================================================
// Internal Row Types (for query mapping)
// ============================================================================

/// Internal type for reading review details from the database.
struct ReviewDetailRow {
    review_id: String,
    jj_change_id: String,
    initial_commit: String,
    final_commit: Option<String>,
    title: String,
    description: Option<String>,
    author: String,
    created_at: String,
    status: String,
    status_changed_at: Option<String>,
    status_changed_by: Option<String>,
    abandon_reason: Option<String>,
    thread_count: i64,
    open_thread_count: i64,
}

impl ReviewDetailRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            review_id: row.get(0)?,
            jj_change_id: row.get(1)?,
            initial_commit: row.get(2)?,
            final_commit: row.get(3)?,
            title: row.get(4)?,
            description: row.get(5)?,
            author: row.get(6)?,
            created_at: row.get(7)?,
            status: row.get(8)?,
            status_changed_at: row.get(9)?,
            status_changed_by: row.get(10)?,
            abandon_reason: row.get(11)?,
            thread_count: row.get(12)?,
            open_thread_count: row.get(13)?,
        })
    }
}

/// Internal type for reading thread details from the database.
struct ThreadDetailRow {
    thread_id: String,
    review_id: String,
    file_path: String,
    selection_type: String,
    selection_start: i64,
    selection_end: Option<i64>,
    commit_hash: String,
    author: String,
    created_at: String,
    status: String,
    status_changed_at: Option<String>,
    status_changed_by: Option<String>,
    resolve_reason: Option<String>,
    reopen_reason: Option<String>,
}

impl ThreadDetailRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            thread_id: row.get(0)?,
            review_id: row.get(1)?,
            file_path: row.get(2)?,
            selection_type: row.get(3)?,
            selection_start: row.get(4)?,
            selection_end: row.get(5)?,
            commit_hash: row.get(6)?,
            author: row.get(7)?,
            created_at: row.get(8)?,
            status: row.get(9)?,
            status_changed_at: row.get(10)?,
            status_changed_by: row.get(11)?,
            resolve_reason: row.get(12)?,
            reopen_reason: row.get(13)?,
        })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{
        CodeSelection, CommentAdded, Event, EventEnvelope, ReviewCreated, ReviewersRequested,
        ThreadCreated, ThreadResolved,
    };
    use crate::projection::apply_event;

    fn setup_db() -> ProjectionDb {
        let db = ProjectionDb::open_in_memory().unwrap();
        db.init_schema().unwrap();
        db
    }

    fn make_review(review_id: &str, author: &str, title: &str) -> EventEnvelope {
        EventEnvelope::new(
            author,
            Event::ReviewCreated(ReviewCreated {
                review_id: review_id.to_string(),
                jj_change_id: format!("change-{review_id}"),
                initial_commit: format!("commit-{review_id}"),
                title: title.to_string(),
                description: Some(format!("Description for {review_id}")),
            }),
        )
    }

    fn make_thread(thread_id: &str, review_id: &str, file: &str, line: u32) -> EventEnvelope {
        EventEnvelope::new(
            "thread_author",
            Event::ThreadCreated(ThreadCreated {
                thread_id: thread_id.to_string(),
                review_id: review_id.to_string(),
                file_path: file.to_string(),
                selection: CodeSelection::line(line),
                commit_hash: "abc123".to_string(),
            }),
        )
    }

    fn make_comment(comment_id: &str, thread_id: &str, body: &str) -> EventEnvelope {
        EventEnvelope::new(
            "commenter",
            Event::CommentAdded(CommentAdded {
                comment_id: comment_id.to_string(),
                thread_id: thread_id.to_string(),
                body: body.to_string(),
            }),
        )
    }

    // ========================================================================
    // list_reviews tests
    // ========================================================================

    #[test]
    fn test_list_reviews_empty() {
        let db = setup_db();
        let reviews = db.list_reviews(None, None).unwrap();
        assert!(reviews.is_empty());
    }

    #[test]
    fn test_list_reviews_all() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "First review")).unwrap();
        apply_event(&db, &make_review("cr-002", "bob", "Second review")).unwrap();

        let reviews = db.list_reviews(None, None).unwrap();
        assert_eq!(reviews.len(), 2);
    }

    #[test]
    fn test_list_reviews_filter_by_status() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Open review")).unwrap();
        apply_event(&db, &make_review("cr-002", "alice", "Will be merged")).unwrap();

        // Merge the second review
        apply_event(
            &db,
            &EventEnvelope::new(
                "merger",
                Event::ReviewMerged(crate::events::ReviewMerged {
                    review_id: "cr-002".to_string(),
                    final_commit: "final".to_string(),
                }),
            ),
        )
        .unwrap();

        let open_reviews = db.list_reviews(Some("open"), None).unwrap();
        assert_eq!(open_reviews.len(), 1);
        assert_eq!(open_reviews[0].review_id, "cr-001");

        let merged_reviews = db.list_reviews(Some("merged"), None).unwrap();
        assert_eq!(merged_reviews.len(), 1);
        assert_eq!(merged_reviews[0].review_id, "cr-002");
    }

    #[test]
    fn test_list_reviews_filter_by_author() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Alice's review")).unwrap();
        apply_event(&db, &make_review("cr-002", "bob", "Bob's review")).unwrap();
        apply_event(&db, &make_review("cr-003", "alice", "Another Alice review")).unwrap();

        let alice_reviews = db.list_reviews(None, Some("alice")).unwrap();
        assert_eq!(alice_reviews.len(), 2);

        let bob_reviews = db.list_reviews(None, Some("bob")).unwrap();
        assert_eq!(bob_reviews.len(), 1);
        assert_eq!(bob_reviews[0].review_id, "cr-002");
    }

    #[test]
    fn test_list_reviews_filter_combined() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Open by alice")).unwrap();
        apply_event(&db, &make_review("cr-002", "alice", "Merged by alice")).unwrap();
        apply_event(&db, &make_review("cr-003", "bob", "Open by bob")).unwrap();

        // Merge cr-002
        apply_event(
            &db,
            &EventEnvelope::new(
                "merger",
                Event::ReviewMerged(crate::events::ReviewMerged {
                    review_id: "cr-002".to_string(),
                    final_commit: "final".to_string(),
                }),
            ),
        )
        .unwrap();

        let results = db.list_reviews(Some("open"), Some("alice")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].review_id, "cr-001");
    }

    #[test]
    fn test_list_reviews_with_thread_counts() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Review with threads")).unwrap();
        apply_event(&db, &make_thread("th-001", "cr-001", "src/main.rs", 10)).unwrap();
        apply_event(&db, &make_thread("th-002", "cr-001", "src/lib.rs", 20)).unwrap();

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

        let reviews = db.list_reviews(None, None).unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].thread_count, 2);
        assert_eq!(reviews[0].open_thread_count, 1);
    }

    // ========================================================================
    // get_review tests
    // ========================================================================

    #[test]
    fn test_get_review_not_found() {
        let db = setup_db();
        let result = db.get_review("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_review_basic() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Test review")).unwrap();

        let review = db.get_review("cr-001").unwrap().unwrap();
        assert_eq!(review.review_id, "cr-001");
        assert_eq!(review.title, "Test review");
        assert_eq!(review.author, "alice");
        assert_eq!(review.status, "open");
        assert_eq!(review.jj_change_id, "change-cr-001");
        assert!(review.description.is_some());
    }

    #[test]
    fn test_get_review_with_reviewers() {
        let db = setup_db();

        apply_event(
            &db,
            &make_review("cr-001", "alice", "Review with reviewers"),
        )
        .unwrap();
        apply_event(
            &db,
            &EventEnvelope::new(
                "alice",
                Event::ReviewersRequested(ReviewersRequested {
                    review_id: "cr-001".to_string(),
                    reviewers: vec!["bob".to_string(), "charlie".to_string()],
                }),
            ),
        )
        .unwrap();

        let review = db.get_review("cr-001").unwrap().unwrap();
        assert_eq!(review.reviewers.len(), 2);
        assert!(review.reviewers.contains(&"bob".to_string()));
        assert!(review.reviewers.contains(&"charlie".to_string()));
    }

    #[test]
    fn test_get_review_with_threads() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Review")).unwrap();
        apply_event(&db, &make_thread("th-001", "cr-001", "src/main.rs", 10)).unwrap();
        apply_event(&db, &make_thread("th-002", "cr-001", "src/lib.rs", 20)).unwrap();

        let review = db.get_review("cr-001").unwrap().unwrap();
        assert_eq!(review.thread_count, 2);
        assert_eq!(review.open_thread_count, 2);
    }

    // ========================================================================
    // list_threads tests
    // ========================================================================

    #[test]
    fn test_list_threads_empty() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Empty review")).unwrap();

        let threads = db.list_threads("cr-001", None, None).unwrap();
        assert!(threads.is_empty());
    }

    #[test]
    fn test_list_threads_all() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Review")).unwrap();
        apply_event(&db, &make_thread("th-001", "cr-001", "src/main.rs", 10)).unwrap();
        apply_event(&db, &make_thread("th-002", "cr-001", "src/lib.rs", 20)).unwrap();

        let threads = db.list_threads("cr-001", None, None).unwrap();
        assert_eq!(threads.len(), 2);
    }

    #[test]
    fn test_list_threads_filter_by_status() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Review")).unwrap();
        apply_event(&db, &make_thread("th-001", "cr-001", "src/main.rs", 10)).unwrap();
        apply_event(&db, &make_thread("th-002", "cr-001", "src/lib.rs", 20)).unwrap();

        // Resolve first thread
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

        let open_threads = db.list_threads("cr-001", Some("open"), None).unwrap();
        assert_eq!(open_threads.len(), 1);
        assert_eq!(open_threads[0].thread_id, "th-002");

        let resolved_threads = db.list_threads("cr-001", Some("resolved"), None).unwrap();
        assert_eq!(resolved_threads.len(), 1);
        assert_eq!(resolved_threads[0].thread_id, "th-001");
    }

    #[test]
    fn test_list_threads_filter_by_file() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Review")).unwrap();
        apply_event(&db, &make_thread("th-001", "cr-001", "src/main.rs", 10)).unwrap();
        apply_event(&db, &make_thread("th-002", "cr-001", "src/main.rs", 50)).unwrap();
        apply_event(&db, &make_thread("th-003", "cr-001", "src/lib.rs", 20)).unwrap();

        let main_threads = db
            .list_threads("cr-001", None, Some("src/main.rs"))
            .unwrap();
        assert_eq!(main_threads.len(), 2);

        let lib_threads = db.list_threads("cr-001", None, Some("src/lib.rs")).unwrap();
        assert_eq!(lib_threads.len(), 1);
    }

    #[test]
    fn test_list_threads_with_comment_counts() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Review")).unwrap();
        apply_event(&db, &make_thread("th-001", "cr-001", "src/main.rs", 10)).unwrap();
        apply_event(&db, &make_comment("c-001", "th-001", "First comment")).unwrap();
        apply_event(&db, &make_comment("c-002", "th-001", "Second comment")).unwrap();

        let threads = db.list_threads("cr-001", None, None).unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].comment_count, 2);
    }

    #[test]
    fn test_list_threads_sorted_by_file_and_line() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Review")).unwrap();
        apply_event(&db, &make_thread("th-001", "cr-001", "src/main.rs", 100)).unwrap();
        apply_event(&db, &make_thread("th-002", "cr-001", "src/lib.rs", 20)).unwrap();
        apply_event(&db, &make_thread("th-003", "cr-001", "src/main.rs", 10)).unwrap();

        let threads = db.list_threads("cr-001", None, None).unwrap();
        assert_eq!(threads.len(), 3);
        // Should be sorted by file, then line
        assert_eq!(threads[0].file_path, "src/lib.rs");
        assert_eq!(threads[1].file_path, "src/main.rs");
        assert_eq!(threads[1].selection_start, 10);
        assert_eq!(threads[2].file_path, "src/main.rs");
        assert_eq!(threads[2].selection_start, 100);
    }

    // ========================================================================
    // get_thread tests
    // ========================================================================

    #[test]
    fn test_get_thread_not_found() {
        let db = setup_db();
        let result = db.get_thread("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_thread_basic() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Review")).unwrap();
        apply_event(&db, &make_thread("th-001", "cr-001", "src/main.rs", 42)).unwrap();

        let thread = db.get_thread("th-001").unwrap().unwrap();
        assert_eq!(thread.thread_id, "th-001");
        assert_eq!(thread.review_id, "cr-001");
        assert_eq!(thread.file_path, "src/main.rs");
        assert_eq!(thread.selection_start, 42);
        assert_eq!(thread.status, "open");
        assert!(thread.comments.is_empty());
    }

    #[test]
    fn test_get_thread_with_comments() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Review")).unwrap();
        apply_event(&db, &make_thread("th-001", "cr-001", "src/main.rs", 10)).unwrap();
        apply_event(&db, &make_comment("c-001", "th-001", "First comment")).unwrap();
        apply_event(&db, &make_comment("c-002", "th-001", "Second comment")).unwrap();

        let thread = db.get_thread("th-001").unwrap().unwrap();
        assert_eq!(thread.comments.len(), 2);
        assert_eq!(thread.comments[0].body, "First comment");
        assert_eq!(thread.comments[1].body, "Second comment");
    }

    #[test]
    fn test_get_thread_resolved() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Review")).unwrap();
        apply_event(&db, &make_thread("th-001", "cr-001", "src/main.rs", 10)).unwrap();
        apply_event(
            &db,
            &EventEnvelope::new(
                "resolver",
                Event::ThreadResolved(ThreadResolved {
                    thread_id: "th-001".to_string(),
                    reason: Some("Fixed the issue".to_string()),
                }),
            ),
        )
        .unwrap();

        let thread = db.get_thread("th-001").unwrap().unwrap();
        assert_eq!(thread.status, "resolved");
        assert_eq!(thread.resolve_reason, Some("Fixed the issue".to_string()));
        assert_eq!(thread.status_changed_by, Some("resolver".to_string()));
    }

    // ========================================================================
    // list_comments tests
    // ========================================================================

    #[test]
    fn test_list_comments_empty() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Review")).unwrap();
        apply_event(&db, &make_thread("th-001", "cr-001", "src/main.rs", 10)).unwrap();

        let comments = db.list_comments("th-001").unwrap();
        assert!(comments.is_empty());
    }

    #[test]
    fn test_list_comments_ordered_by_time() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Review")).unwrap();
        apply_event(&db, &make_thread("th-001", "cr-001", "src/main.rs", 10)).unwrap();
        apply_event(&db, &make_comment("c-001", "th-001", "First")).unwrap();
        apply_event(&db, &make_comment("c-002", "th-001", "Second")).unwrap();
        apply_event(&db, &make_comment("c-003", "th-001", "Third")).unwrap();

        let comments = db.list_comments("th-001").unwrap();
        assert_eq!(comments.len(), 3);
        assert_eq!(comments[0].body, "First");
        assert_eq!(comments[1].body, "Second");
        assert_eq!(comments[2].body, "Third");
    }

    #[test]
    fn test_list_comments_only_for_specified_thread() {
        let db = setup_db();

        apply_event(&db, &make_review("cr-001", "alice", "Review")).unwrap();
        apply_event(&db, &make_thread("th-001", "cr-001", "src/main.rs", 10)).unwrap();
        apply_event(&db, &make_thread("th-002", "cr-001", "src/lib.rs", 20)).unwrap();
        apply_event(&db, &make_comment("c-001", "th-001", "Thread 1 comment")).unwrap();
        apply_event(&db, &make_comment("c-002", "th-002", "Thread 2 comment")).unwrap();

        let comments_1 = db.list_comments("th-001").unwrap();
        assert_eq!(comments_1.len(), 1);
        assert_eq!(comments_1[0].body, "Thread 1 comment");

        let comments_2 = db.list_comments("th-002").unwrap();
        assert_eq!(comments_2.len(), 1);
        assert_eq!(comments_2[0].body, "Thread 2 comment");
    }
}
