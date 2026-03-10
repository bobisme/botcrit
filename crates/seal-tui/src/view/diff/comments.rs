//! Comment block rendering (thread comment bubbles in the diff stream).

use crate::render_backend::{buffer_draw_text, buffer_fill_rect, color_lerp, Style};

use crate::db::ThreadSummary;
use crate::layout::BLOCK_PADDING;
use crate::markdown::{
    draw_markdown_content, markdown_line_bg, render_markdown, MarkdownContent, MarkdownStyle,
};
use crate::view::components::Rect;

use super::helpers::{
    comment_block_area, comment_content_area, draw_plain_line_with_right, PlainLineContent,
};
use super::StreamCursor;

#[derive(Clone)]
pub(super) enum CommentLineKind {
    Header,
    Author,
    Markdown(MarkdownStyle),
}

#[derive(Clone)]
pub(super) struct CommentLine {
    pub content: MarkdownContent,
    pub right: Option<String>,
    pub kind: CommentLineKind,
}

fn build_comment_lines(
    thread: &ThreadSummary,
    comments: &[crate::db::Comment],
    content_width: usize,
) -> Vec<CommentLine> {
    let mut content_lines: Vec<CommentLine> = Vec::new();

    let line_range = thread.selection_end.map_or_else(
        || format!("{}", thread.selection_start),
        |end| format!("{}-{}", thread.selection_start, end),
    );
    let mut right_text = format!("{}:{}", thread.file_path, line_range);
    let right_max = content_width.saturating_sub(thread.thread_id.len().saturating_add(1));
    if right_max > 0 && right_text.len() > right_max {
        right_text = crate::view::components::truncate_path(&right_text, right_max);
    } else if right_max == 0 {
        right_text.clear();
    }
    content_lines.push(CommentLine {
        content: MarkdownContent::Text(thread.thread_id.clone()),
        right: if right_text.is_empty() {
            None
        } else {
            Some(right_text)
        },
        kind: CommentLineKind::Header,
    });
    content_lines.push(CommentLine {
        content: MarkdownContent::Text(String::new()),
        right: None,
        kind: CommentLineKind::Markdown(MarkdownStyle::Body),
    });

    for (index, comment) in comments.iter().enumerate() {
        let left = format!("@{}", comment.author);
        let right_max = content_width.saturating_sub(left.len().saturating_add(1));
        let right = if right_max > 0 {
            let mut id = comment.comment_id.clone();
            if id.len() > right_max {
                id.truncate(right_max);
            }
            Some(id)
        } else {
            None
        };
        content_lines.push(CommentLine {
            content: MarkdownContent::Text(left),
            right,
            kind: CommentLineKind::Author,
        });
        let rendered = render_markdown(&comment.body, content_width);
        for line in rendered {
            content_lines.push(CommentLine {
                content: line.content,
                right: None,
                kind: CommentLineKind::Markdown(line.style),
            });
        }
        if index + 1 < comments.len() {
            content_lines.push(CommentLine {
                content: MarkdownContent::Text(String::new()),
                right: None,
                kind: CommentLineKind::Markdown(MarkdownStyle::Body),
            });
        }
    }

    content_lines
}

/// Compute the total row height of a comment block (for cursor range checks).
pub(super) fn comment_block_rows(
    thread: &ThreadSummary,
    comments: &[crate::db::Comment],
    area: Rect,
) -> usize {
    if comments.is_empty() {
        return 0;
    }
    let padded = comment_content_area(comment_block_area(area));
    let content_width = padded.width as usize;
    let content_lines = build_comment_lines(thread, comments, content_width);
    let content_start = BLOCK_PADDING;
    let content_end = content_start + content_lines.len();
    content_end.saturating_add(BLOCK_PADDING)
}

pub(super) fn emit_comment_block(
    cursor: &mut StreamCursor<'_>,
    area: Rect,
    thread: &ThreadSummary,
    comments: &[crate::db::Comment],
    is_highlighted: bool,
    is_cursor: bool,
) {
    if comments.is_empty() {
        return;
    }

    // Layout: area → block (margined) → padded content
    let block = comment_block_area(area);
    let padded = comment_content_area(block);
    let content_width = padded.width as usize;
    let content_lines = build_comment_lines(thread, comments, content_width);

    let top_margin = 0usize;
    let bottom_margin = 0usize;
    let content_start = top_margin + BLOCK_PADDING;
    let content_end = content_start + content_lines.len();
    let total_rows = content_end
        .saturating_add(BLOCK_PADDING)
        .saturating_add(bottom_margin);

    for row in 0..total_rows {
        cursor.mark_cursor_stop();
        cursor.emit(|buf, y, theme| {
            let base_bg = if is_highlighted {
                color_lerp(theme.panel_bg, theme.primary, 0.03)
            } else {
                theme.panel_bg
            };
            let block_bg = if is_cursor {
                color_lerp(base_bg, theme.primary, 0.08)
            } else {
                base_bg
            };
            let border_style = Style::fg(theme.background).with_bg(block_bg);
            let bar_style = Style::fg(theme.background).with_bg(block_bg);
            let rc = block.x + block.width.saturating_sub(1);
            let rc2 = block.x + block.width.saturating_sub(2);
            if row < top_margin {
                buffer_fill_rect(buf, area.x, y, area.width, 1, theme.background);
            } else if row == top_margin {
                // Top border:  ▛▀…▀▜  (outer corners match window bg)
                buffer_fill_rect(buf, area.x, y, area.width, 1, theme.background);
                buffer_fill_rect(
                    buf,
                    block.x + 1,
                    y,
                    block.width.saturating_sub(2),
                    1,
                    block_bg,
                );
                buffer_draw_text(buf, block.x + 1, y, "▛", border_style);
                for col in 2..block.width.saturating_sub(2) {
                    buffer_draw_text(buf, block.x + col, y, "▀", border_style);
                }
                buffer_draw_text(buf, rc2, y, "▜", border_style);
            } else if row < content_start {
                buffer_fill_rect(buf, area.x, y, area.width, 1, theme.background);
                buffer_fill_rect(buf, block.x, y, block.width, 1, block_bg);
                buffer_draw_text(buf, block.x, y, "▌", bar_style);
                buffer_draw_text(buf, block.x + 1, y, "▌", bar_style);
                buffer_draw_text(buf, rc2, y, "▐", bar_style);
                buffer_draw_text(buf, rc, y, "▐", bar_style);
            } else if row < content_end {
                let line = &content_lines[row - content_start];
                let markdown_style = match line.kind {
                    CommentLineKind::Header => MarkdownStyle::Body,
                    CommentLineKind::Author => MarkdownStyle::Heading,
                    CommentLineKind::Markdown(style) => style,
                };
                let line_bg = markdown_line_bg(theme, block_bg, markdown_style);
                let (left_style, right_style) = match line.kind {
                    CommentLineKind::Header => {
                        (theme.style_muted_on(line_bg), theme.style_muted_on(line_bg))
                    }
                    CommentLineKind::Author => (
                        theme.style_primary_on(line_bg),
                        theme.style_muted_on(line_bg),
                    ),
                    CommentLineKind::Markdown(style) => {
                        (style.style(theme, line_bg), theme.style_muted_on(line_bg))
                    }
                };
                buffer_fill_rect(buf, area.x, y, area.width, 1, theme.background);
                buffer_fill_rect(buf, block.x, y, block.width, 1, block_bg);
                buffer_fill_rect(buf, padded.x, y, padded.width, 1, line_bg);
                buffer_draw_text(buf, block.x, y, "▌", bar_style);
                buffer_draw_text(buf, block.x + 1, y, "▌", bar_style);
                buffer_draw_text(buf, rc2, y, "▐", bar_style);
                buffer_draw_text(buf, rc, y, "▐", bar_style);
                match &line.content {
                    MarkdownContent::Text(text) if line.right.is_some() => {
                        draw_plain_line_with_right(
                            buf,
                            padded,
                            y,
                            line_bg,
                            &PlainLineContent {
                                left: text,
                                right: line.right.as_deref(),
                                left_style,
                                right_style,
                            },
                        );
                    }
                    _ => {
                        draw_markdown_content(
                            buf,
                            theme,
                            padded.x,
                            y,
                            padded.width,
                            line_bg,
                            &line.content,
                            markdown_style,
                        );
                        if let Some(right) = line.right.as_deref() {
                            draw_plain_line_with_right(
                                buf,
                                padded,
                                y,
                                line_bg,
                                &PlainLineContent {
                                    left: "",
                                    right: Some(right),
                                    left_style,
                                    right_style,
                                },
                            );
                        }
                    }
                }
            } else if row < content_end + BLOCK_PADDING {
                buffer_fill_rect(buf, area.x, y, area.width, 1, theme.background);
                if row == content_end + BLOCK_PADDING - 1 {
                    // Bottom border:  ▙▄…▄▟  (outer corners match window bg)
                    buffer_fill_rect(
                        buf,
                        block.x + 1,
                        y,
                        block.width.saturating_sub(2),
                        1,
                        block_bg,
                    );
                    buffer_draw_text(buf, block.x + 1, y, "▙", border_style);
                    for col in 2..block.width.saturating_sub(2) {
                        buffer_draw_text(buf, block.x + col, y, "▄", border_style);
                    }
                    buffer_draw_text(buf, rc2, y, "▟", border_style);
                } else {
                    buffer_fill_rect(buf, block.x, y, block.width, 1, block_bg);
                    buffer_draw_text(buf, block.x, y, "▌", bar_style);
                    buffer_draw_text(buf, block.x + 1, y, "▌", bar_style);
                    buffer_draw_text(buf, rc2, y, "▐", bar_style);
                    buffer_draw_text(buf, rc, y, "▐", bar_style);
                }
            } else {
                buffer_fill_rect(buf, area.x, y, area.width, 1, theme.background);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(id: &str, author: &str, body: &str) -> crate::db::Comment {
        crate::db::Comment {
            comment_id: id.to_string(),
            author: author.to_string(),
            body: body.to_string(),
            created_at: "2026-03-10T00:00:00Z".to_string(),
        }
    }

    fn thread() -> ThreadSummary {
        ThreadSummary {
            thread_id: "th-1234".to_string(),
            file_path: "src/lib.rs".to_string(),
            selection_start: 10,
            selection_end: None,
            status: "open".to_string(),
            comment_count: 2,
        }
    }

    #[test]
    fn build_comment_lines_inserts_blank_row_between_comments() {
        let lines = build_comment_lines(
            &thread(),
            &[
                comment("th-1234.1", "alice", "first"),
                comment("th-1234.2", "bob", "second"),
            ],
            40,
        );

        let blanks = lines
            .iter()
            .filter(|line| matches!(&line.content, MarkdownContent::Text(text) if text.is_empty()))
            .count();
        assert!(blanks >= 2);
    }
}
