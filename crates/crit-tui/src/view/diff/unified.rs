//! Unified diff rendering (single-pane with +/- lines).

use crate::render_backend::{buffer_draw_text, buffer_fill_rect, OptimizedBuffer, Style};

use crate::diff::{DiffLine, DiffLineKind};
use crate::layout::UNIFIED_LINE_NUM_WIDTH;
use crate::syntax::HighlightSpan;
use crate::theme::Theme;

use super::helpers::{
    cursor_bg, cursor_fg, diff_content_width, diff_content_x, draw_diff_base_line, selection_bg,
};
use super::text_util::{draw_highlighted_text, draw_wrapped_line, HighlightContent, WrappedLine};
use super::{DisplayLine, LineRenderCtx};

pub(super) fn render_unified_diff_line_block(
    buffer: &mut OptimizedBuffer,
    y: u32,
    display_line: &DisplayLine,
    theme: &Theme,
    ctx: &LineRenderCtx<'_>,
    highlights: Option<&Vec<HighlightSpan>>,
) {
    let dt = &theme.diff;
    match display_line {
        DisplayLine::HunkHeader => {
            draw_diff_base_line(buffer, ctx.area, y, dt.context_bg);
            let sep = "···";
            let sep_x = diff_content_x(ctx.area)
                + diff_content_width(ctx.area).saturating_sub(sep.len() as u32) / 2;
            buffer_draw_text(buffer, sep_x, y, sep, theme.style_muted_on(dt.context_bg));
        }
        DisplayLine::Diff(line) => {
            let base_bg = cursor_bg(
                selection_bg(dt.context_bg, ctx.is_selected, theme),
                ctx.is_cursor,
                theme,
            );
            draw_diff_base_line(buffer, ctx.area, y, base_bg);

            let content_x = diff_content_x(ctx.area);

            let line_num_width = UNIFIED_LINE_NUM_WIDTH;
            let content_start = content_x + line_num_width;
            let content_width = diff_content_width(ctx.area).saturating_sub(line_num_width);
            render_diff_line(
                buffer,
                y,
                &UnifiedLineLayout {
                    x: content_x,
                    content_x: content_start,
                    content_width,
                },
                line,
                dt,
                highlights,
                ctx.is_cursor,
                ctx.is_selected,
                theme,
            );
        }
    }
}

pub(super) fn render_unified_diff_line_wrapped_row(
    buffer: &mut OptimizedBuffer,
    y: u32,
    line: &DiffLine,
    theme: &Theme,
    ctx: &LineRenderCtx<'_>,
    wrapped: &[WrappedLine],
    row: usize,
) {
    let dt = &theme.diff;
    let is_cursor = ctx.is_cursor;
    let is_sel = ctx.is_selected;
    let (bg, line_num_bg, default_fg, sign, sign_color) = match line.kind {
        DiffLineKind::Added => (
            cursor_bg(selection_bg(dt.added_bg, is_sel, theme), is_cursor, theme),
            cursor_bg(
                selection_bg(dt.added_line_number_bg, is_sel, theme),
                is_cursor,
                theme,
            ),
            cursor_fg(dt.added, is_cursor),
            "+",
            cursor_fg(dt.highlight_added, is_cursor),
        ),
        DiffLineKind::Removed => (
            cursor_bg(selection_bg(dt.removed_bg, is_sel, theme), is_cursor, theme),
            cursor_bg(
                selection_bg(dt.removed_line_number_bg, is_sel, theme),
                is_cursor,
                theme,
            ),
            cursor_fg(dt.removed, is_cursor),
            "-",
            cursor_fg(dt.highlight_removed, is_cursor),
        ),
        DiffLineKind::Context => (
            cursor_bg(selection_bg(dt.context_bg, is_sel, theme), is_cursor, theme),
            cursor_bg(selection_bg(dt.context_bg, is_sel, theme), is_cursor, theme),
            cursor_fg(dt.context, is_cursor),
            " ",
            cursor_fg(dt.context, is_cursor),
        ),
    };

    let base_bg = cursor_bg(selection_bg(dt.context_bg, is_sel, theme), is_cursor, theme);
    draw_diff_base_line(buffer, ctx.area, y, base_bg);

    let content_x = diff_content_x(ctx.area);

    let line_num_width = UNIFIED_LINE_NUM_WIDTH;
    let line_num_x = content_x;
    buffer_fill_rect(buffer, line_num_x, y, line_num_width, 1, line_num_bg);
    if row == 0 {
        let old_ln = line
            .old_line
            .map_or_else(|| "     ".to_string(), |n| format!("{n:>5}"));
        let new_ln = line
            .new_line
            .map_or_else(|| "     ".to_string(), |n| format!("{n:>5}"));

        let ln_fg = cursor_fg(dt.line_number, is_cursor);
        buffer_draw_text(
            buffer,
            line_num_x,
            y,
            &old_ln,
            Style::fg(ln_fg).with_bg(line_num_bg),
        );
        buffer_draw_text(
            buffer,
            line_num_x + 5,
            y,
            " ",
            Style::fg(ln_fg).with_bg(line_num_bg),
        );
        buffer_draw_text(
            buffer,
            line_num_x + 6,
            y,
            &new_ln,
            Style::fg(ln_fg).with_bg(line_num_bg),
        );
        buffer_draw_text(
            buffer,
            line_num_x + 11,
            y,
            " ",
            Style::fg(ln_fg).with_bg(line_num_bg),
        );
    }

    let content_start = line_num_x + line_num_width;
    let content_width = diff_content_width(ctx.area).saturating_sub(line_num_width);
    buffer_fill_rect(buffer, content_start, y, content_width, 1, bg);
    if row == 0 {
        buffer_draw_text(
            buffer,
            content_start,
            y,
            sign,
            Style::fg(sign_color).with_bg(bg),
        );
    }

    if let Some(line_content) = wrapped.get(row) {
        let max_content = content_width.saturating_sub(2);
        draw_wrapped_line(
            buffer,
            content_start + 1,
            y,
            max_content,
            line_content,
            default_fg,
            bg,
        );
    }
}

/// Layout coordinates for a unified diff line.
pub(super) struct UnifiedLineLayout {
    x: u32,
    content_x: u32,
    content_width: u32,
}

/// Render a single unified diff line (line numbers + sign + content)
pub(super) fn render_diff_line(
    buffer: &mut OptimizedBuffer,
    y: u32,
    layout: &UnifiedLineLayout,
    line: &DiffLine,
    dt: &crate::theme::DiffTheme,
    highlights: Option<&Vec<HighlightSpan>>,
    is_cursor: bool,
    is_selected: bool,
    theme: &Theme,
) {
    let (bg, line_num_bg, default_fg, sign, sign_color) = match line.kind {
        DiffLineKind::Added => (
            cursor_bg(
                selection_bg(dt.added_bg, is_selected, theme),
                is_cursor,
                theme,
            ),
            cursor_bg(
                selection_bg(dt.added_line_number_bg, is_selected, theme),
                is_cursor,
                theme,
            ),
            cursor_fg(dt.added, is_cursor),
            "+",
            cursor_fg(dt.highlight_added, is_cursor),
        ),
        DiffLineKind::Removed => (
            cursor_bg(
                selection_bg(dt.removed_bg, is_selected, theme),
                is_cursor,
                theme,
            ),
            cursor_bg(
                selection_bg(dt.removed_line_number_bg, is_selected, theme),
                is_cursor,
                theme,
            ),
            cursor_fg(dt.removed, is_cursor),
            "-",
            cursor_fg(dt.highlight_removed, is_cursor),
        ),
        DiffLineKind::Context => (
            cursor_bg(
                selection_bg(dt.context_bg, is_selected, theme),
                is_cursor,
                theme,
            ),
            cursor_bg(
                selection_bg(dt.context_bg, is_selected, theme),
                is_cursor,
                theme,
            ),
            cursor_fg(dt.context, is_cursor),
            " ",
            cursor_fg(dt.context, is_cursor),
        ),
    };

    let ln_fg = cursor_fg(dt.line_number, is_cursor);
    buffer_fill_rect(buffer, layout.x, y, 12, 1, line_num_bg);
    buffer_fill_rect(buffer, layout.content_x, y, layout.content_width, 1, bg);

    let old_ln = line
        .old_line
        .map_or_else(|| "     ".to_string(), |n| format!("{n:>5}"));
    let new_ln = line
        .new_line
        .map_or_else(|| "     ".to_string(), |n| format!("{n:>5}"));

    buffer_draw_text(
        buffer,
        layout.x,
        y,
        &old_ln,
        Style::fg(ln_fg).with_bg(line_num_bg),
    );
    buffer_draw_text(
        buffer,
        layout.x + 5,
        y,
        " ",
        Style::fg(ln_fg).with_bg(line_num_bg),
    );
    buffer_draw_text(
        buffer,
        layout.x + 6,
        y,
        &new_ln,
        Style::fg(ln_fg).with_bg(line_num_bg),
    );
    buffer_draw_text(
        buffer,
        layout.x + 11,
        y,
        " ",
        Style::fg(ln_fg).with_bg(line_num_bg),
    );

    buffer_draw_text(
        buffer,
        layout.content_x,
        y,
        sign,
        Style::fg(sign_color).with_bg(bg),
    );

    let max_content = layout.content_width.saturating_sub(2);
    draw_highlighted_text(
        buffer,
        layout.content_x + 1,
        y,
        max_content,
        &HighlightContent {
            spans: highlights,
            fallback_text: &line.content,
            fallback_fg: default_fg,
            bg,
        },
    );
}
