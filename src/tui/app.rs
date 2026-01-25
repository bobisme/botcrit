//! Application state and logic for the TUI.

use std::path::PathBuf;

use anyhow::Result;

use crate::projection::{Comment, ProjectionDb, ReviewSummary, ThreadSummary};

/// Which panel has focus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Reviews,
    Threads,
    Comments,
}

impl Panel {
    /// Get the panel number for display
    #[must_use]
    pub const fn number(self) -> u8 {
        match self {
            Self::Reviews => 1,
            Self::Threads => 2,
            Self::Comments => 3,
        }
    }

    /// Cycle to next panel
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Reviews => Self::Threads,
            Self::Threads => Self::Comments,
            Self::Comments => Self::Reviews,
        }
    }

    /// Cycle to previous panel
    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Reviews => Self::Comments,
            Self::Threads => Self::Reviews,
            Self::Comments => Self::Threads,
        }
    }
}

/// Messages for the Elm architecture update loop
#[derive(Debug, Clone)]
pub enum Message {
    /// Move selection by delta (negative = up, positive = down)
    MoveSelection(i32),
    /// Jump to top of list
    JumpToTop,
    /// Jump to bottom of list
    JumpToBottom,
    /// Focus a specific panel
    FocusPanel(Panel),
    /// Cycle to next panel
    NextPanel,
    /// Cycle to previous panel
    PrevPanel,
    /// Toggle help overlay
    ToggleHelp,
    /// Refresh data from database
    Refresh,
    /// Quit the application
    Quit,
}

/// Application state
pub struct App {
    /// Path to repository root
    pub repo_root: PathBuf,

    /// Currently focused panel
    pub focused_panel: Panel,

    /// List of reviews
    pub reviews: Vec<ReviewSummary>,
    /// Selected review index
    pub review_index: usize,

    /// Threads for selected review
    pub threads: Vec<ThreadSummary>,
    /// Selected thread index
    pub thread_index: usize,

    /// Comments for selected thread
    pub comments: Vec<Comment>,
    /// Selected comment index
    pub comment_index: usize,

    /// Show help overlay
    pub show_help: bool,

    /// Should quit
    pub should_quit: bool,

    /// Status message (shown in status bar)
    pub status_message: Option<String>,
}

impl App {
    /// Create a new App instance
    pub fn new(repo_root: PathBuf) -> Result<Self> {
        let mut app = Self {
            repo_root,
            focused_panel: Panel::Reviews,
            reviews: Vec::new(),
            review_index: 0,
            threads: Vec::new(),
            thread_index: 0,
            comments: Vec::new(),
            comment_index: 0,
            show_help: false,
            should_quit: false,
            status_message: None,
        };
        app.refresh()?;
        Ok(app)
    }

    /// Refresh all data from the database
    pub fn refresh(&mut self) -> Result<()> {
        let db = self.open_db()?;

        // Load reviews (open ones first, then others)
        self.reviews = db.list_reviews(None, None)?;

        // Reset index if out of bounds
        if self.review_index >= self.reviews.len() {
            self.review_index = self.reviews.len().saturating_sub(1);
        }

        // Load threads for selected review
        self.refresh_threads(&db)?;

        self.status_message = Some("Refreshed".to_string());
        Ok(())
    }

    /// Refresh threads for the currently selected review
    fn refresh_threads(&mut self, db: &ProjectionDb) -> Result<()> {
        if let Some(review) = self.reviews.get(self.review_index) {
            self.threads = db.list_threads(&review.review_id, None, None)?;
        } else {
            self.threads.clear();
        }

        if self.thread_index >= self.threads.len() {
            self.thread_index = self.threads.len().saturating_sub(1);
        }

        self.refresh_comments(db)?;
        Ok(())
    }

    /// Refresh comments for the currently selected thread
    fn refresh_comments(&mut self, db: &ProjectionDb) -> Result<()> {
        if let Some(thread) = self.threads.get(self.thread_index) {
            self.comments = db.list_comments(&thread.thread_id)?;
        } else {
            self.comments.clear();
        }

        if self.comment_index >= self.comments.len() {
            self.comment_index = self.comments.len().saturating_sub(1);
        }

        Ok(())
    }

    /// Open the projection database
    fn open_db(&self) -> Result<ProjectionDb> {
        let index_path = self.repo_root.join(".crit").join("index.db");
        let events_path = self.repo_root.join(".crit").join("events.jsonl");
        let log = crate::log::open_or_create(&events_path)?;
        let db = ProjectionDb::open(&index_path)?;
        db.init_schema()?;
        crate::projection::sync_from_log(&db, &log)?;
        Ok(db)
    }

    /// Get the currently selected review (if any)
    #[must_use]
    pub fn selected_review(&self) -> Option<&ReviewSummary> {
        self.reviews.get(self.review_index)
    }

    /// Get the currently selected thread (if any)
    #[must_use]
    pub fn selected_thread(&self) -> Option<&ThreadSummary> {
        self.threads.get(self.thread_index)
    }

    /// Get the currently selected comment (if any)
    #[must_use]
    pub fn selected_comment(&self) -> Option<&Comment> {
        self.comments.get(self.comment_index)
    }

    /// Get the current list length for the focused panel
    fn current_list_len(&self) -> usize {
        match self.focused_panel {
            Panel::Reviews => self.reviews.len(),
            Panel::Threads => self.threads.len(),
            Panel::Comments => self.comments.len(),
        }
    }

    /// Get the current selection index for the focused panel
    fn current_index(&self) -> usize {
        match self.focused_panel {
            Panel::Reviews => self.review_index,
            Panel::Threads => self.thread_index,
            Panel::Comments => self.comment_index,
        }
    }

    /// Set the current selection index for the focused panel
    fn set_current_index(&mut self, index: usize) {
        match self.focused_panel {
            Panel::Reviews => self.review_index = index,
            Panel::Threads => self.thread_index = index,
            Panel::Comments => self.comment_index = index,
        }
    }

    /// Move selection by delta, clamping to valid range
    fn move_selection(&mut self, delta: i32) {
        let len = self.current_list_len();
        if len == 0 {
            return;
        }

        let current = self.current_index();
        let new_index = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs() as usize)
        } else {
            (current + delta as usize).min(len - 1)
        };

        self.set_current_index(new_index);

        // When review selection changes, refresh threads
        if self.focused_panel == Panel::Reviews && new_index != current {
            if let Ok(db) = self.open_db() {
                let _ = self.refresh_threads(&db);
            }
        }

        // When thread selection changes, refresh comments
        if self.focused_panel == Panel::Threads && new_index != current {
            if let Ok(db) = self.open_db() {
                let _ = self.refresh_comments(&db);
            }
        }
    }
}

/// Update the model based on a message (Elm architecture)
pub fn update(app: &mut App, message: Message) -> Option<Message> {
    match message {
        Message::MoveSelection(delta) => {
            app.move_selection(delta);
            None
        }
        Message::JumpToTop => {
            app.set_current_index(0);
            // Trigger refresh of dependent data
            if app.focused_panel == Panel::Reviews {
                if let Ok(db) = app.open_db() {
                    let _ = app.refresh_threads(&db);
                }
            } else if app.focused_panel == Panel::Threads {
                if let Ok(db) = app.open_db() {
                    let _ = app.refresh_comments(&db);
                }
            }
            None
        }
        Message::JumpToBottom => {
            let len = app.current_list_len();
            if len > 0 {
                app.set_current_index(len - 1);
                // Trigger refresh of dependent data
                if app.focused_panel == Panel::Reviews {
                    if let Ok(db) = app.open_db() {
                        let _ = app.refresh_threads(&db);
                    }
                } else if app.focused_panel == Panel::Threads {
                    if let Ok(db) = app.open_db() {
                        let _ = app.refresh_comments(&db);
                    }
                }
            }
            None
        }
        Message::FocusPanel(panel) => {
            app.focused_panel = panel;
            None
        }
        Message::NextPanel => {
            app.focused_panel = app.focused_panel.next();
            None
        }
        Message::PrevPanel => {
            app.focused_panel = app.focused_panel.prev();
            None
        }
        Message::ToggleHelp => {
            app.show_help = !app.show_help;
            None
        }
        Message::Refresh => {
            if let Err(e) = app.refresh() {
                app.status_message = Some(format!("Refresh failed: {e}"));
            }
            None
        }
        Message::Quit => {
            app.should_quit = true;
            None
        }
    }
}
