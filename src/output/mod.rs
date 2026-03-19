/// Unified output rendering layer.
///
/// Every command returns a `CommandResult` which is rendered through this
/// module based on the chosen output format (table or json). Diagnostics
/// and verbose output always go to stderr, keeping stdout clean for
/// machine consumption in JSON mode.
pub(crate) mod json;
pub(crate) mod table;

use crate::errors::KrakenError;

/// Output format selection for the CLI.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    #[default]
    Table,
    Json,
}

/// A uniform result from any command, ready for rendering.
#[derive(Debug)]
pub(crate) struct CommandOutput {
    /// JSON value representing the successful payload.
    pub(crate) data: serde_json::Value,
    /// Column headers for table rendering (in display order).
    pub(crate) headers: Vec<String>,
    /// Row data for table rendering. Each row is a vec of cell strings.
    pub(crate) rows: Vec<Vec<String>>,
}

impl CommandOutput {
    /// Create output from a JSON value with explicit table structure.
    pub(crate) fn new(
        data: serde_json::Value,
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    ) -> Self {
        Self {
            data,
            headers,
            rows,
        }
    }

    /// Create a simple key-value output (rendered as two-column table).
    pub(crate) fn key_value(pairs: Vec<(String, String)>, json_data: serde_json::Value) -> Self {
        let headers = vec!["Field".to_string(), "Value".to_string()];
        let rows: Vec<Vec<String>> = pairs.into_iter().map(|(k, v)| vec![k, v]).collect();
        Self {
            data: json_data,
            headers,
            rows,
        }
    }

    /// Create output with a single message.
    pub(crate) fn message(msg: &str) -> Self {
        Self {
            data: serde_json::json!({ "message": msg }),
            headers: vec!["Message".to_string()],
            rows: vec![vec![msg.to_string()]],
        }
    }
}

/// Render a successful command result to the appropriate output stream.
pub(crate) fn render(format: OutputFormat, output: &CommandOutput) {
    match format {
        OutputFormat::Table => table::render(output),
        OutputFormat::Json => json::render_success(&output.data),
    }
}

/// Render an error to the appropriate output stream.
pub fn render_error(format: OutputFormat, err: &KrakenError) {
    match format {
        OutputFormat::Table => {
            eprintln!("Error: {err}");
        }
        OutputFormat::Json => json::render_error(err),
    }
}

/// Write a verbose diagnostic message to stderr (never contaminates stdout).
pub(crate) fn verbose(msg: &str) {
    eprintln!("[verbose] {msg}");
}

/// Write a warning to stderr.
pub fn warn(msg: &str) {
    eprintln!("Warning: {msg}");
}
