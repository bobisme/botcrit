//! Unified diff parser and rendering

mod parse;

pub use parse::{hunk_exclusion_ranges, DiffHunk, DiffLine, DiffLineKind, ParsedDiff};
