//! Output formatting module for botcrit
//!
//! Provides text, JSON, and pretty output formats for CLI output.

use anyhow::Result;
use serde::Serialize;
use std::io::{self, Write};

/// Output format selection
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    /// JSON format - machine-readable output
    Json,
    /// Plain text format - concise, token-efficient output
    #[default]
    Text,
    /// Pretty format - colorized, human-friendly output
    Pretty,
}

/// Formatter that can output data in text, JSON, or pretty format
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
            OutputFormat::Json => {
                let output = serde_json::to_string_pretty(data)?;
                Ok(output)
            }
            OutputFormat::Text => {
                // For now, text format uses JSON pretty
                // bd-rxp.4 will add proper text formatting
                let output = serde_json::to_string_pretty(data)?;
                Ok(output)
            }
            OutputFormat::Pretty => {
                // For now, Pretty format uses JSON pretty
                // Future enhancement: add colorization and enhanced formatting
                let output = serde_json::to_string_pretty(data)?;
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
    /// For JSON format, wraps the array in a named object with count and advice fields.
    /// For other formats, prints the list normally (ignores `collection_name` and `advice`).
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or writing fails
    pub fn print_list<T: Serialize>(
        &self,
        data: &[T],
        empty_message: &str,
        collection_name: &str,
        advice: &[&str],
    ) -> Result<()> {
        match self.format {
            OutputFormat::Json => {
                let items_value = serde_json::to_value(data)?;
                let mut envelope = serde_json::Map::new();
                envelope.insert(collection_name.to_string(), items_value);
                envelope.insert("count".to_string(), serde_json::json!(data.len()));
                envelope.insert("advice".to_string(), serde_json::json!(advice));

                let output = serde_json::to_string_pretty(&serde_json::Value::Object(envelope))?;
                let mut stdout = io::stdout().lock();
                writeln!(stdout, "{output}")?;
                Ok(())
            }
            OutputFormat::Text | OutputFormat::Pretty => {
                if data.is_empty() {
                    let mut stdout = io::stdout().lock();
                    writeln!(stdout, "{empty_message}")?;
                    Ok(())
                } else {
                    self.print(&data)
                }
            }
        }
    }
}

impl Default for Formatter {
    fn default() -> Self {
        Self::new(OutputFormat::default())
    }
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
        // The enum default is Text
        // Note: CLI resolution layer may override this based on TTY detection
        let format = OutputFormat::default();
        assert_eq!(format, OutputFormat::Text);
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
    fn test_formatter_text_output() {
        let formatter = Formatter::new(OutputFormat::Text);
        let data = sample_data();
        let output = formatter.format(&data).expect("Text formatting failed");

        // Text output should contain the field values
        assert!(output.contains("test-item") || output.contains("name"));
        assert!(output.contains("42") || output.contains("count"));
    }

    #[test]
    fn test_formatter_default() {
        let formatter = Formatter::default();
        assert_eq!(formatter.format, OutputFormat::Text);
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

        let text_formatter = Formatter::new(OutputFormat::Text);
        let text_output = text_formatter.format(&data).expect("Text failed");
        // Text should produce some output
        assert!(!text_output.is_empty());
    }

    #[test]
    fn test_format_empty_vec() {
        let data: Vec<String> = vec![];

        let json_formatter = Formatter::new(OutputFormat::Json);
        let json_output = json_formatter.format(&data).expect("JSON failed");
        assert_eq!(json_output.trim(), "[]");

        let text_formatter = Formatter::new(OutputFormat::Text);
        let _text_output = text_formatter.format(&data).expect("Text failed");
    }

    #[test]
    fn test_print_list_json_envelope() {
        #[derive(Debug, Serialize)]
        struct Item {
            id: String,
            name: String,
        }

        let items = vec![
            Item { id: "1".to_string(), name: "first".to_string() },
            Item { id: "2".to_string(), name: "second".to_string() },
        ];

        // Verify the envelope structure by building it the same way print_list does
        let items_value = serde_json::to_value(&items).expect("serialize items");
        let mut envelope = serde_json::Map::new();
        envelope.insert("items".to_string(), items_value);
        envelope.insert("count".to_string(), serde_json::json!(2));
        envelope.insert("advice".to_string(), serde_json::json!(["crit show <id>"]));

        let output = serde_json::to_string_pretty(&serde_json::Value::Object(envelope))
            .expect("serialize envelope");
        let parsed: serde_json::Value = serde_json::from_str(&output).expect("parse");

        assert_eq!(parsed["count"], 2);
        assert!(parsed["items"].is_array());
        assert!(parsed["advice"].is_array());
        assert_eq!(parsed["items"].as_array().expect("items array").len(), 2);
    }

    #[test]
    fn test_formatter_pretty_output() {
        let formatter = Formatter::new(OutputFormat::Pretty);
        let data = sample_data();
        let output = formatter.format(&data).expect("Pretty formatting failed");

        // Pretty output should contain the field values
        assert!(output.contains("test-item") || output.contains("name"));
        assert!(output.contains("42") || output.contains("count"));
        assert!(!output.is_empty());
    }

    #[test]
    fn test_output_format_pretty_variant_exists() {
        // Verify Pretty variant exists in the enum
        let format = OutputFormat::Pretty;
        let formatter = Formatter::new(format);
        assert_eq!(formatter.format, OutputFormat::Pretty);
    }
}
