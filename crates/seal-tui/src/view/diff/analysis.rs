//! Thread-to-diff mapping, change counting, and thread range analysis.

use crate::db::ThreadSummary;
use crate::diff::{DiffLineKind, ParsedDiff};

use super::{ChangeCounts, ThreadAnchor};

pub fn diff_change_counts(diff: &ParsedDiff) -> ChangeCounts {
    let mut added = 0usize;
    let mut removed = 0usize;
    for hunk in &diff.hunks {
        for line in &hunk.lines {
            match line.kind {
                DiffLineKind::Added => added += 1,
                DiffLineKind::Removed => removed += 1,
                DiffLineKind::Context => {}
            }
        }
    }
    ChangeCounts { added, removed }
}

/// Map threads to display line indices within the diff
#[must_use]
pub fn map_threads_to_diff(diff: &ParsedDiff, threads: &[&ThreadSummary]) -> Vec<ThreadAnchor> {
    let mut anchors = Vec::new();

    // Build maps from line numbers to display line index
    // Check both old and new line numbers since threads could reference either
    let mut new_line_to_display: std::collections::HashMap<u32, usize> =
        std::collections::HashMap::new();
    let mut display_idx = 0;

    for hunk in &diff.hunks {
        display_idx += 1; // hunk header
        for line in &hunk.lines {
            if let Some(new_ln) = line.new_line {
                new_line_to_display.insert(new_ln, display_idx);
            }
            display_idx += 1;
        }
    }

    // Map each thread to its display position
    // Only anchor on new-file line numbers — old-line fallback causes false
    // anchoring when a thread's line number coincidentally matches a removed line
    // in a different commit.
    for thread in threads {
        let start_line = thread.selection_start as u32;
        let display_line = new_line_to_display.get(&start_line);

        if let Some(&display_line) = display_line {
            let line_count = thread
                .selection_end
                .map_or(1, |end| (end - thread.selection_start + 1) as usize);

            // Comment block goes after the last line of the range
            let end_line = thread.selection_end.unwrap_or(thread.selection_start) as u32;
            let comment_after_line = new_line_to_display
                .get(&end_line)
                .copied()
                .unwrap_or(display_line);

            anchors.push(ThreadAnchor {
                thread_id: thread.thread_id.clone(),
                display_line,
                comment_after_line,
                line_count,
                status: thread.status.clone(),
                comment_count: thread.comment_count,
                is_expanded: true,
            });
        }
    }

    // Sort by display line
    anchors.sort_by_key(|a| a.display_line);
    anchors
}

pub(super) fn build_thread_ranges(threads: &[&ThreadSummary]) -> Vec<(i64, i64)> {
    threads
        .iter()
        .map(|thread| {
            let end = thread.selection_end.unwrap_or(thread.selection_start);
            (
                thread.selection_start.min(end),
                thread.selection_start.max(end),
            )
        })
        .collect()
}

pub(super) fn line_in_thread_ranges(line: Option<i64>, ranges: &[(i64, i64)]) -> bool {
    let Some(line) = line else {
        return false;
    };
    ranges
        .iter()
        .any(|(start, end)| line >= *start && line <= *end)
}
