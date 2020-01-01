//! Centralized layout constants and derived width functions.
//!
//! All magic numbers for block, diff, comment, and side-by-side layout live
//! here so they stay in sync between the rendering layer (`view/diff`) and
//! the stream-layout calculator (`stream.rs`).

// --- Block constants (file headers, pinned headers, comment blocks) ---

pub const BLOCK_MARGIN: usize = 1;
pub const BLOCK_PADDING: usize = 1;
pub const BLOCK_SIDE_MARGIN: u32 = 2;
pub const BLOCK_LEFT_PAD: u32 = 2;
pub const BLOCK_RIGHT_PAD: u32 = 2;

/// Minimum terminal width before we switch from SBS to unified.
pub const SIDE_BY_SIDE_MIN_WIDTH: u32 = 100;

// --- Diff constants ---

pub const DIFF_H_PAD: u32 = 2;
pub const DIFF_MARGIN: u32 = 0;
pub const ORPHANED_CONTEXT_LEFT_PAD: u32 = 2;

// --- Thread / line-number column widths ---

pub const THREAD_COL_WIDTH: u32 = 0;
pub const UNIFIED_LINE_NUM_WIDTH: u32 = 12;
pub const SBS_LINE_NUM_WIDTH: u32 = 6;
pub const CONTEXT_LINE_NUM_WIDTH: u32 = 6;

// --- Comment layout ---

pub const COMMENT_H_MARGIN: u32 = 4;
pub const COMMENT_H_PAD: u32 = 1;

// --- Context lines around threads ---

pub const CONTEXT_LINES: i64 = 5;

// --- Block height ---

#[must_use]
pub const fn block_height(content_lines: usize) -> usize {
    content_lines + (BLOCK_MARGIN * 2) + (BLOCK_PADDING * 2)
}

/// Number of stream rows visible in the diff pane.
///
/// Accounts for the help bar footer (2 lines + 1 margin = 3) and the pinned
/// header block at the top of the stream area.
#[must_use]
pub const fn visible_stream_rows(terminal_height: u16) -> usize {
    let total = terminal_height as u32;
    let footer: u32 = 3; // help bar (2 lines) + margin (1 line)
    let pinned: u32 = block_height(1) as u32;
    total.saturating_sub(footer + pinned) as usize
}

// --- Stream-layout inner-width helpers ---

/// Inner width for diff content (no block bar/margins, just horizontal padding).
#[must_use]
pub const fn diff_inner_width(pane_width: u32) -> u32 {
    pane_width.saturating_sub(DIFF_H_PAD * 2)
}

/// Inner width for block content (file headers, comment blocks, description).
/// Accounts for: side margins, bar character, and internal padding.
#[must_use]
pub const fn block_inner_width(pane_width: u32) -> u32 {
    pane_width.saturating_sub(BLOCK_SIDE_MARGIN * 2 + 1 + BLOCK_LEFT_PAD + BLOCK_RIGHT_PAD)
}
