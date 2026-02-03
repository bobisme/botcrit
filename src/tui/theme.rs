//! Theme constants for the TUI.

use ratatui::style::Color;
use ratatui::widgets::BorderType;

/// Border type for all panels
pub const BORDER_TYPE: BorderType = BorderType::Rounded;

/// Focused panel border color
pub const FOCUSED: Color = Color::Green;

/// Selected item background
pub const SELECTED_BG: Color = Color::DarkGray;

/// Open/current/active items
pub const CURRENT: Color = Color::Green;

/// Resolved/merged items
pub const RESOLVED: Color = Color::Blue;

/// Stale/warning items (e.g., drift detected)
pub const STALE: Color = Color::Yellow;

/// Error/conflict items (reserved for future use)
#[allow(dead_code)]
pub const CONFLICT: Color = Color::Red;

/// Abandoned/inactive items
pub const INACTIVE: Color = Color::DarkGray;

/// Secondary/dim text
pub const DIM: Color = Color::DarkGray;

/// Status bar color
pub const STATUS_BAR: Color = Color::Blue;
