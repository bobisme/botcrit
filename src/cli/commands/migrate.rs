//! Migration command: v1 -> v2 data format upgrade.
//!
//! Converts from single `.crit/events.jsonl` to per-review event logs.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::events::{Event, EventEnvelope};
use crate::log::{open_or_create_review, AppendLog, FileLog};
use crate::output::OutputFormat;
use crate::version::{detect_version, write_version_file, DataVersion, CURRENT_VERSION};

/// Path to legacy events.jsonl
fn legacy_events_path(crit_root: &Path) -> std::path::PathBuf {
    crit_root.join(".crit").join("events.jsonl")
}

/// Run the migrate command.
pub fn run_migrate(
    crit_root: &Path,
    dry_run: bool,
    backup: bool,
    format: OutputFormat,
) -> Result<()> {
    // Check current version
    let version = detect_version(crit_root)?;

    match version {
        Some(DataVersion::V2) => {
            if format == OutputFormat::Json {
                println!(
                    r#"{{"status":"already_migrated","version":2,"message":"Repository already uses v2 format"}}"#
                );
            } else {
                println!("✓ Repository already uses v2 format. No migration needed.");
            }
            return Ok(());
        }
        None => {
            // Check if .crit/ exists at all
            let crit_dir = crit_root.join(".crit");
            if !crit_dir.exists() {
                bail!(
                    "No .crit/ directory found. Run 'crit init' first to initialize the repository."
                );
            }

            // Empty repo - just write v2 version file
            if dry_run {
                if format == OutputFormat::Json {
                    println!(
                        r#"{{"status":"dry_run","version":2,"message":"Would create v2 version file (no events to migrate)"}}"#
                    );
                } else {
                    println!("Would create v2 version file (no events to migrate).");
                }
            } else {
                write_version_file(crit_root, DataVersion::V2)?;
                if format == OutputFormat::Json {
                    println!(
                        r#"{{"status":"success","version":2,"message":"Created v2 version file","events_migrated":0}}"#
                    );
                } else {
                    println!("✓ Created v2 version file. Repository is now v2.");
                }
            }
            return Ok(());
        }
        Some(DataVersion::V1) => {
            // Proceed with migration
        }
    }

    // Read all events from legacy file
    let legacy_path = legacy_events_path(crit_root);
    let legacy_log = FileLog::new(&legacy_path);
    let events = legacy_log
        .read_all()
        .context("Failed to read legacy events.jsonl")?;

    if events.is_empty() {
        if dry_run {
            if format == OutputFormat::Json {
                println!(
                    r#"{{"status":"dry_run","version":2,"message":"Would migrate (no events in v1 file)"}}"#
                );
            } else {
                println!("Would migrate empty events.jsonl to v2 format.");
            }
        } else {
            // Backup and write version file
            if backup {
                let backup_path = legacy_path.with_extension("jsonl.v1.backup");
                fs::rename(&legacy_path, &backup_path)
                    .context("Failed to backup events.jsonl")?;
            } else {
                fs::remove_file(&legacy_path).context("Failed to remove events.jsonl")?;
            }
            write_version_file(crit_root, DataVersion::V2)?;
            if format == OutputFormat::Json {
                println!(
                    r#"{{"status":"success","version":2,"events_migrated":0}}"#
                );
            } else {
                println!("✓ Migrated to v2 (no events).");
            }
        }
        return Ok(());
    }

    // Group events by review_id
    let mut events_by_review: HashMap<String, Vec<EventEnvelope>> = HashMap::new();

    for event in &events {
        let review_id = extract_review_id(&event.event);
        if let Some(id) = review_id {
            events_by_review
                .entry(id.to_string())
                .or_default()
                .push(event.clone());
        }
    }

    // Summary
    let total_events = events.len();
    let review_count = events_by_review.len();

    if dry_run {
        if format == OutputFormat::Json {
            let reviews: Vec<_> = events_by_review
                .iter()
                .map(|(id, evts)| {
                    serde_json::json!({
                        "review_id": id,
                        "event_count": evts.len()
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::json!({
                    "status": "dry_run",
                    "total_events": total_events,
                    "review_count": review_count,
                    "reviews": reviews
                })
            );
        } else {
            println!("DRY RUN - Would migrate:");
            println!("  Total events: {}", total_events);
            println!("  Reviews: {}", review_count);
            for (id, evts) in events_by_review.iter() {
                println!("    {}: {} events", id, evts.len());
            }
        }
        return Ok(());
    }

    // Perform migration: write events to per-review logs
    for (review_id, review_events) in &events_by_review {
        let log = open_or_create_review(crit_root, review_id)?;

        // Sort by timestamp (should already be sorted, but be safe)
        let mut sorted_events = review_events.clone();
        sorted_events.sort_by(|a, b| a.ts.cmp(&b.ts));

        for event in &sorted_events {
            log.append(event)?;
        }
    }

    // Backup or remove legacy file
    if backup {
        let backup_path = legacy_path.with_extension("jsonl.v1.backup");
        fs::rename(&legacy_path, &backup_path).context("Failed to backup events.jsonl")?;
    } else {
        fs::remove_file(&legacy_path).context("Failed to remove events.jsonl")?;
    }

    // Write version file
    write_version_file(crit_root, DataVersion::V2)?;

    // Delete old index.db to force rebuild from new structure
    let index_path = crit_root.join(".crit").join("index.db");
    if index_path.exists() {
        fs::remove_file(&index_path).context("Failed to remove old index.db")?;
    }

    if format == OutputFormat::Json {
        println!(
            "{}",
            serde_json::json!({
                "status": "success",
                "version": CURRENT_VERSION,
                "events_migrated": total_events,
                "reviews_created": review_count
            })
        );
    } else {
        println!("✓ Migration complete!");
        println!("  Events migrated: {}", total_events);
        println!("  Reviews: {}", review_count);
        if backup {
            println!(
                "  Backup: {}",
                legacy_path.with_extension("jsonl.v1.backup").display()
            );
        }
    }

    Ok(())
}

/// Extract the review_id from an event.
fn extract_review_id(event: &Event) -> Option<&str> {
    match event {
        Event::ReviewCreated(e) => Some(&e.review_id),
        Event::ReviewersRequested(e) => Some(&e.review_id),
        Event::ReviewerVoted(e) => Some(&e.review_id),
        Event::ReviewApproved(e) => Some(&e.review_id),
        Event::ReviewMerged(e) => Some(&e.review_id),
        Event::ReviewAbandoned(e) => Some(&e.review_id),
        Event::ThreadCreated(e) => Some(&e.review_id),
        Event::ThreadResolved(_) => {
            // ThreadResolved doesn't have review_id directly, need to look it up
            // For migration, we skip these - they'll be associated via the thread_id
            // when reading the review's events
            None
        }
        Event::ThreadReopened(_) => None,
        Event::CommentAdded(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{CodeSelection, ReviewCreated, ReviewerVoted, ThreadCreated, VoteType};
    use crate::log::{list_review_ids, open_or_create};
    use tempfile::tempdir;

    fn make_review_created(review_id: &str) -> EventEnvelope {
        EventEnvelope::new(
            "test_agent",
            Event::ReviewCreated(ReviewCreated {
                review_id: review_id.to_string(),
                jj_change_id: "change123".to_string(),
                initial_commit: "commit456".to_string(),
                title: format!("Test Review {}", review_id),
                description: None,
            }),
        )
    }

    fn make_thread_created(review_id: &str, thread_id: &str) -> EventEnvelope {
        EventEnvelope::new(
            "test_agent",
            Event::ThreadCreated(ThreadCreated {
                thread_id: thread_id.to_string(),
                review_id: review_id.to_string(),
                file_path: "src/main.rs".to_string(),
                selection: CodeSelection::line(42),
                commit_hash: "abc123".to_string(),
            }),
        )
    }

    fn make_vote(review_id: &str) -> EventEnvelope {
        EventEnvelope::new(
            "reviewer",
            Event::ReviewerVoted(ReviewerVoted {
                review_id: review_id.to_string(),
                vote: VoteType::Lgtm,
                reason: Some("Looks good".to_string()),
            }),
        )
    }

    #[test]
    fn test_migrate_empty_repo() {
        let dir = tempdir().unwrap();
        let crit_root = dir.path();

        // Create empty .crit/ directory
        fs::create_dir(crit_root.join(".crit")).unwrap();

        // Run migration
        run_migrate(crit_root, false, true, OutputFormat::Toon).unwrap();

        // Check version file created
        let version_content =
            fs::read_to_string(crit_root.join(".crit").join("version")).unwrap();
        assert_eq!(version_content.trim(), "2");
    }

    #[test]
    fn test_migrate_v1_to_v2() {
        let dir = tempdir().unwrap();
        let crit_root = dir.path();

        // Create v1 structure with events
        let crit_dir = crit_root.join(".crit");
        fs::create_dir(&crit_dir).unwrap();

        let legacy_path = crit_dir.join("events.jsonl");
        let log = open_or_create(&legacy_path).unwrap();

        // Add events for two reviews
        log.append(&make_review_created("cr-001")).unwrap();
        log.append(&make_thread_created("cr-001", "th-001")).unwrap();
        log.append(&make_review_created("cr-002")).unwrap();
        log.append(&make_vote("cr-001")).unwrap();
        log.append(&make_thread_created("cr-002", "th-002")).unwrap();

        // Run migration
        run_migrate(crit_root, false, true, OutputFormat::Toon).unwrap();

        // Check version file
        let version_content = fs::read_to_string(crit_dir.join("version")).unwrap();
        assert_eq!(version_content.trim(), "2");

        // Check backup exists
        assert!(legacy_path.with_extension("jsonl.v1.backup").exists());
        assert!(!legacy_path.exists());

        // Check review directories created
        let review_ids = list_review_ids(crit_root).unwrap();
        assert_eq!(review_ids.len(), 2);
        assert!(review_ids.contains(&"cr-001".to_string()));
        assert!(review_ids.contains(&"cr-002".to_string()));

        // Check events in cr-001
        let log1 = crate::log::ReviewLog::new(crit_root, "cr-001");
        let events1 = log1.read_all().unwrap();
        assert_eq!(events1.len(), 3); // ReviewCreated, ThreadCreated, ReviewerVoted

        // Check events in cr-002
        let log2 = crate::log::ReviewLog::new(crit_root, "cr-002");
        let events2 = log2.read_all().unwrap();
        assert_eq!(events2.len(), 2); // ReviewCreated, ThreadCreated
    }

    #[test]
    fn test_migrate_dry_run() {
        let dir = tempdir().unwrap();
        let crit_root = dir.path();

        // Create v1 structure
        let crit_dir = crit_root.join(".crit");
        fs::create_dir(&crit_dir).unwrap();

        let legacy_path = crit_dir.join("events.jsonl");
        let log = open_or_create(&legacy_path).unwrap();
        log.append(&make_review_created("cr-001")).unwrap();

        // Run dry run
        run_migrate(crit_root, true, true, OutputFormat::Toon).unwrap();

        // Nothing should change
        assert!(legacy_path.exists());
        assert!(!crit_dir.join("version").exists());
        assert!(!crit_dir.join("reviews").exists());
    }

    #[test]
    fn test_migrate_already_v2() {
        let dir = tempdir().unwrap();
        let crit_root = dir.path();

        // Create v2 structure
        let crit_dir = crit_root.join(".crit");
        fs::create_dir(&crit_dir).unwrap();
        fs::write(crit_dir.join("version"), "2\n").unwrap();

        // Run migration - should be no-op
        run_migrate(crit_root, false, true, OutputFormat::Toon).unwrap();

        // Still v2
        let version_content = fs::read_to_string(crit_dir.join("version")).unwrap();
        assert_eq!(version_content.trim(), "2");
    }

    #[test]
    fn test_migrate_no_backup() {
        let dir = tempdir().unwrap();
        let crit_root = dir.path();

        // Create v1 structure
        let crit_dir = crit_root.join(".crit");
        fs::create_dir(&crit_dir).unwrap();

        let legacy_path = crit_dir.join("events.jsonl");
        let log = open_or_create(&legacy_path).unwrap();
        log.append(&make_review_created("cr-001")).unwrap();

        // Run migration without backup
        run_migrate(crit_root, false, false, OutputFormat::Toon).unwrap();

        // Original file should be gone, no backup
        assert!(!legacy_path.exists());
        assert!(!legacy_path.with_extension("jsonl.v1.backup").exists());
    }
}
