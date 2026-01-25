-- botcrit SQLite Schema for Projection Engine
-- This stores the projected state from events.jsonl (not the events themselves).
-- The index.db is ephemeral and can be rebuilt from the event log at any time.

--------------------------------------------------------------------------------
-- SYNC STATE
--------------------------------------------------------------------------------

-- Tracks the last successfully processed line from events.jsonl.
-- Enables incremental sync: on startup, read only new lines from offset.
-- Single-row table pattern: id=1 is the only row.
CREATE TABLE sync_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_line_number INTEGER NOT NULL DEFAULT 0,
    last_sync_ts TEXT,  -- ISO8601 timestamp of last sync
    events_file_hash TEXT  -- Optional: detect if file was truncated/replaced
);

-- Initialize with default row
INSERT INTO sync_state (id, last_line_number) VALUES (1, 0);

--------------------------------------------------------------------------------
-- REVIEWS
--------------------------------------------------------------------------------

-- Core review entity. One row per ReviewCreated event.
-- Status is computed from subsequent events (ReviewMerged, ReviewAbandoned).
CREATE TABLE reviews (
    review_id TEXT PRIMARY KEY,
    jj_change_id TEXT NOT NULL,      -- Stable jj change ID (survives rebases)
    initial_commit TEXT NOT NULL,    -- Commit hash when review was created
    final_commit TEXT,               -- Set on merge
    title TEXT NOT NULL,
    description TEXT,
    author TEXT NOT NULL,
    created_at TEXT NOT NULL,        -- ISO8601 timestamp
    
    -- Derived status: 'open', 'approved', 'merged', 'abandoned'
    -- Updated by ReviewApproved, ReviewMerged, ReviewAbandoned events
    status TEXT NOT NULL DEFAULT 'open' 
        CHECK (status IN ('open', 'approved', 'merged', 'abandoned')),
    
    -- Denormalized for queries: when was status last changed?
    status_changed_at TEXT,
    status_changed_by TEXT,
    abandon_reason TEXT              -- Set on ReviewAbandoned
);

-- For listing reviews by status (most common query)
CREATE INDEX idx_reviews_status ON reviews(status);

-- For filtering by author
CREATE INDEX idx_reviews_author ON reviews(author);

-- For jj integration: lookup review by change_id
CREATE INDEX idx_reviews_change_id ON reviews(jj_change_id);

--------------------------------------------------------------------------------
-- REVIEWERS
--------------------------------------------------------------------------------

-- Many-to-many: reviewers requested for a review.
-- One row per reviewer per ReviewersRequested event batch.
-- Could have multiple rows if reviewers are requested in batches.
CREATE TABLE review_reviewers (
    review_id TEXT NOT NULL REFERENCES reviews(review_id),
    reviewer TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    requested_by TEXT NOT NULL,
    PRIMARY KEY (review_id, reviewer)
);

CREATE INDEX idx_reviewers_reviewer ON review_reviewers(reviewer);

--------------------------------------------------------------------------------
-- THREADS
--------------------------------------------------------------------------------

-- Comment threads anchored to specific file locations.
-- Each thread is created at a specific commit snapshot.
CREATE TABLE threads (
    thread_id TEXT PRIMARY KEY,
    review_id TEXT NOT NULL REFERENCES reviews(review_id),
    file_path TEXT NOT NULL,
    
    -- Selection anchor (stored as original location at commit_hash)
    -- selection_type: 'line' or 'range'
    selection_type TEXT NOT NULL CHECK (selection_type IN ('line', 'range')),
    selection_start INTEGER NOT NULL,  -- Start line (or single line)
    selection_end INTEGER,             -- End line for range, NULL for single line
    
    -- The commit hash where this thread was anchored
    -- Used for drift detection: compare original vs current
    commit_hash TEXT NOT NULL,
    
    author TEXT NOT NULL,
    created_at TEXT NOT NULL,
    
    -- Status: 'open' or 'resolved'
    -- Updated by ThreadResolved, ThreadReopened events
    status TEXT NOT NULL DEFAULT 'open' 
        CHECK (status IN ('open', 'resolved')),
    
    -- Last status change metadata
    status_changed_at TEXT,
    status_changed_by TEXT,
    resolve_reason TEXT,             -- From ThreadResolved
    reopen_reason TEXT               -- From ThreadReopened
);

-- Primary query: threads for a review (with optional status/file filters)
CREATE INDEX idx_threads_review_id ON threads(review_id);

-- Filter by status (unresolved threads query)
CREATE INDEX idx_threads_status ON threads(status);

-- Filter by file path within a review
CREATE INDEX idx_threads_review_file ON threads(review_id, file_path);

-- Composite for common query: open threads in a review
CREATE INDEX idx_threads_review_status ON threads(review_id, status);

--------------------------------------------------------------------------------
-- COMMENTS
--------------------------------------------------------------------------------

-- Individual comments within threads.
-- Append-only from CommentAdded events.
CREATE TABLE comments (
    comment_id TEXT PRIMARY KEY,
    thread_id TEXT NOT NULL REFERENCES threads(thread_id),
    body TEXT NOT NULL,
    author TEXT NOT NULL,
    created_at TEXT NOT NULL,
    
    -- For idempotency: optional client-provided request ID
    -- Duplicate request_ids are rejected during event processing
    request_id TEXT UNIQUE
);

-- Primary query: comments for a thread (ordered by time)
CREATE INDEX idx_comments_thread_id ON comments(thread_id);

-- Idempotency check: lookup by request_id
-- The UNIQUE constraint on request_id handles this, but explicit index helps
CREATE INDEX idx_comments_request_id ON comments(request_id) WHERE request_id IS NOT NULL;

--------------------------------------------------------------------------------
-- DENORMALIZED COUNTS (Optional Optimization)
--------------------------------------------------------------------------------

-- These could be maintained by triggers or computed on-the-fly.
-- For a v1, we might skip these and just COUNT(*) as needed.
-- Including as a design note for future optimization.

-- CREATE TABLE review_stats (
--     review_id TEXT PRIMARY KEY REFERENCES reviews(review_id),
--     thread_count INTEGER DEFAULT 0,
--     open_thread_count INTEGER DEFAULT 0,
--     comment_count INTEGER DEFAULT 0
-- );

--------------------------------------------------------------------------------
-- VIEWS (Convenience for Common Queries)
--------------------------------------------------------------------------------

-- Active reviews with thread counts
CREATE VIEW v_reviews_summary AS
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

-- Thread detail with comment count
CREATE VIEW v_threads_detail AS
SELECT 
    t.*,
    r.title AS review_title,
    COUNT(c.comment_id) AS comment_count
FROM threads t
JOIN reviews r ON r.review_id = t.review_id
LEFT JOIN comments c ON c.thread_id = t.thread_id
GROUP BY t.thread_id;

--------------------------------------------------------------------------------
-- DESIGN NOTES
--------------------------------------------------------------------------------

-- 1. No foreign key enforcement at runtime (for performance).
--    Integrity is guaranteed by event processing logic.
--    Enable with: PRAGMA foreign_keys = ON; if desired.

-- 2. Timestamps are stored as TEXT in ISO8601 format.
--    SQLite's datetime functions work with this format.
--    Alternative: store as INTEGER (Unix epoch) for sorting efficiency.

-- 3. The schema is optimized for read-heavy workloads.
--    Writes only happen during sync from events.jsonl.

-- 4. Drift detection is NOT stored in the database.
--    current_line is computed at query time by diffing commit_hash vs HEAD.
--    This avoids stale data and keeps the schema simpler.

-- 5. request_id in comments enables idempotent writes.
--    During event processing, we can check: INSERT OR IGNORE based on request_id.

-- 6. Status enums are stored as TEXT for readability.
--    If performance becomes an issue, migrate to INTEGER codes.

-- 7. Rebuild procedure: DELETE FROM all tables, reset sync_state,
--    then replay events.jsonl from line 0.
