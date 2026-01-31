//! Append-only event log for botcrit.
//!
//! Implements the write path of the event sourcing architecture, storing events
//! as JSON Lines in `.crit/events.jsonl` with advisory file locking for
//! concurrent access.

use std::collections::hash_map::DefaultHasher;
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fs2::FileExt;

use crate::events::EventEnvelope;

/// Trait for append-only event log operations.
pub trait AppendLog {
    /// Append an event to the log.
    fn append(&self, event: &EventEnvelope) -> Result<()>;

    /// Read all events from the log.
    fn read_all(&self) -> Result<Vec<EventEnvelope>>;

    /// Read events starting from a line offset (0-indexed).
    fn read_from(&self, line: usize) -> Result<Vec<EventEnvelope>>;

    /// Get the number of events in the log.
    fn len(&self) -> Result<usize>;

    /// Check if the log is empty.
    fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Get the total number of lines in the log, including empty lines.
    ///
    /// Used for truncation detection: if `last_sync_line > total_lines()`,
    /// the file was truncated (e.g., by jj working copy restoration).
    fn total_lines(&self) -> Result<usize> {
        // Default: same as len(). Override for file-based implementations
        // to count all lines including empty ones.
        self.len()
    }

    /// Compute a hash of the first `n` lines for content-change detection.
    ///
    /// Returns `None` if `n == 0` or the log has no content to hash.
    /// Used alongside truncation detection to catch same-length file
    /// replacement (e.g., jj restoring a file with different content
    /// but the same number of lines).
    fn prefix_hash(&self, _n: usize) -> Result<Option<String>> {
        Ok(None)
    }
}

/// File-based implementation of the append-only event log.
///
/// Uses advisory file locking (via `fs2`) to ensure atomic appends from
/// multiple concurrent agents.
#[derive(Debug, Clone)]
pub struct FileLog {
    path: PathBuf,
}

impl FileLog {
    /// Create a new FileLog pointing to the given path.
    ///
    /// Does not create the file; use `open_or_create` for that.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Get the path to the log file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl AppendLog for FileLog {
    fn append(&self, event: &EventEnvelope) -> Result<()> {
        let json_line = event.to_json_line().context("Failed to serialize event")?;

        // Open file for appending
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("Failed to open log file: {}", self.path.display()))?;

        // Acquire exclusive lock for writing
        file.lock_exclusive()
            .context("Failed to acquire exclusive lock")?;

        // Seek to end (should already be there due to append mode, but be explicit)
        file.seek(SeekFrom::End(0))
            .context("Failed to seek to end of file")?;

        // Write the JSON line with newline
        writeln!(file, "{}", json_line).context("Failed to write event to log")?;

        // Flush to ensure data is written
        file.flush().context("Failed to flush log file")?;

        // Lock is automatically released when file is dropped
        Ok(())
    }

    fn read_all(&self) -> Result<Vec<EventEnvelope>> {
        self.read_from(0)
    }

    fn read_from(&self, line: usize) -> Result<Vec<EventEnvelope>> {
        let file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Vec::new());
            }
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to open log file: {}", self.path.display()))
            }
        };

        // Acquire shared lock for reading
        file.lock_shared()
            .context("Failed to acquire shared lock")?;

        let reader = BufReader::new(file);
        let mut events = Vec::new();

        for (idx, line_result) in reader.lines().enumerate() {
            // Skip lines before the offset
            if idx < line {
                continue;
            }

            let line_content = line_result
                .with_context(|| format!("Failed to read line {} from log file", idx))?;

            // Skip empty lines
            if line_content.trim().is_empty() {
                continue;
            }

            let event = EventEnvelope::from_json_line(&line_content)
                .with_context(|| format!("Failed to parse event at line {}", idx))?;

            events.push(event);
        }

        // Lock is automatically released when file is dropped
        Ok(events)
    }

    fn len(&self) -> Result<usize> {
        let file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(0);
            }
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to open log file: {}", self.path.display()))
            }
        };

        // Acquire shared lock for reading
        file.lock_shared()
            .context("Failed to acquire shared lock")?;

        let reader = BufReader::new(file);
        let count = reader
            .lines()
            .filter_map(|l| l.ok())
            .filter(|l| !l.trim().is_empty())
            .count();

        Ok(count)
    }

    fn total_lines(&self) -> Result<usize> {
        let file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(0);
            }
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to open log file: {}", self.path.display()))
            }
        };

        file.lock_shared()
            .context("Failed to acquire shared lock")?;

        let reader = BufReader::new(file);
        let count = reader.lines().filter_map(|l| l.ok()).count();

        Ok(count)
    }

    fn prefix_hash(&self, n: usize) -> Result<Option<String>> {
        if n == 0 {
            return Ok(None);
        }

        let file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to open log file: {}", self.path.display()))
            }
        };

        file.lock_shared()
            .context("Failed to acquire shared lock")?;

        let reader = BufReader::new(file);
        let mut hasher = DefaultHasher::new();

        for (idx, line_result) in reader.lines().enumerate() {
            if idx >= n {
                break;
            }
            let line = line_result
                .with_context(|| format!("Failed to read line {} for hashing", idx))?;
            line.hash(&mut hasher);
        }

        Ok(Some(format!("{:016x}", hasher.finish())))
    }
}

/// Open an existing log file or create a new one.
///
/// Creates parent directories if they don't exist.
/// Creates an empty file if it doesn't exist.
pub fn open_or_create(path: &Path) -> Result<FileLog> {
    // Create parent directories if needed
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create parent directories: {}", parent.display())
            })?;
        }
    }

    // Create empty file if it doesn't exist
    if !path.exists() {
        File::create(path)
            .with_context(|| format!("Failed to create log file: {}", path.display()))?;
    }

    Ok(FileLog::new(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Event, ReviewCreated};
    use tempfile::tempdir;

    fn make_test_event(id: &str) -> EventEnvelope {
        EventEnvelope::new(
            "test_agent",
            Event::ReviewCreated(ReviewCreated {
                review_id: id.to_string(),
                jj_change_id: "change123".to_string(),
                initial_commit: "commit456".to_string(),
                title: format!("Test Review {}", id),
                description: None,
            }),
        )
    }

    #[test]
    fn test_open_or_create_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("deep").join("events.jsonl");

        let log = open_or_create(&path).unwrap();
        assert!(path.exists());
        assert_eq!(log.path(), path);
    }

    #[test]
    fn test_open_or_create_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("events.jsonl");

        // Create file with some content
        std::fs::write(&path, "").unwrap();

        let log = open_or_create(&path).unwrap();
        assert_eq!(log.path(), path);
    }

    #[test]
    fn test_append_and_read_all() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let log = open_or_create(&path).unwrap();

        // Append events
        let event1 = make_test_event("cr-001");
        let event2 = make_test_event("cr-002");

        log.append(&event1).unwrap();
        log.append(&event2).unwrap();

        // Read all events
        let events = log.read_all().unwrap();
        assert_eq!(events.len(), 2);

        match &events[0].event {
            Event::ReviewCreated(r) => assert_eq!(r.review_id, "cr-001"),
            _ => panic!("Expected ReviewCreated"),
        }

        match &events[1].event {
            Event::ReviewCreated(r) => assert_eq!(r.review_id, "cr-002"),
            _ => panic!("Expected ReviewCreated"),
        }
    }

    #[test]
    fn test_read_from_offset() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let log = open_or_create(&path).unwrap();

        // Append 5 events
        for i in 1..=5 {
            log.append(&make_test_event(&format!("cr-{:03}", i)))
                .unwrap();
        }

        // Read from offset 2 (should get events 3, 4, 5)
        let events = log.read_from(2).unwrap();
        assert_eq!(events.len(), 3);

        match &events[0].event {
            Event::ReviewCreated(r) => assert_eq!(r.review_id, "cr-003"),
            _ => panic!("Expected ReviewCreated"),
        }

        match &events[2].event {
            Event::ReviewCreated(r) => assert_eq!(r.review_id, "cr-005"),
            _ => panic!("Expected ReviewCreated"),
        }
    }

    #[test]
    fn test_read_from_beyond_end() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let log = open_or_create(&path).unwrap();

        log.append(&make_test_event("cr-001")).unwrap();

        // Read from offset beyond the file
        let events = log.read_from(100).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_len() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let log = open_or_create(&path).unwrap();

        assert_eq!(log.len().unwrap(), 0);

        log.append(&make_test_event("cr-001")).unwrap();
        assert_eq!(log.len().unwrap(), 1);

        log.append(&make_test_event("cr-002")).unwrap();
        log.append(&make_test_event("cr-003")).unwrap();
        assert_eq!(log.len().unwrap(), 3);
    }

    #[test]
    fn test_is_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let log = open_or_create(&path).unwrap();

        assert!(log.is_empty().unwrap());

        log.append(&make_test_event("cr-001")).unwrap();
        assert!(!log.is_empty().unwrap());
    }

    #[test]
    fn test_read_nonexistent_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("does_not_exist.jsonl");
        let log = FileLog::new(&path);

        // Should return empty vec, not error
        let events = log.read_all().unwrap();
        assert!(events.is_empty());

        assert_eq!(log.len().unwrap(), 0);
    }

    #[test]
    fn test_append_creates_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("new_file.jsonl");
        let log = FileLog::new(&path);

        assert!(!path.exists());

        log.append(&make_test_event("cr-001")).unwrap();

        assert!(path.exists());
        assert_eq!(log.len().unwrap(), 1);
    }

    #[test]
    fn test_file_format_is_jsonl() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let log = open_or_create(&path).unwrap();

        log.append(&make_test_event("cr-001")).unwrap();
        log.append(&make_test_event("cr-002")).unwrap();

        // Read raw file content
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();

        assert_eq!(lines.len(), 2);

        // Each line should be valid JSON
        for line in &lines {
            let _: serde_json::Value = serde_json::from_str(line).unwrap();
        }

        // First line should contain cr-001
        assert!(lines[0].contains("cr-001"));
        // Second line should contain cr-002
        assert!(lines[1].contains("cr-002"));
    }
}
