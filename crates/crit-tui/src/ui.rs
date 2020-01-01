//! UI rendering for the TUI.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::app::{App, ViewMode};
use super::theme;

/// Draw the entire UI
pub fn draw(frame: &mut Frame, app: &mut App) {
    match app.view_mode {
        ViewMode::ReviewList => {
            app.review_list.render(frame, frame.area());
        }
        ViewMode::ReviewDetail => {
            if let Some(ref mut detail) = app.review_detail {
                detail.render(frame, frame.area());
            }
        }
    }

    // Draw help overlay if active
    if app.show_help {
        draw_help_popup(frame, app);
    }
}

/// Draw help popup
fn draw_help_popup(frame: &mut Frame, app: &App) {
    let area = centered_rect(50, 70, frame.area());

    frame.render_widget(Clear, area);

    let help_text = match app.view_mode {
        ViewMode::ReviewList => help_text_list(),
        ViewMode::ReviewDetail => help_text_detail(),
    };

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_type(theme::BORDER_TYPE)
        .border_style(Style::default().fg(theme::FOCUSED));

    let paragraph = Paragraph::new(help_text).block(block);
    frame.render_widget(paragraph, area);
}

/// Help text for review list view
fn help_text_list() -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "Navigation",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  j/Down     Move down"),
        Line::from("  k/Up       Move up"),
        Line::from("  g          Jump to top"),
        Line::from("  G          Jump to bottom"),
        Line::from(""),
        Line::from(Span::styled(
            "Actions",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  Enter/l    Open review"),
        Line::from("  R          Refresh"),
        Line::from(""),
        Line::from(Span::styled(
            "General",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  ?          Toggle help"),
        Line::from("  q          Quit"),
        Line::from("  Ctrl+Z     Suspend"),
    ]
}

/// Help text for review detail view
fn help_text_detail() -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "Scrolling",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  j/Down     Scroll down"),
        Line::from("  k/Up       Scroll up"),
        Line::from("  d          Half page down"),
        Line::from("  u          Half page up"),
        Line::from("  g          Jump to top"),
        Line::from("  G          Jump to bottom"),
        Line::from(""),
        Line::from(Span::styled(
            "Files",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  Tab/]      Next file"),
        Line::from("  Shift+Tab/[ Previous file"),
        Line::from("  c          Toggle collapse"),
        Line::from("  C          Collapse all"),
        Line::from("  e          Expand all"),
        Line::from(""),
        Line::from(Span::styled(
            "Display",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  s          Toggle side-by-side"),
        Line::from(""),
        Line::from(Span::styled(
            "General",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  q/Esc/h    Back to list"),
        Line::from("  R          Refresh"),
        Line::from("  ?          Toggle help"),
        Line::from("  Ctrl+Z     Suspend"),
    ]
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
