//! Application state and logic for the TUI.

use std::path::PathBuf;

use anyhow::Result;

use super::views::{ReviewDetailView, ReviewListView};
use crate::projection::ProjectionDb;

/// Which view is currently active
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// Review list (entry point)
    ReviewList,
    /// Review detail (diff with inline comments)
    ReviewDetail,
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
    /// Select the current item (Enter/l)
    Select,
    /// Go back to previous view (Esc/q in detail, q in list to quit)
    Back,
    /// Toggle help overlay
    ToggleHelp,
    /// Refresh data from database
    Refresh,
    /// Quit the application
    Quit,
    // Review detail specific
    /// Scroll down in diff view
    ScrollDown(u16),
    /// Scroll up in diff view
    ScrollUp(u16),
    /// Move to next file
    NextFile,
    /// Move to previous file
    PrevFile,
    /// Toggle collapse current file
    ToggleCollapse,
    /// Collapse all files
    CollapseAll,
    /// Expand all files
    ExpandAll,
    /// Toggle side-by-side mode
    ToggleSideBySide,
    /// Select a specific file by index
    SelectFile(usize),
}

/// Application state
pub struct App {
    /// Path to repository root
    pub repo_root: PathBuf,

    /// Current view mode
    pub view_mode: ViewMode,

    /// Review list view state
    pub review_list: ReviewListView,

    /// Review detail view state (only Some when viewing a review)
    pub review_detail: Option<ReviewDetailView>,

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
        let db = Self::open_db_static(&repo_root)?;
        let reviews = db.list_reviews(None, None)?;

        let mut app = Self {
            repo_root,
            view_mode: ViewMode::ReviewList,
            review_list: ReviewListView::new(reviews),
            review_detail: None,
            show_help: false,
            should_quit: false,
            status_message: None,
        };
        app.status_message = Some("Ready".to_string());
        Ok(app)
    }

    /// Refresh all data from the database
    pub fn refresh(&mut self) -> Result<()> {
        let db = self.open_db()?;
        self.review_list.reviews = db.list_reviews(None, None)?;

        // If we're in detail view, refresh that too
        if let Some(ref mut detail) = self.review_detail {
            // Reload the review detail
            if let Some(review) = db.get_review(&detail.review.review_id)? {
                self.review_detail = Some(ReviewDetailView::new(review, &self.repo_root, &db));
            }
        }

        self.status_message = Some("Refreshed".to_string());
        Ok(())
    }

    /// Open the projection database (static version for initialization)
    fn open_db_static(repo_root: &PathBuf) -> Result<ProjectionDb> {
        let crit_dir = repo_root.join(".crit");
        let index_path = crit_dir.join("index.db");
        let events_path = crit_dir.join("events.jsonl");
        let log = crate::log::open_or_create(&events_path)?;
        let db = ProjectionDb::open(&index_path)?;
        db.init_schema()?;
        crate::projection::sync_from_log_with_backup(&db, &log, Some(&crit_dir))?;
        Ok(db)
    }

    /// Open the projection database
    fn open_db(&self) -> Result<ProjectionDb> {
        Self::open_db_static(&self.repo_root)
    }

    /// Enter review detail view for the selected review
    fn enter_review_detail(&mut self) -> Result<()> {
        let Some(review_summary) = self.review_list.selected_review() else {
            return Ok(());
        };

        let db = self.open_db()?;
        let Some(review) = db.get_review(&review_summary.review_id)? else {
            self.status_message = Some("Review not found".to_string());
            return Ok(());
        };

        self.review_detail = Some(ReviewDetailView::new(review, &self.repo_root, &db));
        self.view_mode = ViewMode::ReviewDetail;
        Ok(())
    }

    /// Go back from detail to list view
    fn leave_review_detail(&mut self) {
        self.review_detail = None;
        self.view_mode = ViewMode::ReviewList;
    }
}

/// Update the model based on a message (Elm architecture)
pub fn update(app: &mut App, message: Message) -> Option<Message> {
    // Help overlay takes priority
    if app.show_help {
        if let Message::ToggleHelp = message {
            app.show_help = false;
        }
        // Any other key also dismisses help in the event handler
        return None;
    }

    match message {
        Message::ToggleHelp => {
            app.show_help = true;
            None
        }
        Message::Quit => {
            app.should_quit = true;
            None
        }
        Message::Refresh => {
            if let Err(e) = app.refresh() {
                app.status_message = Some(format!("Refresh failed: {e}"));
            }
            None
        }
        Message::Back => {
            match app.view_mode {
                ViewMode::ReviewList => {
                    // In list view, back means quit
                    app.should_quit = true;
                }
                ViewMode::ReviewDetail => {
                    app.leave_review_detail();
                }
            }
            None
        }
        Message::Select => {
            if app.view_mode == ViewMode::ReviewList {
                if let Err(e) = app.enter_review_detail() {
                    app.status_message = Some(format!("Error: {e}"));
                }
            }
            None
        }
        Message::MoveSelection(delta) => {
            match app.view_mode {
                ViewMode::ReviewList => {
                    if delta > 0 {
                        app.review_list.move_down();
                    } else {
                        app.review_list.move_up();
                    }
                }
                ViewMode::ReviewDetail => {
                    // In detail view, j/k scrolls
                    if let Some(ref mut detail) = app.review_detail {
                        if delta > 0 {
                            detail.scroll_down(1);
                        } else {
                            detail.scroll_up(1);
                        }
                    }
                }
            }
            None
        }
        Message::JumpToTop => {
            match app.view_mode {
                ViewMode::ReviewList => {
                    app.review_list.jump_to_top();
                }
                ViewMode::ReviewDetail => {
                    if let Some(ref mut detail) = app.review_detail {
                        detail.scroll_offset = 0;
                    }
                }
            }
            None
        }
        Message::JumpToBottom => {
            match app.view_mode {
                ViewMode::ReviewList => {
                    app.review_list.jump_to_bottom();
                }
                ViewMode::ReviewDetail => {
                    if let Some(ref mut detail) = app.review_detail {
                        let max = detail.content_height.saturating_sub(detail.visible_height);
                        detail.scroll_offset = max;
                    }
                }
            }
            None
        }
        Message::ScrollDown(amount) => {
            if let Some(ref mut detail) = app.review_detail {
                detail.scroll_down(amount);
            }
            None
        }
        Message::ScrollUp(amount) => {
            if let Some(ref mut detail) = app.review_detail {
                detail.scroll_up(amount);
            }
            None
        }
        Message::NextFile => {
            if let Some(ref mut detail) = app.review_detail {
                detail.next_file();
            }
            None
        }
        Message::PrevFile => {
            if let Some(ref mut detail) = app.review_detail {
                detail.prev_file();
            }
            None
        }
        Message::ToggleCollapse => {
            if let Some(ref mut detail) = app.review_detail {
                detail.toggle_collapse();
            }
            None
        }
        Message::CollapseAll => {
            if let Some(ref mut detail) = app.review_detail {
                detail.collapse_all();
            }
            None
        }
        Message::ExpandAll => {
            if let Some(ref mut detail) = app.review_detail {
                detail.expand_all();
            }
            None
        }
        Message::ToggleSideBySide => {
            if let Some(ref mut detail) = app.review_detail {
                detail.toggle_side_by_side();
            }
            None
        }
        Message::SelectFile(index) => {
            if let Some(ref mut detail) = app.review_detail {
                detail.select_file(index);
            }
            None
        }
    }
}
