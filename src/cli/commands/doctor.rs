//! Implementation of `crit doctor` health check command.

use anyhow::Result;
use serde::Serialize;
use std::path::Path;
use std::process::Command;

use crate::cli::commands::init::{events_path, index_path, is_initialized};
use crate::events::EventEnvelope;
use crate::log::open_or_create;
use crate::output::{Formatter, OutputFormat};
use crate::projection::{sync_from_log, ProjectionDb};

/// Result of a single health check.
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub status: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

impl CheckResult {
    fn pass(name: &str, message: &str) -> Self {
        Self {
            name: name.to_string(),
            status: "pass".to_string(),
            message: message.to_string(),
            remediation: None,
        }
    }

    fn fail(name: &str, message: &str, remediation: &str) -> Self {
        Self {
            name: name.to_string(),
            status: "fail".to_string(),
            message: message.to_string(),
            remediation: Some(remediation.to_string()),
        }
    }

    fn warn(name: &str, message: &str, remediation: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            status: "warn".to_string(),
            message: message.to_string(),
            remediation: remediation.map(ToString::to_string),
        }
    }
}

/// Overall health status.
#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    pub healthy: bool,
    pub checks: Vec<CheckResult>,
}

/// Run the doctor health check.
pub fn run_doctor(repo_root: &Path, format: OutputFormat) -> Result<()> {
    let mut checks = Vec::new();

    // Check 1: jj installed
    checks.push(check_jj_installed());

    // Check 2: jj repo
    checks.push(check_jj_repo(repo_root));

    // Check 3: .crit directory
    checks.push(check_crit_initialized(repo_root));

    // Check 4: events.jsonl parseable (only if initialized)
    if is_initialized(repo_root) {
        checks.push(check_events_parseable(repo_root));

        // Check 5: index.db sync status
        checks.push(check_index_sync(repo_root));
    }

    let healthy = checks.iter().all(|c| c.status != "fail");

    let report = HealthReport { healthy, checks };

    let formatter = Formatter::new(format);
    formatter.print(&report)?;

    // Exit with error code if unhealthy
    if !healthy {
        std::process::exit(1);
    }

    Ok(())
}

/// Check if jj is installed.
fn check_jj_installed() -> CheckResult {
    match Command::new("jj").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            let version = version.trim();
            CheckResult::pass("jj_installed", &format!("jj is installed: {}", version))
        }
        Ok(_) => CheckResult::fail(
            "jj_installed",
            "jj command failed",
            "Install jj: https://github.com/martinvonz/jj",
        ),
        Err(_) => CheckResult::fail(
            "jj_installed",
            "jj is not installed",
            "Install jj: https://github.com/martinvonz/jj",
        ),
    }
}

/// Check if current directory is a jj repo.
fn check_jj_repo(repo_root: &Path) -> CheckResult {
    let jj_dir = repo_root.join(".jj");
    if jj_dir.exists() && jj_dir.is_dir() {
        CheckResult::pass("jj_repo", "Current directory is a jj repository")
    } else {
        CheckResult::fail(
            "jj_repo",
            "Not a jj repository",
            "Run 'jj git init' to initialize a jj repository",
        )
    }
}

/// Check if crit is initialized.
fn check_crit_initialized(repo_root: &Path) -> CheckResult {
    if is_initialized(repo_root) {
        let crit_dir = repo_root.join(".crit");
        let events = crit_dir.join("events.jsonl");
        let index = crit_dir.join("index.db");

        let mut details = vec![".crit/ exists"];
        if events.exists() {
            details.push("events.jsonl present");
        }
        if index.exists() {
            details.push("index.db present");
        }

        CheckResult::pass("crit_initialized", &details.join(", "))
    } else {
        CheckResult::fail(
            "crit_initialized",
            ".crit directory not found",
            "Run 'crit init' to initialize crit in this repository",
        )
    }
}

/// Check if events.jsonl is parseable.
fn check_events_parseable(repo_root: &Path) -> CheckResult {
    let events_file = events_path(repo_root);

    match std::fs::read_to_string(&events_file) {
        Ok(contents) => {
            let mut valid_count = 0;
            let mut errors = Vec::new();

            for (i, line) in contents.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                match EventEnvelope::from_json_line(line) {
                    Ok(_) => valid_count += 1,
                    Err(e) => {
                        errors.push(format!("Line {}: {}", i + 1, e));
                        if errors.len() >= 3 {
                            break;
                        }
                    }
                }
            }

            if errors.is_empty() {
                CheckResult::pass(
                    "events_parseable",
                    &format!("events.jsonl is valid ({} events)", valid_count),
                )
            } else {
                CheckResult::fail(
                    "events_parseable",
                    &format!(
                        "events.jsonl has {} parse error(s): {}",
                        errors.len(),
                        errors.join("; ")
                    ),
                    "Fix the malformed JSON lines or restore from backup",
                )
            }
        }
        Err(e) => CheckResult::fail(
            "events_parseable",
            &format!("Cannot read events.jsonl: {}", e),
            "Check file permissions or run 'crit init' to recreate",
        ),
    }
}

/// Check if index.db is in sync with events.jsonl.
fn check_index_sync(repo_root: &Path) -> CheckResult {
    let db_result = ProjectionDb::open(&index_path(repo_root));
    let log_result = open_or_create(&events_path(repo_root));

    match (db_result, log_result) {
        (Ok(db), Ok(log)) => {
            // Initialize schema if needed
            if let Err(e) = db.init_schema() {
                return CheckResult::fail(
                    "index_sync",
                    &format!("Failed to initialize schema: {}", e),
                    "Delete .crit/index.db and it will be recreated",
                );
            }

            // Try to sync and check for errors
            match sync_from_log(&db, &log) {
                Ok(events_processed) => {
                    // Get some stats
                    let review_count = db.list_reviews(None, None).map(|r| r.len()).unwrap_or(0);
                    CheckResult::pass(
                        "index_sync",
                        &format!(
                            "index.db is in sync ({} reviews, {} events)",
                            review_count, events_processed
                        ),
                    )
                }
                Err(e) => CheckResult::warn(
                    "index_sync",
                    &format!("Sync completed with warning: {}", e),
                    Some("This may indicate corrupted events or schema mismatch"),
                ),
            }
        }
        (Err(e), _) => CheckResult::fail(
            "index_sync",
            &format!("Cannot open index.db: {}", e),
            "Delete .crit/index.db and it will be recreated on next command",
        ),
        (_, Err(e)) => CheckResult::fail(
            "index_sync",
            &format!("Cannot open events.jsonl: {}", e),
            "Check file permissions",
        ),
    }
}
