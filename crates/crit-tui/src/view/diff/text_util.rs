//! Text wrapping, truncation, and highlighted-text drawing utilities.

use crate::render_backend::{buffer_draw_text, OptimizedBuffer, Rgba, Style};

use crate::syntax::HighlightSpan;
use crate::text::wrap_text_preserve;

// --- Character-level helpers ---

pub(super) fn split_at_char(text: &str, max_chars: usize) -> (&str, &str) {
    if max_chars == 0 {
        return ("", text);
    }
    for (count, (idx, _)) in text.char_indices().enumerate() {
        if count == max_chars {
            return (&text[..idx], &text[idx..]);
        }
    }
    (text, "")
}

/// Truncate a string to at most `max_chars` characters, respecting UTF-8 char boundaries.
pub(super) fn truncate_chars(text: &str, max_chars: usize) -> &str {
    split_at_char(text, max_chars).0
}

// --- Wrapping ---

pub(super) enum WrappedLine {
    Spans(Vec<HighlightSpan>),
    Text(String),
}

pub(super) fn wrap_highlight_spans(
    spans: &[HighlightSpan],
    max_width: usize,
) -> Vec<Vec<HighlightSpan>> {
    if max_width == 0 {
        return Vec::new();
    }
    let mut lines: Vec<Vec<HighlightSpan>> = Vec::new();
    let mut current: Vec<HighlightSpan> = Vec::new();
    let mut width = 0usize;

    for span in spans {
        let mut remaining = span.text.as_str();
        while !remaining.is_empty() {
            let available = max_width.saturating_sub(width);
            if available == 0 {
                lines.push(current);
                current = Vec::new();
                width = 0;
                continue;
            }
            let (chunk, rest) = split_at_char(remaining, available);
            if !chunk.is_empty() {
                current.push(HighlightSpan {
                    text: chunk.to_string(),
                    fg: span.fg,
                    bold: span.bold,
                    italic: span.italic,
                });
                width += chunk.chars().count();
            }
            remaining = rest;
            if width >= max_width {
                lines.push(current);
                current = Vec::new();
                width = 0;
            }
        }
    }

    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }

    lines
}

pub(super) fn wrap_content(
    spans: Option<&Vec<HighlightSpan>>,
    text: &str,
    max_width: usize,
) -> Vec<WrappedLine> {
    if max_width == 0 {
        return vec![WrappedLine::Text(String::new())];
    }
    if let Some(spans) = spans {
        if spans.is_empty() {
            return wrap_text_preserve(text, max_width)
                .into_iter()
                .map(WrappedLine::Text)
                .collect();
        }
        let wrapped = wrap_highlight_spans(spans, max_width);
        return wrapped.into_iter().map(WrappedLine::Spans).collect();
    }

    wrap_text_preserve(text, max_width)
        .into_iter()
        .map(WrappedLine::Text)
        .collect()
}

// --- Drawing ---

pub(super) fn draw_wrapped_line(
    buffer: &mut OptimizedBuffer,
    x: u32,
    y: u32,
    max_width: u32,
    line: &WrappedLine,
    fallback_fg: Rgba,
    bg: Rgba,
) {
    match line {
        WrappedLine::Spans(spans) => {
            draw_highlighted_text(
                buffer,
                x,
                y,
                max_width,
                &HighlightContent {
                    spans: Some(spans),
                    fallback_text: "",
                    fallback_fg,
                    bg,
                },
            );
        }
        WrappedLine::Text(text) => {
            draw_highlighted_text(
                buffer,
                x,
                y,
                max_width,
                &HighlightContent {
                    spans: None,
                    fallback_text: text,
                    fallback_fg,
                    bg,
                },
            );
        }
    }
}

/// Content parameters for highlighted text rendering.
pub(super) struct HighlightContent<'a> {
    pub spans: Option<&'a Vec<HighlightSpan>>,
    pub fallback_text: &'a str,
    pub fallback_fg: Rgba,
    pub bg: Rgba,
}

pub(super) fn draw_highlighted_text(
    buffer: &mut OptimizedBuffer,
    x: u32,
    y: u32,
    max_width: u32,
    content: &HighlightContent<'_>,
) {
    let max_chars = max_width as usize;

    let bg = content.bg;
    if let Some(spans) = content.spans {
        if spans.is_empty() {
            let text = truncate_chars(content.fallback_text, max_chars);
            buffer_draw_text(
                buffer,
                x,
                y,
                text,
                Style::fg(content.fallback_fg).with_bg(bg),
            );
            return;
        }

        let mut col = x;
        let mut chars_drawn = 0;
        for span in spans {
            if chars_drawn >= max_chars {
                break;
            }
            let remaining = max_chars - chars_drawn;
            let span_char_count = span.text.chars().count();
            let text = if span_char_count > remaining {
                truncate_chars(&span.text, remaining)
            } else {
                &span.text
            };
            if !text.is_empty() {
                let drawn = text.chars().count();
                buffer_draw_text(buffer, col, y, text, Style::fg(span.fg).with_bg(bg));
                col += drawn as u32;
                chars_drawn += drawn;
            }
        }
    } else {
        let text = truncate_chars(content.fallback_text, max_chars);
        buffer_draw_text(
            buffer,
            x,
            y,
            text,
            Style::fg(content.fallback_fg).with_bg(bg),
        );
    }
}
