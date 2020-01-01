//! Theme system for botcrit-ui
//!
//! Themes are defined by 7 seed colors. All other colors are derived
//! automatically using `lerp/blend_over`. Individual derived colors can
//! be overridden for fine-tuning.

use std::path::Path;

use crate::render_backend::{
    color_blend_over, color_from_hex, color_lerp, color_luminance, color_with_alpha, Rgba, Style,
};
use serde::{Deserialize, Serialize};

use crate::syntax::SyntaxColors;

/// Diff-specific color tokens
#[derive(Debug, Clone)]
pub struct DiffTheme {
    /// Text color for added lines
    pub added: Rgba,
    /// Text color for removed lines
    pub removed: Rgba,
    /// Text color for context lines
    pub context: Rgba,
    /// Color for @@ hunk headers
    pub hunk_header: Rgba,

    /// Bright highlight for + signs
    pub highlight_added: Rgba,
    /// Bright highlight for - signs
    pub highlight_removed: Rgba,

    /// Background for added lines
    pub added_bg: Rgba,
    /// Background for removed lines
    pub removed_bg: Rgba,
    /// Background for context lines
    pub context_bg: Rgba,

    /// Line number text color
    pub line_number: Rgba,
    /// Line number bg for added lines
    pub added_line_number_bg: Rgba,
    /// Line number bg for removed lines
    pub removed_line_number_bg: Rgba,
}

/// Complete theme definition
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,

    // Base colors
    pub background: Rgba,
    pub foreground: Rgba,

    // UI chrome
    pub border: Rgba,
    pub border_focused: Rgba,
    pub panel_bg: Rgba,

    // Selection/highlighting
    pub selection_bg: Rgba,
    pub selection_fg: Rgba,
    pub cursor: Rgba,

    // Semantic colors
    pub primary: Rgba,
    pub success: Rgba,
    pub warning: Rgba,
    pub error: Rgba,
    pub muted: Rgba,

    // Diff colors
    pub diff: DiffTheme,

    // Syntax highlighting colors
    pub syntax: SyntaxColors,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

// ---------------------------------------------------------------------------
// Style token methods
// ---------------------------------------------------------------------------

impl Theme {
    /// `Style::fg(self.muted)`
    #[must_use]
    pub const fn style_muted(&self) -> Style {
        Style::fg(self.muted)
    }

    /// `Style::fg(self.muted).with_bg(bg)`
    #[must_use]
    pub const fn style_muted_on(&self, bg: Rgba) -> Style {
        Style::fg(self.muted).with_bg(bg)
    }

    /// `Style::fg(self.foreground)`
    #[must_use]
    pub const fn style_foreground(&self) -> Style {
        Style::fg(self.foreground)
    }

    /// `Style::fg(self.foreground).with_bg(bg)`
    #[must_use]
    pub const fn style_foreground_on(&self, bg: Rgba) -> Style {
        Style::fg(self.foreground).with_bg(bg)
    }

    /// `Style::fg(self.primary)`
    #[must_use]
    pub const fn style_primary(&self) -> Style {
        Style::fg(self.primary)
    }

    /// `Style::fg(self.primary).with_bg(bg)`
    #[must_use]
    pub const fn style_primary_on(&self, bg: Rgba) -> Style {
        Style::fg(self.primary).with_bg(bg)
    }
}

impl DiffTheme {
    /// `Style::fg(self.line_number).with_bg(bg)`
    #[must_use]
    pub const fn style_line_number(&self, bg: Rgba) -> Style {
        Style::fg(self.line_number).with_bg(bg)
    }
}

// ---------------------------------------------------------------------------
// Seed-based theme construction
// ---------------------------------------------------------------------------

/// The 7 seed colors that define a theme.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeSeeds {
    pub background: String,
    pub foreground: String,
    pub primary: String,
    pub muted: String,
    pub success: String,
    pub warning: String,
    pub error: String,
}

/// Optional overrides for any derived color.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThemeOverrides {
    pub panel_bg: Option<String>,
    pub selection_bg: Option<String>,
    pub selection_fg: Option<String>,
    pub border: Option<String>,
    pub border_focused: Option<String>,
    pub cursor: Option<String>,
    // Diff
    pub diff_added: Option<String>,
    pub diff_removed: Option<String>,
    pub diff_context: Option<String>,
    pub diff_hunk_header: Option<String>,
    pub diff_highlight_added: Option<String>,
    pub diff_highlight_removed: Option<String>,
    pub diff_added_bg: Option<String>,
    pub diff_removed_bg: Option<String>,
    pub diff_context_bg: Option<String>,
    pub diff_line_number: Option<String>,
    pub diff_added_line_number_bg: Option<String>,
    pub diff_removed_line_number_bg: Option<String>,
    // Syntax
    pub syntax_keyword: Option<String>,
    pub syntax_function: Option<String>,
    pub syntax_type_name: Option<String>,
    pub syntax_string: Option<String>,
    pub syntax_number: Option<String>,
    pub syntax_comment: Option<String>,
    pub syntax_operator: Option<String>,
    pub syntax_punctuation: Option<String>,
    pub syntax_variable: Option<String>,
    pub syntax_constant: Option<String>,
    pub syntax_attribute: Option<String>,
}

impl Theme {
    /// Build a complete theme from 7 seed colors, deriving everything else.
    ///
    /// # Errors
    ///
    /// Returns an error if any seed or override color string fails to parse.
    #[allow(clippy::similar_names)]
    pub fn from_seeds(
        name: String,
        seeds: &ThemeSeeds,
        overrides: Option<&ThemeOverrides>,
    ) -> anyhow::Result<Self> {
        let bg = parse_color(&seeds.background)?;
        let fg = parse_color(&seeds.foreground)?;
        let primary = parse_color(&seeds.primary)?;
        let muted = parse_color(&seeds.muted)?;
        let success = parse_color(&seeds.success)?;
        let warning = parse_color(&seeds.warning)?;
        let error = parse_color(&seeds.error)?;

        let is_dark = color_luminance(bg) < 0.5;

        // --- Derive UI chrome ---
        let mut panel_bg = color_lerp(bg, fg, 0.05);
        let mut selection_bg = color_blend_over(color_with_alpha(primary, 0.25), bg);
        let mut selection_fg = fg;
        let mut border = color_lerp(bg, fg, 0.15);
        let mut border_focused = primary;
        let mut cursor = fg;

        // --- Derive diff colors ---
        let mut diff = DiffTheme {
            added: color_lerp(success, fg, 0.3),
            removed: color_lerp(error, fg, 0.3),
            context: fg,
            hunk_header: muted,
            highlight_added: color_lerp(success, primary, 0.3),
            highlight_removed: color_lerp(error, fg, 0.15),
            added_bg: color_blend_over(color_with_alpha(success, 0.08), bg),
            removed_bg: color_blend_over(color_with_alpha(error, 0.08), bg),
            context_bg: bg,
            line_number: muted,
            added_line_number_bg: color_blend_over(color_with_alpha(success, 0.05), bg),
            removed_line_number_bg: color_blend_over(color_with_alpha(error, 0.05), bg),
        };

        // --- Syntax defaults based on lightness ---
        let mut syntax = if is_dark {
            SyntaxColors::tokyo_night()
        } else {
            SyntaxColors::light()
        };

        // --- Apply overrides ---
        if let Some(ov) = overrides {
            apply_override(&mut panel_bg, ov.panel_bg.as_ref())?;
            apply_override(&mut selection_bg, ov.selection_bg.as_ref())?;
            apply_override(&mut selection_fg, ov.selection_fg.as_ref())?;
            apply_override(&mut border, ov.border.as_ref())?;
            apply_override(&mut border_focused, ov.border_focused.as_ref())?;
            apply_override(&mut cursor, ov.cursor.as_ref())?;

            apply_override(&mut diff.added, ov.diff_added.as_ref())?;
            apply_override(&mut diff.removed, ov.diff_removed.as_ref())?;
            apply_override(&mut diff.context, ov.diff_context.as_ref())?;
            apply_override(&mut diff.hunk_header, ov.diff_hunk_header.as_ref())?;
            apply_override(&mut diff.highlight_added, ov.diff_highlight_added.as_ref())?;
            apply_override(
                &mut diff.highlight_removed,
                ov.diff_highlight_removed.as_ref(),
            )?;
            apply_override(&mut diff.added_bg, ov.diff_added_bg.as_ref())?;
            apply_override(&mut diff.removed_bg, ov.diff_removed_bg.as_ref())?;
            apply_override(&mut diff.context_bg, ov.diff_context_bg.as_ref())?;
            apply_override(&mut diff.line_number, ov.diff_line_number.as_ref())?;
            apply_override(
                &mut diff.added_line_number_bg,
                ov.diff_added_line_number_bg.as_ref(),
            )?;
            apply_override(
                &mut diff.removed_line_number_bg,
                ov.diff_removed_line_number_bg.as_ref(),
            )?;

            apply_override(&mut syntax.keyword, ov.syntax_keyword.as_ref())?;
            apply_override(&mut syntax.function, ov.syntax_function.as_ref())?;
            apply_override(&mut syntax.type_name, ov.syntax_type_name.as_ref())?;
            apply_override(&mut syntax.string, ov.syntax_string.as_ref())?;
            apply_override(&mut syntax.number, ov.syntax_number.as_ref())?;
            apply_override(&mut syntax.comment, ov.syntax_comment.as_ref())?;
            apply_override(&mut syntax.operator, ov.syntax_operator.as_ref())?;
            apply_override(&mut syntax.punctuation, ov.syntax_punctuation.as_ref())?;
            apply_override(&mut syntax.variable, ov.syntax_variable.as_ref())?;
            apply_override(&mut syntax.constant, ov.syntax_constant.as_ref())?;
            apply_override(&mut syntax.attribute, ov.syntax_attribute.as_ref())?;
        }

        Ok(Self {
            name,
            background: bg,
            foreground: fg,
            border,
            border_focused,
            panel_bg,
            selection_bg,
            selection_fg,
            cursor,
            primary,
            success,
            warning,
            error,
            muted,
            diff,
            syntax,
        })
    }

    /// Default dark theme (Tokyo Night inspired).
    ///
    /// # Panics
    ///
    /// Panics if the built-in dark theme seed colors are invalid.
    #[must_use]
    pub fn dark() -> Self {
        Self::from_seeds(
            "dark".to_string(),
            &ThemeSeeds {
                background: "#1a1b26".into(),
                foreground: "#c0caf5".into(),
                primary: "#7aa2f7".into(),
                muted: "#565f89".into(),
                success: "#9ece6a".into(),
                warning: "#e0af68".into(),
                error: "#f7768e".into(),
            },
            Some(&ThemeOverrides {
                syntax_keyword: Some("#bb9af7".into()),
                syntax_function: Some("#7aa2f7".into()),
                syntax_type_name: Some("#2ac3de".into()),
                syntax_string: Some("#9ece6a".into()),
                syntax_number: Some("#ff9e64".into()),
                syntax_comment: Some("#565f89".into()),
                syntax_operator: Some("#89ddff".into()),
                syntax_punctuation: Some("#a9b1d6".into()),
                syntax_variable: Some("#c0caf5".into()),
                syntax_constant: Some("#ff9e64".into()),
                syntax_attribute: Some("#bb9af7".into()),
                ..Default::default()
            }),
        )
        .expect("built-in dark theme seeds are valid")
    }

    /// Light theme variant.
    ///
    /// # Panics
    ///
    /// Panics if the built-in light theme seed colors are invalid.
    #[must_use]
    pub fn light() -> Self {
        Self::from_seeds(
            "light".to_string(),
            &ThemeSeeds {
                background: "#d5d6db".into(),
                foreground: "#343b58".into(),
                primary: "#34548a".into(),
                muted: "#6a6f87".into(),
                success: "#485e30".into(),
                warning: "#8f5e15".into(),
                error: "#8c4351".into(),
            },
            Some(&ThemeOverrides {
                syntax_keyword: Some("#5c21a5".into()),
                syntax_function: Some("#0550ae".into()),
                syntax_type_name: Some("#0969da".into()),
                syntax_string: Some("#0a3069".into()),
                syntax_number: Some("#953800".into()),
                syntax_comment: Some("#6e7781".into()),
                syntax_operator: Some("#0550ae".into()),
                syntax_punctuation: Some("#24292f".into()),
                syntax_variable: Some("#24292f".into()),
                syntax_constant: Some("#953800".into()),
                syntax_attribute: Some("#5c21a5".into()),
                ..Default::default()
            }),
        )
        .expect("built-in light theme seeds are valid")
    }
}

// ---------------------------------------------------------------------------
// JSON theme file formats
// ---------------------------------------------------------------------------

/// Seed-based theme file format (new).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeSeedFile {
    pub name: String,
    #[serde(rename = "syntaxTheme")]
    pub syntax_theme: Option<String>,
    pub seeds: ThemeSeeds,
    #[serde(default)]
    pub overrides: Option<ThemeOverrides>,
}

/// Legacy JSON theme format with all colors explicit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeFile {
    pub name: String,
    pub colors: ThemeColors,
    #[serde(rename = "syntaxTheme")]
    pub syntax_theme: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeColors {
    pub background: String,
    pub foreground: String,
    pub border: String,
    pub border_focused: String,
    pub panel_bg: String,
    pub selection_bg: String,
    pub selection_fg: String,
    pub cursor: String,
    pub primary: String,
    pub success: String,
    pub warning: String,
    pub error: String,
    pub muted: String,

    // Diff colors
    pub diff_added: String,
    pub diff_removed: String,
    pub diff_context: String,
    pub diff_hunk_header: String,
    pub diff_highlight_added: String,
    pub diff_highlight_removed: String,
    pub diff_added_bg: String,
    pub diff_removed_bg: String,
    pub diff_context_bg: String,
    pub diff_line_number: String,
    pub diff_added_line_number_bg: String,
    pub diff_removed_line_number_bg: String,

    // Optional syntax colors
    pub syntax_keyword: Option<String>,
    pub syntax_function: Option<String>,
    pub syntax_type_name: Option<String>,
    pub syntax_string: Option<String>,
    pub syntax_number: Option<String>,
    pub syntax_comment: Option<String>,
    pub syntax_operator: Option<String>,
    pub syntax_punctuation: Option<String>,
    pub syntax_variable: Option<String>,
    pub syntax_constant: Option<String>,
    pub syntax_attribute: Option<String>,
}

impl TryFrom<ThemeFile> for Theme {
    type Error = anyhow::Error;

    fn try_from(file: ThemeFile) -> Result<Self, Self::Error> {
        let c = &file.colors;
        let is_light = file.name.to_lowercase().contains("light");
        let mut syntax = if is_light {
            SyntaxColors::light()
        } else {
            SyntaxColors::tokyo_night()
        };
        apply_override(&mut syntax.keyword, c.syntax_keyword.as_ref())?;
        apply_override(&mut syntax.function, c.syntax_function.as_ref())?;
        apply_override(&mut syntax.type_name, c.syntax_type_name.as_ref())?;
        apply_override(&mut syntax.string, c.syntax_string.as_ref())?;
        apply_override(&mut syntax.number, c.syntax_number.as_ref())?;
        apply_override(&mut syntax.comment, c.syntax_comment.as_ref())?;
        apply_override(&mut syntax.operator, c.syntax_operator.as_ref())?;
        apply_override(&mut syntax.punctuation, c.syntax_punctuation.as_ref())?;
        apply_override(&mut syntax.variable, c.syntax_variable.as_ref())?;
        apply_override(&mut syntax.constant, c.syntax_constant.as_ref())?;
        apply_override(&mut syntax.attribute, c.syntax_attribute.as_ref())?;
        Ok(Self {
            name: file.name,
            background: parse_color(&c.background)?,
            foreground: parse_color(&c.foreground)?,
            border: parse_color(&c.border)?,
            border_focused: parse_color(&c.border_focused)?,
            panel_bg: parse_color(&c.panel_bg)?,
            selection_bg: parse_color(&c.selection_bg)?,
            selection_fg: parse_color(&c.selection_fg)?,
            cursor: parse_color(&c.cursor)?,
            primary: parse_color(&c.primary)?,
            success: parse_color(&c.success)?,
            warning: parse_color(&c.warning)?,
            error: parse_color(&c.error)?,
            muted: parse_color(&c.muted)?,
            diff: DiffTheme {
                added: parse_color(&c.diff_added)?,
                removed: parse_color(&c.diff_removed)?,
                context: parse_color(&c.diff_context)?,
                hunk_header: parse_color(&c.diff_hunk_header)?,
                highlight_added: parse_color(&c.diff_highlight_added)?,
                highlight_removed: parse_color(&c.diff_highlight_removed)?,
                added_bg: parse_color(&c.diff_added_bg)?,
                removed_bg: parse_color(&c.diff_removed_bg)?,
                context_bg: parse_color(&c.diff_context_bg)?,
                line_number: parse_color(&c.diff_line_number)?,
                added_line_number_bg: parse_color(&c.diff_added_line_number_bg)?,
                removed_line_number_bg: parse_color(&c.diff_removed_line_number_bg)?,
            },
            syntax,
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_color(hex: &str) -> anyhow::Result<Rgba> {
    color_from_hex(hex).ok_or_else(|| anyhow::anyhow!("Invalid hex color: {hex}"))
}

fn apply_override(target: &mut Rgba, source: Option<&String>) -> anyhow::Result<()> {
    if let Some(hex) = source {
        *target = parse_color(hex)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ThemeLoadResult {
    pub theme: Theme,
    pub syntax_theme: Option<String>,
}

const BUILTIN_THEMES: &[(&str, &str)] = &[
    (
        "default-dark",
        include_str!("../../themes/default-dark.json"),
    ),
    (
        "default-light",
        include_str!("../../themes/default-light.json"),
    ),
    ("catppuccin", include_str!("../../themes/catppuccin.json")),
    ("dracula", include_str!("../../themes/dracula.json")),
    ("gruvbox", include_str!("../../themes/gruvbox.json")),
    ("nord", include_str!("../../themes/nord.json")),
    ("solarized", include_str!("../../themes/solarized.json")),
    ("monokai", include_str!("../../themes/monokai.json")),
    ("ayu", include_str!("../../themes/ayu.json")),
    ("vesper", include_str!("../../themes/vesper.json")),
];

/// Load a theme from a JSON file on disk.
///
/// # Errors
///
/// Returns an error if the file cannot be read or contains invalid theme JSON.
pub fn load_theme_from_path(path: &Path) -> anyhow::Result<ThemeLoadResult> {
    let json = std::fs::read_to_string(path)?;
    load_theme_from_str(&json)
}

/// Parse a theme from a JSON string (seed or legacy format).
///
/// # Errors
///
/// Returns an error if the JSON is malformed or contains invalid color values.
pub fn load_theme_from_str(json: &str) -> anyhow::Result<ThemeLoadResult> {
    // Detect format: "seeds" key → new seed format, "colors" key → legacy
    let value: serde_json::Value = serde_json::from_str(json)?;

    if value.get("seeds").is_some() {
        let seed_file: ThemeSeedFile = serde_json::from_value(value)?;
        let syntax_theme = seed_file.syntax_theme.clone();
        let theme = Theme::from_seeds(
            seed_file.name,
            &seed_file.seeds,
            seed_file.overrides.as_ref(),
        )?;
        Ok(ThemeLoadResult {
            theme,
            syntax_theme,
        })
    } else {
        let theme_file: ThemeFile = serde_json::from_value(value)?;
        let syntax_theme = theme_file.syntax_theme.clone();
        let theme = Theme::try_from(theme_file)?;
        Ok(ThemeLoadResult {
            theme,
            syntax_theme,
        })
    }
}

#[must_use]
pub fn load_built_in_theme(name: &str) -> Option<ThemeLoadResult> {
    BUILTIN_THEMES
        .iter()
        .find(|(theme_name, _)| *theme_name == name)
        .and_then(|(_, json)| load_theme_from_str(json).ok())
}

#[must_use]
pub fn built_in_theme_names() -> Vec<&'static str> {
    BUILTIN_THEMES.iter().map(|(name, _)| *name).collect()
}
