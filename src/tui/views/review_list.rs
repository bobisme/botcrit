//! Review list view - the entry point of the UI.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::projection::ReviewSummary;
use crate::tui::theme;

/// State for the review list view
pub struct ReviewListView {
    /// All reviews
    pub reviews: Vec<ReviewSummary>,
    /// List selection state
    pub list_state: ListState,
}

impl ReviewListView {
    /// Create a new review list view
    pub fn new(mut reviews: Vec<ReviewSummary>) -> Self {
        // Sort: open first, then approved, merged, abandoned
        reviews.sort_by_key(|r| match r.status.as_str() {
            "open" => 0,
            "approved" => 1,
            "merged" => 2,
            "abandoned" => 3,
            _ => 4,
        });
        let mut list_state = ListState::default();
        if !reviews.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            reviews,
            list_state,
        }
    }

    /// Get the currently selected review
    #[must_use]
    pub fn selected_review(&self) -> Option<&ReviewSummary> {
        self.list_state.selected().and_then(|i| self.reviews.get(i))
    }

    /// Move selection up
    pub fn move_up(&mut self) {
        if self.reviews.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    /// Move selection down
    pub fn move_down(&mut self) {
        if self.reviews.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => (i + 1).min(self.reviews.len() - 1),
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    /// Jump to top
    pub fn jump_to_top(&mut self) {
        if !self.reviews.is_empty() {
            self.list_state.select(Some(0));
        }
    }

    /// Jump to bottom
    pub fn jump_to_bottom(&mut self) {
        if !self.reviews.is_empty() {
            self.list_state.select(Some(self.reviews.len() - 1));
        }
    }

    /// Render the view
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header
                Constraint::Min(5),    // List
                Constraint::Length(1), // Status bar
            ])
            .split(area);

        self.render_header(frame, chunks[0]);
        self.render_list(frame, chunks[1]);
        self.render_status_bar(frame, chunks[2]);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let header = Paragraph::new(Line::from(vec![
            Span::styled(
                " crit ",
                Style::default()
                    .fg(theme::FOCUSED)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("Code Reviews"),
        ]))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(theme::DIM)),
        );
        frame.render_widget(header, area);
    }

    fn render_list(&mut self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .reviews
            .iter()
            .map(|review| {
                let status_style = match review.status.as_str() {
                    "open" => Style::default().fg(theme::CURRENT),
                    "approved" => Style::default()
                        .fg(theme::CURRENT)
                        .add_modifier(Modifier::BOLD),
                    "merged" => Style::default().fg(theme::RESOLVED),
                    "abandoned" => Style::default().fg(theme::INACTIVE),
                    _ => Style::default(),
                };

                let status_icon = match review.status.as_str() {
                    "open" => "●",
                    "approved" => "✓",
                    "merged" => "◆",
                    "abandoned" => "○",
                    _ => " ",
                };

                let thread_info = if review.open_thread_count > 0 {
                    Span::styled(
                        format!("  {} open", review.open_thread_count),
                        Style::default().fg(theme::STALE),
                    )
                } else if review.thread_count > 0 {
                    Span::styled(
                        format!("  {} threads", review.thread_count),
                        Style::default().fg(theme::DIM),
                    )
                } else {
                    Span::raw("")
                };

                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {} ", status_icon), status_style),
                    Span::styled(&review.review_id, Style::default().fg(theme::DIM)),
                    Span::raw("  "),
                    Span::styled(&review.title, Style::default()),
                    Span::raw("  "),
                    Span::styled(&review.author, Style::default().fg(theme::DIM)),
                    thread_info,
                ]))
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .title(" Reviews ")
                    .borders(Borders::ALL)
                    .border_type(theme::BORDER_TYPE)
                    .border_style(Style::default().fg(theme::FOCUSED)),
            )
            .highlight_style(
                Style::default()
                    .bg(theme::SELECTED_BG)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("  ");

        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_status_bar(&self, frame: &mut Frame, area: Rect) {
        let hints = vec![
            ("Navigate", "j/k"),
            ("Select", "Enter/l"),
            ("Top/Bottom", "g/G"),
            ("Help", "?"),
            ("Quit", "q"),
        ];

        let mut spans = Vec::new();
        for (i, (action, key)) in hints.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" │ ", Style::default().fg(theme::DIM)));
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

        let paragraph = Paragraph::new(Line::from(spans));
        frame.render_widget(paragraph, area);
    }
}
