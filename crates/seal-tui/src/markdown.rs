//! Minimal markdown rendering for review descriptions and comments.

use crate::render_backend::{Rgba, Style};
use crate::syntax::{HighlightSpan, Highlighter};
use crate::text::{wrap_text, wrap_text_preserve};
use crate::theme::Theme;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarkdownStyle {
    Body,
    Heading,
    Quote,
    List,
    Code,
    CodeMeta,
}

impl MarkdownStyle {
    #[must_use]
    pub fn style(self, theme: &Theme, bg: Rgba) -> Style {
        match self {
            Self::Body | Self::List | Self::Code => theme.style_foreground_on(bg),
            Self::Heading => theme.style_primary_on(bg).with_bold(),
            Self::Quote | Self::CodeMeta => theme.style_muted_on(bg),
        }
    }
}

#[derive(Clone, Debug)]
pub enum MarkdownContent {
    Text(String),
    Highlighted {
        spans: Vec<HighlightSpan>,
        fallback: String,
    },
}

#[derive(Clone, Debug)]
pub struct MarkdownLine {
    pub content: MarkdownContent,
    pub style: MarkdownStyle,
}

impl MarkdownLine {
    #[must_use]
    pub fn plain(text: String, style: MarkdownStyle) -> Self {
        Self {
            content: MarkdownContent::Text(text),
            style,
        }
    }

    #[must_use]
    pub fn fallback_text(&self) -> &str {
        match &self.content {
            MarkdownContent::Text(text) => text,
            MarkdownContent::Highlighted { fallback, .. } => fallback,
        }
    }
}

#[must_use]
pub fn render_markdown(text: &str, max_width: usize) -> Vec<MarkdownLine> {
    if max_width == 0 {
        return Vec::new();
    }

    let highlighter = Highlighter::new();
    let mut code_highlighter = None;
    let mut in_code_block = false;
    let mut lines = Vec::new();

    for raw_line in text.split('\n') {
        let trimmed = raw_line.trim_end();
        if let Some(fence_info) = trimmed.strip_prefix("```") {
            if in_code_block {
                in_code_block = false;
                code_highlighter = None;
            } else {
                in_code_block = true;
                let fence_info = fence_info.trim();
                code_highlighter =
                    highlighter.for_fence_info((!fence_info.is_empty()).then_some(fence_info));
                if !fence_info.is_empty() {
                    lines.push(MarkdownLine::plain(
                        format!("[{}]", fence_info),
                        MarkdownStyle::CodeMeta,
                    ));
                }
            }
            continue;
        }

        if in_code_block {
            push_code_line(&mut lines, raw_line, max_width, code_highlighter.as_mut());
            continue;
        }

        if raw_line.trim().is_empty() {
            lines.push(MarkdownLine::plain(String::new(), MarkdownStyle::Body));
            continue;
        }

        if let Some(heading) = parse_heading(raw_line) {
            push_wrapped_plain(
                &mut lines,
                &heading,
                max_width,
                MarkdownStyle::Heading,
                None,
            );
            continue;
        }

        if let Some((prefix, body, continuation)) = parse_list_item(raw_line) {
            push_wrapped_plain(
                &mut lines,
                &body,
                max_width,
                MarkdownStyle::List,
                Some((prefix, continuation)),
            );
            continue;
        }

        if let Some(body) = raw_line.trim_start().strip_prefix('>') {
            push_wrapped_plain(
                &mut lines,
                body.trim_start(),
                max_width,
                MarkdownStyle::Quote,
                Some(("> ".to_string(), "  ".to_string())),
            );
            continue;
        }

        push_wrapped_plain(&mut lines, raw_line, max_width, MarkdownStyle::Body, None);
    }

    lines
}

fn push_code_line(
    out: &mut Vec<MarkdownLine>,
    raw_line: &str,
    max_width: usize,
    highlighter: Option<&mut crate::syntax::FileHighlighter<'_>>,
) {
    if let Some(highlighter) = highlighter {
        let spans = highlighter.highlight_line(raw_line);
        for wrapped in wrap_highlighted_line(&spans, max_width) {
            let fallback = wrapped
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>();
            out.push(MarkdownLine {
                content: MarkdownContent::Highlighted {
                    spans: wrapped,
                    fallback,
                },
                style: MarkdownStyle::Code,
            });
        }
        return;
    }

    for line in wrap_text_preserve(raw_line, max_width) {
        out.push(MarkdownLine::plain(line, MarkdownStyle::Code));
    }
}

fn push_wrapped_plain(
    out: &mut Vec<MarkdownLine>,
    text: &str,
    max_width: usize,
    style: MarkdownStyle,
    prefix: Option<(String, String)>,
) {
    if let Some((first_prefix, continuation_prefix)) = prefix {
        let available = max_width
            .saturating_sub(first_prefix.chars().count())
            .max(1);
        let wrapped = wrap_text(text, available);
        if wrapped.is_empty() {
            out.push(MarkdownLine::plain(first_prefix, style));
            return;
        }

        for (index, line) in wrapped.into_iter().enumerate() {
            let prefix = if index == 0 {
                first_prefix.as_str()
            } else {
                continuation_prefix.as_str()
            };
            out.push(MarkdownLine::plain(format!("{prefix}{line}"), style));
        }
        return;
    }

    for line in wrap_text(text, max_width) {
        out.push(MarkdownLine::plain(line, style));
    }
}

fn parse_heading(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if hashes == 0 {
        return None;
    }

    let body = trimmed[hashes..].trim_start();
    if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

fn parse_list_item(line: &str) -> Option<(String, String, String)> {
    let trimmed = line.trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(body) = trimmed.strip_prefix(marker) {
            return Some((
                marker.to_string(),
                body.trim_start().to_string(),
                "  ".to_string(),
            ));
        }
    }

    let digit_count = trimmed.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digit_count == 0 {
        return None;
    }

    let suffix = &trimmed[digit_count..];
    let body = suffix.strip_prefix(". ")?;
    let prefix = &trimmed[..digit_count + 2];
    let continuation = " ".repeat(prefix.chars().count());
    Some((
        prefix.to_string(),
        body.trim_start().to_string(),
        continuation,
    ))
}

fn split_at_char(text: &str, max_chars: usize) -> (&str, &str) {
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

fn wrap_highlighted_line(spans: &[HighlightSpan], max_width: usize) -> Vec<Vec<HighlightSpan>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_markdown_formats_code_fences() {
        let lines = render_markdown("Before\n```rust\nfn main() {}\n```\nAfter", 40);

        assert_eq!(lines.len(), 4);
        assert!(matches!(lines[1].style, MarkdownStyle::CodeMeta));
        assert!(matches!(
            lines[2].content,
            MarkdownContent::Highlighted { .. }
        ));
    }

    #[test]
    fn test_render_markdown_formats_lists() {
        let lines = render_markdown("- first item wraps nicely", 10);

        assert!(lines.len() >= 2);
        assert_eq!(lines[0].fallback_text(), "- first");
        assert!(lines[1].fallback_text().starts_with("  "));
    }
}
