//! Review detail view - shows diff with inline comments.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use ansi_to_tui::IntoText;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState,
    },
    Frame,
};

use crate::jj::JjRepo;
use crate::projection::{Comment, ProjectionDb, ReviewDetail, ThreadSummary};
use crate::tui::theme;

/// A thread with its comments for display
#[derive(Debug, Clone)]
pub struct ThreadWithComments {
    /// Thread summary info
    pub summary: ThreadSummary,
    /// Comments in this thread
    pub comments: Vec<Comment>,
}

/// A file section in the diff
#[derive(Debug, Clone)]
pub struct FileSection {
    /// File path
    pub path: String,
    /// Whether this section is collapsed
    pub collapsed: bool,
    /// Raw diff output (git format)
    pub raw_diff: String,
    /// Threads on this file (with their comments)
    pub threads: Vec<ThreadWithComments>,
}

/// State for the review detail view
pub struct ReviewDetailView {
    /// The review being displayed
    pub review: ReviewDetail,
    /// Target commit for the diff (resolved from change_id or final_commit)
    pub target_commit: String,
    /// File sections with their diffs
    pub files: Vec<FileSection>,
    /// Currently focused file index
    pub focused_file: usize,
    /// Scroll offset within the view
    pub scroll_offset: u16,
    /// Total content height
    pub content_height: u16,
    /// Visible height
    pub visible_height: u16,
    /// File list state (for left panel)
    pub file_list_state: ListState,
    /// Whether delta is available
    pub has_delta: bool,
    /// Whether to show side-by-side (vs unified)
    pub side_by_side: bool,
    /// Line offset for each file section (calculated during render)
    file_offsets: Vec<u16>,
}

impl ReviewDetailView {
    /// Create a new review detail view
    pub fn new(review: ReviewDetail, repo_root: &Path, db: &ProjectionDb) -> Self {
        let has_delta = which::which("delta").is_ok();

        // Get threads for this review
        let all_threads = db
            .list_threads(&review.review_id, None, None)
            .unwrap_or_default();

        // Get the diff and split by file
        let jj = JjRepo::new(repo_root);

        // Determine the target commit for the diff:
        // - For merged reviews: use final_commit
        // - For open/approved: resolve jj_change_id to its current commit
        let target_commit = review
            .final_commit
            .clone()
            .or_else(|| jj.get_commit_for_rev(&review.jj_change_id).ok())
            .unwrap_or_else(|| "@".to_string());

        let files = Self::load_file_sections(&jj, &review, &target_commit, &all_threads, db);

        let mut file_list_state = ListState::default();
        if !files.is_empty() {
            file_list_state.select(Some(0));
        }

        let file_count = files.len();
        Self {
            review,
            target_commit,
            files,
            focused_file: 0,
            scroll_offset: 0,
            content_height: 0,
            visible_height: 0,
            file_list_state,
            has_delta,
            side_by_side: true, // Default to side-by-side
            file_offsets: vec![0; file_count],
        }
    }

    /// Load file sections from the diff
    fn load_file_sections(
        jj: &JjRepo,
        review: &ReviewDetail,
        target_commit: &str,
        threads: &[ThreadSummary],
        db: &ProjectionDb,
    ) -> Vec<FileSection> {
        // Get changed files between the review's initial and target commits
        let changed_files: Vec<String> = jj
            .changed_files_between(&review.initial_commit, target_commit)
            .unwrap_or_default()
            .into_iter()
            // Filter out .crit/ metadata files - they're not part of the code review
            .filter(|f| !f.starts_with(".crit/"))
            .collect();

        let mut sections = Vec::new();

        for file_path in changed_files {
            // Get diff for this specific file
            let diff_output = jj
                .diff_git_file(&review.initial_commit, target_commit, &file_path)
                .unwrap_or_default();

            // Get threads for this file with their comments
            let file_threads: Vec<ThreadWithComments> = threads
                .iter()
                .filter(|t| t.file_path == file_path)
                .map(|t| {
                    let comments = db.list_comments(&t.thread_id).unwrap_or_default();
                    ThreadWithComments {
                        summary: t.clone(),
                        comments,
                    }
                })
                .collect();

            sections.push(FileSection {
                path: file_path,
                collapsed: false,
                raw_diff: diff_output,
                threads: file_threads,
            });
        }

        sections
    }

    /// Toggle side-by-side mode
    pub fn toggle_side_by_side(&mut self) {
        self.side_by_side = !self.side_by_side;
        // Content will be re-rendered on next draw with new mode
    }

    /// Toggle collapse state of current file
    pub fn toggle_collapse(&mut self) {
        if let Some(section) = self.files.get_mut(self.focused_file) {
            section.collapsed = !section.collapsed;
        }
    }

    /// Collapse all files
    pub fn collapse_all(&mut self) {
        for section in &mut self.files {
            section.collapsed = true;
        }
    }

    /// Expand all files
    pub fn expand_all(&mut self) {
        for section in &mut self.files {
            section.collapsed = false;
        }
    }

    /// Move to next file
    pub fn next_file(&mut self) {
        if !self.files.is_empty() {
            self.focused_file = (self.focused_file + 1) % self.files.len();
            self.file_list_state.select(Some(self.focused_file));
            self.scroll_to_focused_file();
        }
    }

    /// Move to previous file
    pub fn prev_file(&mut self) {
        if !self.files.is_empty() {
            self.focused_file = if self.focused_file == 0 {
                self.files.len() - 1
            } else {
                self.focused_file - 1
            };
            self.file_list_state.select(Some(self.focused_file));
            self.scroll_to_focused_file();
        }
    }

    /// Scroll to the currently focused file
    fn scroll_to_focused_file(&mut self) {
        if let Some(&offset) = self.file_offsets.get(self.focused_file) {
            self.scroll_offset = offset;
        }
    }

    /// Select a file by index
    pub fn select_file(&mut self, index: usize) {
        if index < self.files.len() {
            self.focused_file = index;
            self.file_list_state.select(Some(index));
            self.scroll_to_focused_file();
        }
    }

    /// Scroll down
    pub fn scroll_down(&mut self, amount: u16) {
        let max_scroll = self.content_height.saturating_sub(self.visible_height);
        self.scroll_offset = (self.scroll_offset + amount).min(max_scroll);
    }

    /// Scroll up
    pub fn scroll_up(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    /// Render the view
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // Header
                Constraint::Min(5),    // Main content
                Constraint::Length(1), // Status bar
            ])
            .split(area);

        self.render_header(frame, chunks[0]);
        self.render_main_content(frame, chunks[1]);
        self.render_status_bar(frame, chunks[2]);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let status_style = match self.review.status.as_str() {
            "open" => Style::default().fg(theme::CURRENT),
            "approved" => Style::default()
                .fg(theme::CURRENT)
                .add_modifier(Modifier::BOLD),
            "merged" => Style::default().fg(theme::RESOLVED),
            _ => Style::default().fg(theme::INACTIVE),
        };

        let header = Paragraph::new(Line::from(vec![
            Span::styled(" ← ", Style::default().fg(theme::DIM)),
            Span::styled(&self.review.review_id, Style::default().fg(theme::DIM)),
            Span::raw("  "),
            Span::styled(
                &self.review.title,
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(format!("[{}]", self.review.status), status_style),
            Span::raw("  "),
            Span::styled(&self.review.author, Style::default().fg(theme::DIM)),
        ]));
        frame.render_widget(header, area);
    }

    fn render_main_content(&mut self, frame: &mut Frame, area: Rect) {
        // Decide layout based on width
        let show_file_list = area.width > 80;

        if show_file_list {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(30), // File list
                    Constraint::Min(50),    // Diff content
                ])
                .split(area);

            self.render_file_list(frame, chunks[0]);
            self.render_diff_content(frame, chunks[1]);
        } else {
            self.render_diff_content(frame, area);
        }
    }

    fn render_file_list(&mut self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .files
            .iter()
            .map(|section| {
                let icon = if section.collapsed { "▶" } else { "▼" };
                let thread_indicator = if !section.threads.is_empty() {
                    let open_count = section
                        .threads
                        .iter()
                        .filter(|t| t.summary.status == "open")
                        .count();
                    if open_count > 0 {
                        Span::styled(
                            format!("  {}", open_count),
                            Style::default().fg(theme::STALE),
                        )
                    } else {
                        Span::styled(
                            format!(" ✓{}", section.threads.len()),
                            Style::default().fg(theme::RESOLVED),
                        )
                    }
                } else {
                    Span::raw("")
                };

                // Truncate path if needed
                let display_path = if section.path.len() > 24 {
                    format!("…{}", &section.path[section.path.len() - 23..])
                } else {
                    section.path.clone()
                };

                ListItem::new(Line::from(vec![
                    Span::raw(format!("{} ", icon)),
                    Span::raw(display_path),
                    thread_indicator,
                ]))
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .title(" Files ")
                    .borders(Borders::ALL)
                    .border_type(theme::BORDER_TYPE),
            )
            .highlight_style(Style::default().bg(theme::SELECTED_BG))
            .highlight_symbol("  ");

        frame.render_stateful_widget(list, area, &mut self.file_list_state);
    }

    fn render_diff_content(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(theme::BORDER_TYPE)
            .border_style(Style::default().fg(theme::FOCUSED));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        self.visible_height = inner.height;

        // Build combined content with file headers
        let mut lines: Vec<Line<'static>> = Vec::new();
        // Track the line offset for each file section
        let mut file_offsets: Vec<u16> = Vec::with_capacity(self.files.len());

        for (i, section) in self.files.iter().enumerate() {
            // Record the starting line for this file
            file_offsets.push(lines.len() as u16);

            // File header
            let is_focused = i == self.focused_file;
            let header_style = if is_focused {
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::FOCUSED)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme::FOCUSED)
                    .add_modifier(Modifier::BOLD)
            };

            let collapse_icon = if section.collapsed { "▶" } else { "▼" };
            let thread_info = if !section.threads.is_empty() {
                let open = section
                    .threads
                    .iter()
                    .filter(|t| t.summary.status == "open")
                    .count();
                if open > 0 {
                    format!("  {} open", open)
                } else {
                    format!("  {}", section.threads.len())
                }
            } else {
                String::new()
            };

            // Build centered file header: ─── filename (threads) ───
            let label = format!(" {} {}{} ", collapse_icon, section.path, thread_info);
            let label_len = label.chars().count();
            let total_width = inner.width as usize;
            let padding = total_width.saturating_sub(label_len);
            let left_pad = padding / 2;
            let right_pad = padding - left_pad;

            let header_line = format!("{}{}{}", "─".repeat(left_pad), label, "─".repeat(right_pad));

            lines.push(Line::from(vec![Span::styled(header_line, header_style)]));

            // File content (if not collapsed)
            if !section.collapsed {
                // Render diff through delta (or raw fallback)
                let rendered = if self.has_delta && !section.raw_diff.is_empty() {
                    render_with_delta(&section.raw_diff, self.side_by_side, inner.width)
                } else {
                    Text::raw(section.raw_diff.clone())
                };

                // Add the diff content
                for line in rendered.lines {
                    lines.push(line);
                }

                // Add thread comments inline
                for thread in &section.threads {
                    let status_style = if thread.summary.status == "open" {
                        Style::default().fg(theme::STALE)
                    } else {
                        Style::default().fg(theme::RESOLVED)
                    };

                    let status_icon = if thread.summary.status == "open" {
                        "○"
                    } else {
                        "✓"
                    };

                    // Thread header
                    lines.push(Line::from(vec![Span::styled(
                        format!(
                            "    ┌─ {} Line {}",
                            status_icon, thread.summary.selection_start
                        ),
                        status_style,
                    )]));

                    // Display each comment
                    for (i, comment) in thread.comments.iter().enumerate() {
                        let is_last = i == thread.comments.len() - 1;
                        let prefix = if is_last { "    └─" } else { "    │ " };

                        // Author line
                        lines.push(Line::from(vec![
                            Span::styled(prefix, status_style),
                            Span::styled(
                                format!(" {}: ", comment.author),
                                Style::default()
                                    .fg(theme::FOCUSED)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]));

                        // Comment body (may be multi-line)
                        for body_line in comment.body.lines() {
                            let body_prefix = if is_last { "       " } else { "    │  " };
                            lines.push(Line::from(vec![
                                Span::styled(body_prefix, status_style),
                                Span::raw(body_line.to_string()),
                            ]));
                        }
                    }

                    lines.push(Line::raw("")); // Spacer after thread
                }
            }
        }

        self.content_height = lines.len() as u16;
        // Store file offsets for scroll-to-file functionality
        self.file_offsets = file_offsets;

        // Apply scroll offset
        let visible_lines: Vec<Line> = lines
            .into_iter()
            .skip(self.scroll_offset as usize)
            .take(inner.height as usize)
            .collect();

        let paragraph = Paragraph::new(visible_lines);
        frame.render_widget(paragraph, inner);

        // Scrollbar
        if self.content_height > self.visible_height {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
            let mut scrollbar_state = ScrollbarState::new(self.content_height as usize)
                .position(self.scroll_offset as usize);
            frame.render_stateful_widget(
                scrollbar,
                area.inner(ratatui::layout::Margin {
                    vertical: 1,
                    horizontal: 0,
                }),
                &mut scrollbar_state,
            );
        }
    }

    fn render_status_bar(&self, frame: &mut Frame, area: Rect) {
        let mode = if self.side_by_side {
            "side-by-side"
        } else {
            "unified"
        };

        let hints = vec![
            ("Scroll", "j/k"),
            ("File", "Tab/[/]"),
            ("Collapse", "c"),
            ("All", "C"),
            ("Mode", "s"),
            ("Back", "q/Esc"),
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

        // Add mode indicator
        spans.push(Span::styled(
            format!("  [{}]", mode),
            Style::default().fg(theme::DIM),
        ));

        if !self.has_delta {
            spans.push(Span::styled(
                "  (delta not found)",
                Style::default().fg(theme::STALE),
            ));
        }

        let paragraph = Paragraph::new(Line::from(spans));
        frame.render_widget(paragraph, area);
    }
}

/// Render diff through delta
fn render_with_delta(diff: &str, side_by_side: bool, width: u16) -> Text<'static> {
    let mut cmd = Command::new("delta");

    if side_by_side {
        cmd.arg("--side-by-side");
    }

    cmd.arg(format!("--width={}", width))
        .arg("--paging=never")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let result = cmd.spawn().and_then(|mut child| {
        if let Some(ref mut stdin) = child.stdin {
            let _ = stdin.write_all(diff.as_bytes());
        }
        child.wait_with_output()
    });

    match result {
        Ok(output) => {
            // Convert ANSI to ratatui Text
            match output.stdout.into_text() {
                Ok(text) => text,
                Err(_) => Text::raw(diff.to_string()),
            }
        }
        Err(_) => Text::raw(diff.to_string()),
    }
}
