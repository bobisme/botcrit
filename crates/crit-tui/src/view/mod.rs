//! View rendering

mod command_palette;
mod comment_editor;
mod components;
mod diff;
mod review_detail;
mod review_list;

pub use diff::map_threads_to_diff;

use crate::render_backend::{buffer_clear, OptimizedBuffer};

use crate::model::{Model, Screen};

pub use components::Rect;

/// Render the current model state to the buffer
pub fn view(model: &Model, buffer: &mut OptimizedBuffer) {
    // Clear with background color
    buffer_clear(buffer, model.theme.background);

    match model.screen {
        Screen::ReviewList => review_list::view(model, buffer),
        Screen::ReviewDetail => review_detail::view(model, buffer),
    }

    comment_editor::view(model, buffer);
    command_palette::view(model, buffer);
}
