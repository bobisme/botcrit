//! UI rendering for the TUI.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use super::app::{App, Panel};
use super::theme;

/// Draw the entire UI
pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),   // Main content
            Constraint::Length(1), // Status bar
        ])
        .split(frame.area());

    draw_main_content(frame, app, chunks[0]);
    draw_status_bar(frame, app, chunks[1]);

    // Draw help overlay if active
    if app.show_help {
        draw_help_popup(frame);
    }
}

/// Draw the main content area (panels)
fn draw_main_content(frame: &mut Frame, app: &App, area: Rect) {
    // Split into left (list panels) and right (detail panel)
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40), // List panels
            Constraint::Percentage(60), // Detail panel
        ])
        .split(area);

    draw_list_panels(frame, app, main_chunks[0]);
    draw_detail_panel(frame, app, main_chunks[1]);
}

/// Draw the stacked list panels (Reviews, Threads, Comments)
fn draw_list_panels(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(area);

    draw_reviews_panel(frame, app, chunks[0]);
    draw_threads_panel(frame, app, chunks[1]);
    draw_comments_panel(frame, app, chunks[2]);
}

/// Draw the reviews panel
fn draw_reviews_panel(frame: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_panel == Panel::Reviews;
    let title = format!("[{}] Reviews", Panel::Reviews.number());

    let items: Vec<ListItem> = app
        .reviews
        .iter()
        .enumerate()
        .map(|(i, review)| {
            let status_style = match review.status.as_str() {
                "open" => Style::default().fg(theme::CURRENT),
                "approved" => Style::default()
                    .fg(theme::CURRENT)
                    .add_modifier(Modifier::BOLD),
                "merged" => Style::default().fg(theme::RESOLVED),
                "abandoned" => Style::default().fg(theme::INACTIVE),
                _ => Style::default(),
            };

            let thread_info = if review.open_thread_count > 0 {
                format!(" {} open", review.open_thread_count)
            } else if review.thread_count > 0 {
                format!(" {} thr", review.thread_count)
            } else {
                String::new()
            };

            let content = format!("{} [{}]{}", review.review_id, review.status, thread_info);

            let style = if i == app.review_index {
                Style::default().bg(theme::SELECTED_BG)
            } else {
                Style::default()
            };

            ListItem::new(Line::from(vec![Span::styled(
                content,
                style.patch(status_style),
            )]))
        })
        .collect();

    let block = styled_block(&title, is_focused);
    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

/// Draw the threads panel
fn draw_threads_panel(frame: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_panel == Panel::Threads;
    let title = format!("[{}] Threads", Panel::Threads.number());

    let items: Vec<ListItem> = app
        .threads
        .iter()
        .enumerate()
        .map(|(i, thread)| {
            let status_indicator = if thread.status == "resolved" {
                Span::styled("✓ ", Style::default().fg(theme::RESOLVED))
            } else {
                Span::styled("○ ", Style::default().fg(theme::CURRENT))
            };

            let line_info = match thread.selection_end {
                Some(end) if end != thread.selection_start => {
                    format!(":{}-{}", thread.selection_start, end)
                }
                _ => format!(":{}", thread.selection_start),
            };

            // Truncate file path if needed
            let file_display = if thread.file_path.len() > 20 {
                format!("...{}", &thread.file_path[thread.file_path.len() - 17..])
            } else {
                thread.file_path.clone()
            };

            let content = format!("{}{}", file_display, line_info);

            let style = if i == app.thread_index {
                Style::default().bg(theme::SELECTED_BG)
            } else {
                Style::default()
            };

            ListItem::new(Line::from(vec![
                status_indicator,
                Span::styled(content, style),
            ]))
        })
        .collect();

    let block = styled_block(&title, is_focused);
    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

/// Draw the comments panel
fn draw_comments_panel(frame: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.focused_panel == Panel::Comments;
    let title = format!("[{}] Comments", Panel::Comments.number());

    let items: Vec<ListItem> = app
        .comments
        .iter()
        .enumerate()
        .map(|(i, comment)| {
            // Truncate body to first line
            let body_preview: String = comment
                .body
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(30)
                .collect();

            let content = format!("{}: {}", comment.author, body_preview);

            let style = if i == app.comment_index {
                Style::default().bg(theme::SELECTED_BG)
            } else {
                Style::default()
            };

            ListItem::new(Span::styled(content, style))
        })
        .collect();

    let block = styled_block(&title, is_focused);
    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

/// Draw the detail panel (context-sensitive)
fn draw_detail_panel(frame: &mut Frame, app: &App, area: Rect) {
    let block = styled_block("[0] Detail", false);

    let content = match app.focused_panel {
        Panel::Reviews => format_review_detail(app),
        Panel::Threads => format_thread_detail(app),
        Panel::Comments => format_comment_detail(app),
    };

    let paragraph = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: true });

    frame.render_widget(paragraph, area);
}

/// Format review detail content
fn format_review_detail(app: &App) -> String {
    match app.selected_review() {
        Some(review) => {
            format!(
                "Review: {}\n\
                 ID: {}\n\
                 Author: {}\n\
                 Status: {}\n\n\
                 Threads: {} total, {} open",
                review.title,
                review.review_id,
                review.author,
                review.status,
                review.thread_count,
                review.open_thread_count,
            )
        }
        None => "No review selected".to_string(),
    }
}

/// Format thread detail content
fn format_thread_detail(app: &App) -> String {
    match app.selected_thread() {
        Some(thread) => {
            let line_info = match thread.selection_end {
                Some(end) if end != thread.selection_start => {
                    format!("lines {}-{}", thread.selection_start, end)
                }
                _ => format!("line {}", thread.selection_start),
            };

            format!(
                "Thread: {}\n\
                 File: {}\n\
                 Location: {}\n\
                 Status: {}\n\
                 Comments: {}",
                thread.thread_id, thread.file_path, line_info, thread.status, thread.comment_count,
            )
        }
        None => "No thread selected".to_string(),
    }
}

/// Format comment detail content
fn format_comment_detail(app: &App) -> String {
    match app.selected_comment() {
        Some(comment) => {
            format!(
                "Comment: {}\n\
                 Author: {}\n\
                 Time: {}\n\n\
                 {}",
                comment.comment_id,
                comment.author,
                format_timestamp(&comment.created_at),
                comment.body,
            )
        }
        None => "No comment selected".to_string(),
    }
}

/// Draw the status bar
fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let hints = get_context_hints(app);

    let mut spans = Vec::new();
    for (i, (action, key)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" | ", Style::default().fg(theme::STATUS_BAR)));
        }
        spans.push(Span::styled(
            format!("{}: ", action),
            Style::default().fg(theme::STATUS_BAR),
        ));
        spans.push(Span::styled(
            *key,
            Style::default()
                .fg(theme::STATUS_BAR)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Add status message if present
    if let Some(ref msg) = app.status_message {
        spans.push(Span::styled(
            format!("  [{}]", msg),
            Style::default().fg(theme::DIM),
        ));
    }

    let paragraph = Paragraph::new(Line::from(spans));
    frame.render_widget(paragraph, area);
}

/// Get context-sensitive key hints
fn get_context_hints(app: &App) -> Vec<(&'static str, &'static str)> {
    let mut hints = vec![("Navigate", "j/k"), ("Panel", "Tab")];

    match app.focused_panel {
        Panel::Reviews => {
            hints.push(("Approve", "a"));
            hints.push(("Merge", "m"));
        }
        Panel::Threads => {
            hints.push(("Resolve", "r"));
        }
        Panel::Comments => {}
    }

    hints.push(("Refresh", "R"));
    hints.push(("Help", "?"));
    hints.push(("Quit", "q"));

    hints
}

/// Draw help popup
fn draw_help_popup(frame: &mut Frame) {
    let area = centered_rect(50, 70, frame.area());

    frame.render_widget(Clear, area);

    let help_text = vec![
        Line::from(Span::styled(
            "Navigation",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  j/↓        Move down"),
        Line::from("  k/↑        Move up"),
        Line::from("  g          Go to top"),
        Line::from("  G          Go to bottom"),
        Line::from(""),
        Line::from(Span::styled(
            "Panels",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  Tab        Next panel"),
        Line::from("  Shift+Tab  Previous panel"),
        Line::from("  1/2/3      Jump to panel"),
        Line::from(""),
        Line::from(Span::styled(
            "Actions",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  r          Resolve thread"),
        Line::from("  a          Approve review"),
        Line::from("  m          Merge review"),
        Line::from("  R          Refresh"),
        Line::from(""),
        Line::from(Span::styled(
            "General",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  ?          Toggle help"),
        Line::from("  q          Quit"),
        Line::from("  Ctrl+Z     Suspend"),
    ];

    let block = Block::default()
        .title("Help")
        .borders(Borders::ALL)
        .border_type(theme::BORDER_TYPE)
        .border_style(Style::default().fg(theme::FOCUSED));

    let paragraph = Paragraph::new(help_text).block(block);
    frame.render_widget(paragraph, area);
}

/// Create a styled block with focus indicator
fn styled_block(title: &str, is_focused: bool) -> Block<'_> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(theme::BORDER_TYPE)
        .border_style(if is_focused {
            Style::default().fg(theme::FOCUSED)
        } else {
            Style::default()
        })
}

/// Create a centered rectangle
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Format timestamp to readable form
fn format_timestamp(iso: &str) -> String {
    // Simple extraction: "2026-01-25T12:34:56..." -> "Jan 25, 12:34"
    if let Some(t_pos) = iso.find('T') {
        let date = &iso[..t_pos];
        let time = &iso[t_pos + 1..];

        let parts: Vec<&str> = date.split('-').collect();
        if parts.len() == 3 {
            let month = match parts[1] {
                "01" => "Jan",
                "02" => "Feb",
                "03" => "Mar",
                "04" => "Apr",
                "05" => "May",
                "06" => "Jun",
                "07" => "Jul",
                "08" => "Aug",
                "09" => "Sep",
                "10" => "Oct",
                "11" => "Nov",
                "12" => "Dec",
                _ => parts[1],
            };
            let day = parts[2].trim_start_matches('0');
            let time_short: String = time.chars().take(5).collect();
            return format!("{} {}, {}", month, day, time_short);
        }
    }
    iso.to_string()
}
