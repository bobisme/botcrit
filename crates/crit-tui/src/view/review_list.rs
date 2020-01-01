//! Review list screen rendering

use crate::render_backend::{buffer_draw_text, buffer_fill_rect, OptimizedBuffer, Style};

use super::components::{
    draw_block, draw_help_bar_ext, draw_text_truncated, BlockLine, HotkeyHint, Rect,
};
use crate::model::{Model, ReviewFilter};

/// Height of the header block (margin + padding + 1 content line + padding + margin)
const HEADER_HEIGHT: u32 = 5;
/// Height of the search bar area (prompt line + blank line below)
const SEARCH_HEIGHT: u32 = 2;
/// Lines per review item
const ITEM_HEIGHT: u32 = 2;

/// Render the review list screen
pub fn view(model: &Model, buffer: &mut OptimizedBuffer) {
    let theme = &model.theme;
    let area = Rect::from_size(model.width, model.height);

    // Fill background
    buffer_fill_rect(
        buffer,
        area.x,
        area.y,
        area.width,
        area.height,
        theme.background,
    );

    // Header block
    let header_text = model.repo_path.as_ref().map_or_else(
        || "Reviews".to_string(),
        |path| {
            let display_path = std::env::var("HOME")
                .ok()
                .and_then(|home| path.strip_prefix(&home).map(|rest| format!("~{rest}")))
                .unwrap_or_else(|| path.clone());
            format!("Reviews for {display_path}")
        },
    );
    draw_block(
        buffer,
        Rect::new(area.x, area.y, area.width, HEADER_HEIGHT),
        theme,
        theme.panel_bg,
        &[BlockLine::new(
            &header_text,
            Style::fg(theme.foreground).with_bold(),
        )],
    );

    // Search bar
    let search_y = area.y + HEADER_HEIGHT;
    draw_search_bar(model, buffer, area.x, search_y, area.width);

    // List area
    let list_y = search_y + SEARCH_HEIGHT;
    let list_height = area
        .height
        .saturating_sub(HEADER_HEIGHT + SEARCH_HEIGHT + 2); // 2 for help bar
    let list_area = Rect::new(area.x, list_y, area.width, list_height);

    let reviews = model.filtered_reviews();

    if reviews.is_empty() {
        buffer_draw_text(
            buffer,
            list_area.x + 4,
            list_area.y,
            "No reviews found",
            theme.style_muted(),
        );
        render_help_bar(model, buffer, area);
        return;
    }

    let visible_items = (list_height / ITEM_HEIGHT) as usize;
    let start = model.list_scroll.min(reviews.len());
    let end = (start + visible_items).min(reviews.len());

    for (row, review) in reviews[start..end].iter().enumerate() {
        let idx = start + row;
        let y = list_area.y + (row as u32) * ITEM_HEIGHT;
        draw_review_item(model, buffer, list_area, y, review, idx == model.list_index);
    }

    render_help_bar(model, buffer, area);
}

fn draw_search_bar(model: &Model, buffer: &mut OptimizedBuffer, x: u32, y: u32, width: u32) {
    let theme = &model.theme;
    buffer_fill_rect(buffer, x, y, width, SEARCH_HEIGHT, theme.background);

    let text_x = x + 5;
    if model.search_active {
        let max_chars = width.saturating_sub(8) as usize; // 5 margin + "/ " + cursor
        let visible = tail_chars(&model.search_input, max_chars);
        let prompt = format!("/ {visible}\u{2588}");
        buffer_draw_text(buffer, text_x, y, &prompt, theme.style_foreground());
    } else {
        buffer_draw_text(buffer, text_x, y, "Press / to search", theme.style_muted());
    }
}

fn tail_chars(text: &str, max_chars: usize) -> &str {
    if max_chars == 0 {
        return "";
    }

    let total = text.chars().count();
    if total <= max_chars {
        return text;
    }

    let skip = total - max_chars;
    let start = text.char_indices().nth(skip).map_or(0, |(idx, _)| idx);
    &text[start..]
}

fn draw_review_item(
    model: &Model,
    buffer: &mut OptimizedBuffer,
    area: Rect,
    y: u32,
    review: &crate::db::ReviewSummary,
    selected: bool,
) {
    let theme = &model.theme;
    let bg = if selected {
        theme.selection_bg
    } else {
        theme.background
    };

    // Fill both lines with 2-space margin on each side
    let margin: u32 = 2;
    let item_x = area.x + margin;
    let item_width = area.width.saturating_sub(margin * 2);
    buffer_fill_rect(buffer, item_x, y, item_width, ITEM_HEIGHT, bg);

    let left_pad: u32 = 3;
    let right_pad: u32 = 2;
    let mut x = item_x + left_pad;
    let right_edge = item_x + item_width.saturating_sub(right_pad);

    // === Line 1: id  title ...    N th ===

    // Review ID
    let id_style = Style::fg(theme.primary).with_bg(bg);
    let id_len = review.review_id.len() as u32;
    buffer_draw_text(buffer, x, y, &review.review_id, id_style);
    x += id_len + 2;

    // Thread count (right-aligned): "N th"
    let thread_text = format_thread_label(review.thread_count, review.open_thread_count);
    let thread_len = thread_text.len() as u32;
    let thread_x = right_edge.saturating_sub(thread_len);
    let thread_color = if selected {
        theme.selection_fg
    } else if review.open_thread_count > 0 {
        theme.warning
    } else {
        theme.muted
    };
    buffer_draw_text(
        buffer,
        thread_x,
        y,
        &thread_text,
        Style::fg(thread_color).with_bg(bg),
    );

    // Title (fills space between ID and thread count)
    let title_width = thread_x.saturating_sub(x + 1);
    let title_style = if selected {
        Style::fg(theme.selection_fg).with_bg(bg)
    } else {
        Style::fg(theme.foreground).with_bg(bg)
    };
    draw_text_truncated(buffer, x, y, &review.title, title_width, title_style);

    // === Line 2: [status]  @author ===
    let y2 = y + 1;
    let mut x2 = item_x + left_pad;

    // Status badge
    let badge = format!("[{}]", review.status);
    let badge_color = if selected {
        theme.selection_fg
    } else {
        match review.status.as_str() {
            "open" | "merged" => theme.success,
            "abandoned" => theme.muted,
            "approved" => theme.warning,
            _ => theme.foreground,
        }
    };
    buffer_draw_text(buffer, x2, y2, &badge, Style::fg(badge_color).with_bg(bg));
    x2 += badge.len() as u32 + 2;

    // Author -> Reviewers
    let people = if review.reviewers.is_empty() {
        format!("@{}", review.author)
    } else {
        let reviewers: Vec<String> = review.reviewers.iter().map(|r| format!("@{r}")).collect();
        format!("@{} -> {}", review.author, reviewers.join(", "))
    };
    let people_color = if selected {
        theme.selection_fg
    } else {
        theme.muted
    };
    let people_width = right_edge.saturating_sub(x2);
    draw_text_truncated(
        buffer,
        x2,
        y2,
        &people,
        people_width,
        Style::fg(people_color).with_bg(bg),
    );
}

fn format_thread_label(total: i64, open: i64) -> String {
    if total == 0 {
        return String::new();
    }
    if open > 0 {
        format!("{open}/{total} th")
    } else {
        format!("{total} th")
    }
}

fn render_help_bar(model: &Model, buffer: &mut OptimizedBuffer, area: Rect) {
    let version = concat!("crit-ui v", env!("CARGO_PKG_VERSION"));
    let filter_hint = HotkeyHint::new(
        match model.filter {
            ReviewFilter::All => "Status (All)",
            ReviewFilter::Open => "Status (Open)",
            ReviewFilter::Closed => "Status (Closed)",
        },
        "s",
    );

    if model.search_active {
        let hints = &[
            HotkeyHint::new("Commands", "ctrl+p"),
            HotkeyHint::new("Select", "Enter"),
            filter_hint,
            HotkeyHint::new("Clear", "Esc"),
            HotkeyHint::new("Quit", "ctrl+c"),
        ];
        draw_help_bar_ext(
            buffer,
            area,
            &model.theme,
            hints,
            model.theme.background,
            version,
        );
    } else {
        let hints = &[
            HotkeyHint::new("Commands", "ctrl+p"),
            HotkeyHint::new("Select", "Enter"),
            filter_hint,
            HotkeyHint::new("Quit", "q"),
        ];
        draw_help_bar_ext(
            buffer,
            area,
            &model.theme,
            hints,
            model.theme.background,
            version,
        );
    }
}
