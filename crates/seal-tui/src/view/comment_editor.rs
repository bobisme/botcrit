//! Inline multi-line comment editor overlay.
//!
//! Renders a bottom-pinned modal centered on the diff pane:
//! - Dimmed background
//! - Text area with existing comments context
//! - Bottom bar with title (left) and hotkeys (right)

use crate::db::Comment;
use crate::markdown::{
    draw_markdown_content, markdown_line_bg, render_markdown, render_markdown_with_highlighter,
};
use crate::render_backend::{buffer_draw_text, buffer_fill_rect, OptimizedBuffer, Style};

use crate::model::{Focus, InlineEditor, Model};
use crate::theme::Theme;
use crate::view::components::{dim_rect, draw_help_bar_ext, draw_text_truncated, HotkeyHint, Rect};

/// Minimum editor panel height.
const MIN_HEIGHT: u32 = 8;
/// Below this diff-pane width the panel spans the full screen.
const MIN_WIDTH: u32 = 60;
/// Horizontal padding inside the panel.
const H_PAD: u32 = 2;
const COMMENT_BODY_INDENT: u32 = 2;
const MAX_CONTEXT_ROWS: u32 = 10;
const MAX_CONTEXT_COMMENTS: usize = 4;

pub fn view(model: &Model, buffer: &mut OptimizedBuffer) {
    if model.focus != Focus::Commenting {
        return;
    }
    let Some(editor) = &model.inline_editor else {
        return;
    };

    let screen = Rect::from_size(model.width, model.height);
    dim_rect(buffer, screen, 0.6);

    // Compute diff pane region for centering
    let sidebar_w = if model.sidebar_visible {
        u32::from(model.layout_mode.sidebar_width())
    } else {
        0
    };
    let diff_pane_x = sidebar_w;
    let diff_pane_width = u32::from(model.width).saturating_sub(sidebar_w);

    let panel = compute_panel(screen, editor, diff_pane_x, diff_pane_width);

    // Fill panel background
    buffer_fill_rect(
        buffer,
        panel.x,
        panel.y,
        panel.width,
        panel.height,
        model.theme.panel_bg,
    );

    let content_x = panel.x + H_PAD;
    let content_width = panel.width.saturating_sub(H_PAD * 2);

    let mut y = panel.y + 1;

    // --- Existing comments context (dimmed) ---
    y = render_existing_comments(
        buffer,
        &model.theme,
        &model.highlighter,
        editor,
        &panel,
        content_x,
        content_width,
        y,
    );

    // --- Text area ---
    // render_text_area naturally leaves a 1-row gap before hotkey_row
    let hotkey_row = panel.y + panel.height - 2;
    render_text_area(
        buffer,
        &model.theme,
        editor,
        content_x,
        content_width,
        y,
        hotkey_row,
    );

    // --- Bottom bar: title left + hotkeys right ---
    let title = build_title(editor);
    let help_area = Rect::new(panel.x, hotkey_row, panel.width, 1);
    let hints = [
        HotkeyHint::new("Submit", "ctrl+s"),
        HotkeyHint::new("Cancel", "esc"),
    ];
    draw_help_bar_ext(
        buffer,
        help_area,
        &model.theme,
        &hints,
        model.theme.panel_bg,
        &title,
    );
}

fn build_title(editor: &InlineEditor) -> String {
    let line_range = match editor.request.end_line {
        Some(end) if end != editor.request.start_line => {
            format!(
                "{}:{}-{}",
                editor.request.file_path, editor.request.start_line, end
            )
        }
        _ => format!("{}:{}", editor.request.file_path, editor.request.start_line),
    };
    if editor.request.thread_id.is_some() {
        format!("Reply on {line_range}")
    } else {
        format!("Comment on {line_range}")
    }
}

fn compute_panel(
    screen: Rect,
    editor: &InlineEditor,
    diff_pane_x: u32,
    diff_pane_width: u32,
) -> Rect {
    let natural_w = (diff_pane_width * 7 / 10).min(80);
    let (panel_width, panel_x) = if natural_w < MIN_WIDTH {
        // Pane too narrow for margins — fill the diff pane
        (diff_pane_width, diff_pane_x)
    } else {
        let x = diff_pane_x + (diff_pane_width.saturating_sub(natural_w)) / 2;
        (natural_w, x)
    };

    let content_width = panel_width.saturating_sub(H_PAD * 2);
    let context_rows = existing_comments_height(&editor.request.existing_comments, content_width);
    let text_area_height = 8u32;
    // 1 top padding + context + text + 1 gap + 1 hotkey row + 1 bottom padding
    let ideal_height = 1 + context_rows + text_area_height + 1 + 1 + 1;
    let panel_height = ideal_height
        .clamp(MIN_HEIGHT, screen.height.saturating_sub(2))
        .min(screen.height);
    // Pin to bottom with 1-row margin
    let panel_y = screen.height.saturating_sub(panel_height + 1);

    Rect::new(panel_x, panel_y, panel_width, panel_height)
}

fn render_existing_comments(
    buffer: &mut OptimizedBuffer,
    theme: &Theme,
    highlighter: &crate::syntax::Highlighter,
    editor: &InlineEditor,
    panel: &Rect,
    content_x: u32,
    content_width: u32,
    mut y: u32,
) -> u32 {
    if editor.request.existing_comments.is_empty() {
        return y;
    }
    let skip = existing_comments_start_index(&editor.request.existing_comments, content_width);
    let body_x = content_x + COMMENT_BODY_INDENT;
    let body_width = content_width.saturating_sub(COMMENT_BODY_INDENT);

    'comments: for comment in editor.request.existing_comments.iter().skip(skip) {
        if y >= panel.y + panel.height - 3 {
            break;
        }

        draw_text_truncated(
            buffer,
            content_x,
            y,
            &format!("@{}", comment.author),
            content_width,
            theme.style_primary().with_bg(theme.panel_bg),
        );
        y += 1;

        for line in
            render_markdown_with_highlighter(&comment.body, body_width as usize, Some(highlighter))
        {
            if y >= panel.y + panel.height - 3 {
                break 'comments;
            }

            let line_bg = markdown_line_bg(theme, theme.panel_bg, line.style);
            buffer_fill_rect(buffer, body_x, y, body_width, 1, line_bg);
            draw_markdown_content(
                buffer,
                theme,
                body_x,
                y,
                body_width,
                line_bg,
                &line.content,
                line.style,
            );
            y += 1;
        }
    }
    y + 1 // blank separator
}

fn existing_comments_height(comments: &[Comment], content_width: u32) -> u32 {
    if comments.is_empty() {
        return 0;
    }

    let skip = existing_comments_start_index(comments, content_width);
    let body_width = content_width.saturating_sub(COMMENT_BODY_INDENT) as usize;
    let mut rows = 0u32;
    for comment in comments.iter().skip(skip) {
        rows += 1;
        rows += render_markdown(&comment.body, body_width).len() as u32;
    }

    rows + 1
}

fn existing_comments_start_index(comments: &[Comment], content_width: u32) -> usize {
    let body_width = content_width.saturating_sub(COMMENT_BODY_INDENT) as usize;
    let mut rows = 0u32;
    let mut count = 0usize;

    for comment in comments.iter().rev() {
        let comment_rows = 1 + render_markdown(&comment.body, body_width).len() as u32;
        if count > 0 && rows + comment_rows > MAX_CONTEXT_ROWS {
            break;
        }
        if count >= MAX_CONTEXT_COMMENTS {
            break;
        }

        rows += comment_rows;
        count += 1;
    }

    comments.len().saturating_sub(count)
}

fn render_text_area(
    buffer: &mut OptimizedBuffer,
    theme: &Theme,
    editor: &InlineEditor,
    content_x: u32,
    content_width: u32,
    text_area_top: u32,
    status_y: u32,
) {
    let available_text_rows = status_y.saturating_sub(text_area_top + 1) as usize;
    let text_style = theme.style_foreground().with_bg(theme.panel_bg);
    let cursor_style = Style::fg(theme.panel_bg).with_bg(theme.foreground);

    let text_x = content_x;
    let text_width = content_width;
    let scroll = editor.scroll;

    for (view_row, line_idx) in (scroll..editor.lines.len())
        .enumerate()
        .take(available_text_rows)
    {
        let line_y = text_area_top + view_row as u32;
        if line_y >= status_y {
            break;
        }
        let line = &editor.lines[line_idx];
        if line_idx == editor.cursor_row {
            render_line_with_cursor(
                buffer,
                text_x,
                line_y,
                line,
                editor.cursor_col,
                text_width,
                text_style,
                cursor_style,
            );
        } else {
            draw_text_truncated(buffer, text_x, line_y, line, text_width, text_style);
        }
    }

    // Show cursor on empty first line
    if editor.lines.len() == 1 && editor.lines[0].is_empty() && editor.cursor_col == 0 {
        buffer_draw_text(buffer, text_x, text_area_top, " ", cursor_style);
    }
}

/// Render a line of text with the cursor shown as an inverted-color block.
#[allow(clippy::too_many_arguments)]
fn render_line_with_cursor(
    buffer: &mut OptimizedBuffer,
    x: u32,
    y: u32,
    line: &str,
    cursor_col: usize,
    max_width: u32,
    text_style: Style,
    cursor_style: Style,
) {
    let chars: Vec<char> = line.chars().collect();
    let mut col = 0u32;

    for (i, &ch) in chars.iter().enumerate() {
        if col >= max_width {
            break;
        }
        let style = if i == cursor_col {
            cursor_style
        } else {
            text_style
        };
        let s = ch.to_string();
        buffer_draw_text(buffer, x + col, y, &s, style);
        col += 1;
    }

    // If cursor is at end of line, draw cursor block on the space after
    if cursor_col >= chars.len() && col < max_width {
        buffer_draw_text(buffer, x + col, y, " ", cursor_style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(author: &str, body: &str) -> Comment {
        Comment {
            comment_id: "th-1.1".to_string(),
            author: author.to_string(),
            body: body.to_string(),
            created_at: "2026-03-10T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn context_height_counts_markdown_rows() {
        let comments = vec![comment("alice", "```rust\nfn main() {}\n```")];
        assert!(existing_comments_height(&comments, 40) > 3);
    }

    #[test]
    fn context_window_prefers_latest_comments() {
        let comments = vec![
            comment("a", "old"),
            comment("b", "older"),
            comment("c", "new"),
            comment("d", "newest"),
            comment("e", "latest"),
        ];

        let skip = existing_comments_start_index(&comments, 20);
        assert!(skip > 0);
        assert_eq!(comments[skip].author, "b");
    }
}
