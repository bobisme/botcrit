//! Command palette modal rendering.
//!
//! Implements the modal overlay design from notes/modal-design.txt:
//! - Dimmed background via alpha-blended overlay
//! - No border
//! - Title (bold left) + "esc" (dim right) header row
//! - Search field with placeholder
//! - Categorized, selectable list items with bullet indicator
//!
//! Supports two modes via `PaletteMode`:
//! - Commands: shows categorized command list
//! - Themes: shows flat theme name list with current theme highlighted

use crate::render_backend::{buffer_draw_text, buffer_fill_rect, OptimizedBuffer, Style};

use crate::{
    command::CommandSpec,
    model::{Focus, Model, PaletteMode},
    theme,
    view::components::{dim_rect, draw_text_truncated, Rect},
};

/// Left padding inside the modal (space before highlight area).
const OUTER_PAD: u32 = 1;
/// Padding inside the highlight area before the bullet.
const INNER_PAD: u32 = 1;
/// Width of the bullet column (● or space).
const BULLET_W: u32 = 1;
/// Space between bullet and text.
const BULLET_GAP: u32 = 1;
/// Trailing padding inside highlight area.
const TRAIL_PAD: u32 = 3;

/// Total left offset from modal edge to content text.
/// = `OUTER_PAD` + `INNER_PAD` + `BULLET_W` + `BULLET_GAP`
const TEXT_INDENT: u32 = OUTER_PAD + INNER_PAD + BULLET_W + BULLET_GAP;

pub fn view(model: &Model, buffer: &mut OptimizedBuffer) {
    if model.focus != Focus::CommandPalette {
        return;
    }

    let screen = Rect::from_size(model.width, model.height);

    // --- Dim background by darkening both fg and bg of every cell ---
    dim_rect(buffer, screen, 0.35);

    match model.command_palette_mode {
        PaletteMode::Commands => render_commands(model, buffer, screen),
        PaletteMode::Themes => render_themes(model, buffer, screen),
    }
}

fn render_commands(model: &Model, buffer: &mut OptimizedBuffer, screen: Rect) {
    // --- Compute modal geometry ---
    let modal_width = 60u32.min(screen.width.saturating_sub(4));

    // Build the list of renderable rows (categories + items) to know total height
    let rows = build_rows(&model.command_palette_commands);
    let list_height = rows.len() as u32;
    // Vertical: 1 blank + title + 1 blank + search + 2 blank + rows + 2 blank
    let modal_height = (1 + 1 + 1 + 1 + 2 + list_height + 2).min(screen.height.saturating_sub(2));
    let modal_x = (screen.width.saturating_sub(modal_width)) / 2;
    let modal_y = screen.height / 4;

    // Fill modal background
    buffer_fill_rect(
        buffer,
        modal_x,
        modal_y,
        modal_width,
        modal_height,
        model.theme.panel_bg,
    );

    let text_x = modal_x + TEXT_INDENT;
    let text_width = modal_width.saturating_sub(TEXT_INDENT + OUTER_PAD);
    let esc_label = "esc";
    let esc_right = modal_x + modal_width - OUTER_PAD - TRAIL_PAD;

    let mut y = modal_y;

    // --- 1 blank row ---
    y += 1;

    // --- Title row: "Commands" (bold left) + "esc" (dim right) ---
    buffer_draw_text(
        buffer,
        text_x,
        y,
        "Commands",
        model.theme.style_foreground().with_bold(),
    );
    let esc_x = esc_right.saturating_sub(esc_label.len() as u32);
    buffer_draw_text(buffer, esc_x, y, esc_label, model.theme.style_muted());
    y += 1;

    // --- 1 blank row ---
    y += 1;

    // --- Search field ---
    render_search_field(model, buffer, text_x, y, text_width);
    y += 1;

    // --- 2 blank rows ---
    y += 2;

    // --- List items ---
    let list_max = modal_y + modal_height - 2; // leave 2 rows at bottom
    for row in &rows {
        if y >= list_max {
            break;
        }
        match row {
            Row::Category(name) => {
                buffer_draw_text(
                    buffer,
                    text_x,
                    y,
                    name,
                    model.theme.style_primary().with_bold(),
                );
            }
            Row::Separator => {
                // blank row between categories
            }
            Row::Item(cmd, idx) => {
                let selected = *idx == model.command_palette_selection;
                render_item_row(buffer, modal_x, y, modal_width, cmd, selected, model);
            }
        }
        y += 1;
    }
}

fn render_themes(model: &Model, buffer: &mut OptimizedBuffer, screen: Rect) {
    let modal_width = 60u32.min(screen.width.saturating_sub(4));

    let theme_names = filtered_theme_names(&model.command_palette_input);
    let list_height = theme_names.len() as u32;
    // Vertical: 1 blank + title + 1 blank + search + 2 blank + rows + 2 blank
    let modal_height = (1 + 1 + 1 + 1 + 2 + list_height + 2).min(screen.height.saturating_sub(2));
    let modal_x = (screen.width.saturating_sub(modal_width)) / 2;
    let modal_y = screen.height / 4;

    buffer_fill_rect(
        buffer,
        modal_x,
        modal_y,
        modal_width,
        modal_height,
        model.theme.panel_bg,
    );

    let text_x = modal_x + TEXT_INDENT;
    let text_width = modal_width.saturating_sub(TEXT_INDENT + OUTER_PAD);
    let esc_label = "esc";
    let esc_right = modal_x + modal_width - OUTER_PAD - TRAIL_PAD;

    let mut y = modal_y;

    // --- 1 blank row ---
    y += 1;

    // --- Title row ---
    buffer_draw_text(
        buffer,
        text_x,
        y,
        "Themes",
        model.theme.style_foreground().with_bold(),
    );
    let esc_x = esc_right.saturating_sub(esc_label.len() as u32);
    buffer_draw_text(buffer, esc_x, y, esc_label, model.theme.style_muted());
    y += 1;

    // --- 1 blank row ---
    y += 1;

    // --- Search field ---
    render_search_field(model, buffer, text_x, y, text_width);
    y += 1;

    // --- 2 blank rows ---
    y += 2;

    // --- Theme list ---
    let list_max = modal_y + modal_height - 2;
    for (idx, name) in theme_names.iter().enumerate() {
        if y >= list_max {
            break;
        }
        let selected = idx == model.command_palette_selection;
        let is_current = *name == model.theme.name;
        render_theme_row(
            buffer,
            &ModalLayout {
                x: modal_x,
                width: modal_width,
            },
            y,
            name,
            selected,
            is_current,
            model,
        );
        y += 1;
    }
}

fn render_search_field(
    model: &Model,
    buffer: &mut OptimizedBuffer,
    text_x: u32,
    y: u32,
    text_width: u32,
) {
    if model.command_palette_input.is_empty() {
        buffer_draw_text(buffer, text_x, y, "Search", model.theme.style_muted());
    } else {
        let input_text = format!("{}\u{2588}", model.command_palette_input);
        draw_text_truncated(
            buffer,
            text_x,
            y,
            &input_text,
            text_width,
            model.theme.style_foreground(),
        );
    }
}

/// Render a single command item row with the horizontal layout spec:
/// `OUTER_PAD | <highlight> INNER_PAD bullet BULLET_GAP name ... shortcut TRAIL_PAD </highlight> | OUTER_PAD`
fn render_item_row(
    buffer: &mut OptimizedBuffer,
    modal_x: u32,
    y: u32,
    modal_width: u32,
    cmd: &CommandSpec,
    selected: bool,
    model: &Model,
) {
    let highlight_x = modal_x + OUTER_PAD;
    let highlight_width = modal_width - (OUTER_PAD * 2);

    let (bg, fg) = if selected {
        (model.theme.selection_bg, model.theme.selection_fg)
    } else {
        (model.theme.panel_bg, model.theme.foreground)
    };
    buffer_fill_rect(buffer, highlight_x, y, highlight_width, 1, bg);

    // Bullet
    let bullet_x = highlight_x + INNER_PAD;
    let bullet = if cmd.active { "●" } else { " " };
    buffer_draw_text(buffer, bullet_x, y, bullet, Style::fg(fg));

    // Content area: name left, shortcut right
    let name_x = bullet_x + BULLET_W + BULLET_GAP;
    let content_end = highlight_x + highlight_width - TRAIL_PAD;
    let content_width = content_end.saturating_sub(name_x);

    if let Some(shortcut) = cmd.shortcut {
        let shortcut_len = shortcut.len() as u32;
        if shortcut_len < content_width {
            let shortcut_x = content_end - shortcut_len;
            buffer_draw_text(buffer, shortcut_x, y, shortcut, model.theme.style_muted());

            let name_max = content_width.saturating_sub(shortcut_len + 1);
            draw_text_truncated(buffer, name_x, y, cmd.name, name_max, Style::fg(fg));
        } else {
            draw_text_truncated(buffer, name_x, y, cmd.name, content_width, Style::fg(fg));
        }
    } else {
        draw_text_truncated(buffer, name_x, y, cmd.name, content_width, Style::fg(fg));
    }
}

/// Render a single theme item row.
/// Uses bullet (●) if this is the currently active theme.
struct ModalLayout {
    x: u32,
    width: u32,
}

fn render_theme_row(
    buffer: &mut OptimizedBuffer,
    layout: &ModalLayout,
    y: u32,
    name: &str,
    selected: bool,
    is_current: bool,
    model: &Model,
) {
    let highlight_x = layout.x + OUTER_PAD;
    let highlight_width = layout.width - (OUTER_PAD * 2);

    let (bg, fg) = if selected {
        (model.theme.selection_bg, model.theme.selection_fg)
    } else {
        (model.theme.panel_bg, model.theme.foreground)
    };
    buffer_fill_rect(buffer, highlight_x, y, highlight_width, 1, bg);

    // Bullet: show ● for current theme
    let bullet_x = highlight_x + INNER_PAD;
    let bullet = if is_current { "●" } else { " " };
    buffer_draw_text(buffer, bullet_x, y, bullet, Style::fg(fg));

    // Theme name
    let name_x = bullet_x + BULLET_W + BULLET_GAP;
    let content_end = highlight_x + highlight_width - TRAIL_PAD;
    let content_width = content_end.saturating_sub(name_x);
    draw_text_truncated(buffer, name_x, y, name, content_width, Style::fg(fg));
}

/// Row types for the command list.
enum Row<'a> {
    Category(&'static str),
    Separator,
    Item(&'a CommandSpec, usize),
}

/// Build a flat list of rows from categorized commands.
fn build_rows(commands: &[CommandSpec]) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    let mut current_category: Option<&str> = None;
    for (selectable_index, cmd) in commands.iter().enumerate() {
        if current_category != Some(cmd.category) {
            if current_category.is_some() {
                rows.push(Row::Separator);
            }
            rows.push(Row::Category(cmd.category));
            current_category = Some(cmd.category);
        }
        rows.push(Row::Item(cmd, selectable_index));
    }

    rows
}

/// Filter theme names by search query (case-insensitive).
fn filtered_theme_names(query: &str) -> Vec<&'static str> {
    let names = theme::built_in_theme_names();
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if terms.is_empty() {
        return names;
    }
    names
        .into_iter()
        .filter(|name| {
            let name_lower = name.to_lowercase();
            terms.iter().all(|term| name_lower.contains(term.as_str()))
        })
        .collect()
}
