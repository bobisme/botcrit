//! Output formatting module for botcrit
//!
//! Provides TOON (human-readable) and JSON output formats for CLI output.

use anyhow::Result;
use serde::Serialize;
use std::io::{self, Write};

/// Output format selection
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    /// TOON format - compact token-oriented notation
    #[default]
    Toon,
    /// JSON format - machine-readable output
    Json,
    /// Plain text format - simple text output
    Text,
}

/// Formatter that can output data in TOON or JSON format
#[derive(Debug, Clone)]
pub struct Formatter {
    format: OutputFormat,
}

impl Formatter {
    /// Create a new formatter with the specified output format
    #[must_use]
    pub const fn new(format: OutputFormat) -> Self {
        Self { format }
    }

    /// Format data according to the configured output format
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails
    pub fn format<T: Serialize>(&self, data: &T) -> Result<String> {
        match self.format {
            OutputFormat::Toon => {
                // Convert to serde_json::Value first, then encode with toon
                let json_value = serde_json::to_value(data)?;
                let output = toon::encode(&json_value, None);
                Ok(output)
            }
            OutputFormat::Json => {
                let output = serde_json::to_string_pretty(data)?;
                Ok(output)
            }
            OutputFormat::Text => {
                // For now, text format is the same as TOON
                // Commands can override this behavior if they need plain text
                let json_value = serde_json::to_value(data)?;
                let output = toon::encode(&json_value, None);
                Ok(output)
            }
        }
    }

    /// Format and print data to stdout
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or writing fails
    pub fn print<T: Serialize>(&self, data: &T) -> Result<()> {
        let output = self.format(data)?;
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "{output}")?;
        Ok(())
    }

    /// Format and print a list with a custom empty message
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or writing fails
    pub fn print_list<T: Serialize>(&self, data: &[T], empty_message: &str) -> Result<()> {
        if data.is_empty() {
            let mut stdout = io::stdout().lock();
            match self.format {
                OutputFormat::Toon | OutputFormat::Text => writeln!(stdout, "{empty_message}")?,
                OutputFormat::Json => writeln!(stdout, "[]")?,
            }
            Ok(())
        } else {
            self.print(&data)
        }
    }
}

impl Default for Formatter {
    fn default() -> Self {
        Self::new(OutputFormat::default())
    }
}

/// Print data in TOON (human-readable) format to stdout
///
/// # Errors
///
/// Returns an error if serialization or writing fails
pub fn print_toon<T: Serialize>(data: &T) -> Result<()> {
    Formatter::new(OutputFormat::Toon).print(data)
}

/// Print data in JSON format to stdout
///
/// # Errors
///
/// Returns an error if serialization or writing fails
pub fn print_json<T: Serialize>(data: &T) -> Result<()> {
    Formatter::new(OutputFormat::Json).print(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Debug, Serialize)]
    struct TestData {
        name: String,
        count: u32,
        active: bool,
    }

    fn sample_data() -> TestData {
        TestData {
            name: "test-item".to_string(),
            count: 42,
            active: true,
        }
    }

    #[test]
    fn test_output_format_default() {
        let format = OutputFormat::default();
        assert_eq!(format, OutputFormat::Toon);
    }

    #[test]
    fn test_formatter_json_output() {
        let formatter = Formatter::new(OutputFormat::Json);
        let data = sample_data();
        let output = formatter.format(&data).expect("JSON formatting failed");

        // Verify it's valid JSON
        let parsed: serde_json::Value =
            serde_json::from_str(&output).expect("Output is not valid JSON");
        assert_eq!(parsed["name"], "test-item");
        assert_eq!(parsed["count"], 42);
        assert_eq!(parsed["active"], true);
    }

    #[test]
    fn test_formatter_toon_output() {
        let formatter = Formatter::new(OutputFormat::Toon);
        let data = sample_data();
        let output = formatter.format(&data).expect("TOON formatting failed");

        // TOON output should contain the field values in human-readable form
        assert!(output.contains("test-item") || output.contains("name"));
        assert!(output.contains("42") || output.contains("count"));
    }

    #[test]
    fn test_formatter_default() {
        let formatter = Formatter::default();
        assert_eq!(formatter.format, OutputFormat::Toon);
    }

    #[test]
    fn test_format_nested_structure() {
        #[derive(Debug, Serialize)]
        struct Nested {
            items: Vec<String>,
            metadata: Metadata,
        }

        #[derive(Debug, Serialize)]
        struct Metadata {
            version: u32,
        }

        let data = Nested {
            items: vec!["a".to_string(), "b".to_string()],
            metadata: Metadata { version: 1 },
        };

        let json_formatter = Formatter::new(OutputFormat::Json);
        let json_output = json_formatter.format(&data).expect("JSON failed");
        assert!(json_output.contains("items"));
        assert!(json_output.contains("metadata"));

        let toon_formatter = Formatter::new(OutputFormat::Toon);
        let toon_output = toon_formatter.format(&data).expect("TOON failed");
        // TOON should produce some output
        assert!(!toon_output.is_empty());
    }

    #[test]
    fn test_format_empty_vec() {
        let data: Vec<String> = vec![];

        let json_formatter = Formatter::new(OutputFormat::Json);
        let json_output = json_formatter.format(&data).expect("JSON failed");
        assert_eq!(json_output.trim(), "[]");

        let toon_formatter = Formatter::new(OutputFormat::Toon);
        let _toon_output = toon_formatter.format(&data).expect("TOON failed");
    }
}
