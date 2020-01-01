//! Terminal User Interface for crit.
//!
//! Provides an interactive TUI for browsing and managing code reviews.

mod app;
mod theme;
mod ui;
mod views;

use std::io::{self, Stdout};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use ratatui::{backend::CrosstermBackend, Terminal};

use app::{update, App, Message, ViewMode};

/// Run the TUI application
pub fn run(repo_root: &Path) -> Result<()> {
    // Setup terminal
    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("Failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("Failed to create terminal")?;

    // Create app
    let mut app = App::new(repo_root.to_path_buf())?;

    // Run main loop
    let result = run_loop(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode().context("Failed to disable raw mode")?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .context("Failed to leave alternate screen")?;
    terminal.show_cursor().context("Failed to show cursor")?;

    result
}

/// Main event loop
fn run_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    while !app.should_quit {
        // Draw
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Handle events with timeout
        if event::poll(Duration::from_millis(100))? {
            if let Some(message) = handle_event(event::read()?, terminal, app)? {
                // Process message and any follow-up messages
                let mut next = Some(message);
                while let Some(msg) = next {
                    next = update(app, msg);
                }
            }
        }
    }

    Ok(())
}

/// Handle an event and return a message (if any)
fn handle_event(
    event: Event,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &App,
) -> Result<Option<Message>> {
    match event {
        Event::Key(key) => Ok(handle_key(key.code, key.modifiers, terminal, app)?),
        Event::Mouse(mouse) => Ok(handle_mouse(mouse, app)),
        _ => Ok(None),
    }
}

/// Handle keyboard input
fn handle_key(
    code: KeyCode,
    modifiers: KeyModifiers,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &App,
) -> Result<Option<Message>> {
    // Help overlay takes priority - any key dismisses it
    if app.show_help {
        return Ok(Some(Message::ToggleHelp));
    }

    let ctrl = modifiers.contains(KeyModifiers::CONTROL);
    let shift = modifiers.contains(KeyModifiers::SHIFT);

    // Global keys
    match code {
        // Suspend (Ctrl+Z)
        KeyCode::Char('z') if ctrl => {
            suspend(terminal)?;
            return Ok(None);
        }
        // Help
        KeyCode::Char('?') => return Ok(Some(Message::ToggleHelp)),
        // Refresh
        KeyCode::Char('R') => return Ok(Some(Message::Refresh)),
        _ => {}
    }

    // View-specific keys
    match app.view_mode {
        ViewMode::ReviewList => handle_list_key(code, modifiers),
        ViewMode::ReviewDetail => handle_detail_key(code, shift),
    }
}

/// Handle keys in review list view
fn handle_list_key(code: KeyCode, _modifiers: KeyModifiers) -> Result<Option<Message>> {
    match code {
        // Quit
        KeyCode::Char('q') => Ok(Some(Message::Quit)),

        // Navigation
        KeyCode::Char('j') | KeyCode::Down => Ok(Some(Message::MoveSelection(1))),
        KeyCode::Char('k') | KeyCode::Up => Ok(Some(Message::MoveSelection(-1))),
        KeyCode::Char('g') => Ok(Some(Message::JumpToTop)),
        KeyCode::Char('G') => Ok(Some(Message::JumpToBottom)),

        // Select review
        KeyCode::Enter | KeyCode::Char('l') => Ok(Some(Message::Select)),

        _ => Ok(None),
    }
}

/// Handle keys in review detail view
fn handle_detail_key(code: KeyCode, shift: bool) -> Result<Option<Message>> {
    match code {
        // Back
        KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('h') => Ok(Some(Message::Back)),

        // Scroll
        KeyCode::Char('j') | KeyCode::Down => Ok(Some(Message::ScrollDown(1))),
        KeyCode::Char('k') | KeyCode::Up => Ok(Some(Message::ScrollUp(1))),
        KeyCode::Char('d') => Ok(Some(Message::ScrollDown(10))), // Half page
        KeyCode::Char('u') => Ok(Some(Message::ScrollUp(10))),   // Half page
        KeyCode::Char('g') => Ok(Some(Message::JumpToTop)),
        KeyCode::Char('G') => Ok(Some(Message::JumpToBottom)),

        // File navigation
        KeyCode::Tab => Ok(Some(Message::NextFile)),
        KeyCode::BackTab => Ok(Some(Message::PrevFile)),
        KeyCode::Char(']') => Ok(Some(Message::NextFile)),
        KeyCode::Char('[') => Ok(Some(Message::PrevFile)),

        // Collapse
        KeyCode::Char('c') if !shift => Ok(Some(Message::ToggleCollapse)),
        KeyCode::Char('C') => Ok(Some(Message::CollapseAll)),
        KeyCode::Char('e') => Ok(Some(Message::ExpandAll)),

        // Side-by-side toggle
        KeyCode::Char('s') => Ok(Some(Message::ToggleSideBySide)),

        _ => Ok(None),
    }
}

/// Handle mouse events
fn handle_mouse(mouse: crossterm::event::MouseEvent, app: &App) -> Option<Message> {
    // Only handle left clicks
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return None;
    }

    // In review detail view, check if click is in the file list area
    if app.view_mode == ViewMode::ReviewDetail {
        // File list is in the left panel (first 30 columns when width > 80)
        // The file list starts at row 3 (after header) and column 1-29
        if mouse.column < 30 && mouse.row >= 3 {
            // Calculate which file was clicked (subtract header rows, borders)
            let file_row = mouse.row.saturating_sub(4) as usize;
            if let Some(ref detail) = app.review_detail {
                if file_row < detail.files.len() {
                    return Some(Message::SelectFile(file_row));
                }
            }
        }
    }

    None
}

/// Suspend the TUI (Ctrl+Z support)
fn suspend(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    // Restore terminal before suspending
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    // Send SIGTSTP to suspend
    signal::kill(Pid::this(), Signal::SIGTSTP)?;

    // Re-setup terminal when resumed
    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    terminal.clear()?;

    Ok(())
}
