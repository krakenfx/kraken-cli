//! Unified output rendering.
//!
//! Every command produces a `CommandOutput` rendered here per the chosen
//! output format (table or json). Diagnostics go to stderr via `tracing` (see
//! [`crate::logging`]) and never touch stdout, keeping it clean for machine
//! consumption in JSON mode.

pub(crate) mod json;
pub(crate) mod summary;
pub(crate) mod table;

use rust_decimal::{Decimal, RoundingStrategy};

use crate::errors::KrakenError;

/// A `Decimal` ready for `{:.dp}` display: rust_decimal's `Display`
/// *truncates* extra digits where f64 rounded, which would render a
/// half-cent loss as `-0.00` and a 0.126% fee as `0.12%`. Half-way cases
/// round away from zero, matching the old float renders.
pub(crate) fn rounded(value: Decimal, dp: u32) -> Decimal {
    value.round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    #[default]
    Table,
    Json,
}

/// A command's result carrying both its JSON payload and its table projection,
/// so either output format renders without further shaping.
#[derive(Debug)]
pub(crate) struct CommandOutput {
    pub(crate) data: serde_json::Value,
    pub(crate) headers: Vec<String>,
    pub(crate) rows: Vec<Vec<String>>,
    /// Lines rendered below the table in table mode. For whole-output notes
    /// that would misread as a column's own value if placed in a table cell;
    /// JSON carries them as typed fields, so this stays table-only.
    pub(crate) footer: Vec<String>,
}

impl CommandOutput {
    pub(crate) fn new(
        data: serde_json::Value,
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    ) -> Self {
        Self {
            data,
            headers,
            rows,
            footer: Vec::new(),
        }
    }

    /// Attach footer lines rendered below the table (table mode only).
    pub(crate) fn with_footer(mut self, footer: Vec<String>) -> Self {
        self.footer = footer;
        self
    }

    pub(crate) fn key_value(pairs: Vec<(String, String)>, json_data: serde_json::Value) -> Self {
        let headers = vec!["Field".to_string(), "Value".to_string()];
        let rows: Vec<Vec<String>> = pairs.into_iter().map(|(k, v)| vec![k, v]).collect();
        Self {
            data: json_data,
            headers,
            rows,
            footer: Vec::new(),
        }
    }

    pub(crate) fn message(msg: &str) -> Self {
        Self {
            data: serde_json::json!({ "message": msg }),
            headers: vec!["Message".to_string()],
            rows: vec![vec![msg.to_string()]],
            footer: Vec::new(),
        }
    }

    /// Stamp the session that scoped this output: the JSON payload gains a
    /// `run` field, and the table a `Run` row — after the `Mode` row
    /// when one leads (paper outputs), else first — with the name in the last
    /// column and `—` filling any cells between. Single-column outputs carry
    /// it in JSON only.
    pub(crate) fn stamp_session(&mut self, name: &str) {
        if let Some(obj) = self.data.as_object_mut() {
            obj.insert("session".into(), serde_json::json!(name));
        }
        let width = self.headers.len();
        if width < 2 {
            return;
        }
        let mut row = vec!["Session".to_string()];
        row.extend(vec!["—".to_string(); width - 2]);
        row.push(name.to_string());
        let after_mode = usize::from(
            self.rows
                .first()
                .is_some_and(|first| first.first().is_some_and(|cell| cell == "Mode")),
        );
        self.rows.insert(after_mode, row);
    }

    /// Stamp the workspace that scoped this output — same placement rules as
    /// [`Self::stamp_session`], with a `workspace` JSON field and a
    /// `Workspace` table row.
    pub(crate) fn stamp_workspace(&mut self, name: &str) {
        if let Some(obj) = self.data.as_object_mut() {
            obj.insert("workspace".into(), serde_json::json!(name));
        }
        let width = self.headers.len();
        if width < 2 {
            return;
        }
        let mut row = vec!["Workspace".to_string()];
        row.extend(vec!["—".to_string(); width - 2]);
        row.push(name.to_string());
        let after_mode = usize::from(
            self.rows
                .first()
                .is_some_and(|first| first.first().is_some_and(|cell| cell == "Mode")),
        );
        self.rows.insert(after_mode, row);
    }

    /// Render a payload we don't type as key-value rows: objects by field,
    /// arrays by index, anything else as a single `Result` row.
    pub(crate) fn from_untyped(data: &serde_json::Value) -> Self {
        let pairs: Vec<(String, String)> = if let Some(obj) = data.as_object() {
            obj.iter()
                .map(|(k, v)| {
                    let val = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (k.clone(), val)
                })
                .collect()
        } else if let Some(arr) = data.as_array() {
            arr.iter()
                .enumerate()
                .map(|(i, v)| (format!("[{i}]"), v.to_string()))
                .collect()
        } else {
            vec![("Result".into(), data.to_string())]
        };
        Self::key_value(pairs, data.clone())
    }
}

/// A JSON field as display text, `"-"` when absent — table cells never
/// render `null` or panic on schema drift.
pub(crate) fn json_field(val: &serde_json::Value, key: &str) -> String {
    val.get(key)
        .map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| "-".to_string())
}

pub(crate) fn render(format: OutputFormat, output: &CommandOutput) {
    match format {
        OutputFormat::Table => table::render(output),
        OutputFormat::Json => json::render_success(&output.data),
    }
}

pub fn render_error(format: OutputFormat, err: &KrakenError) {
    match format {
        OutputFormat::Table => {
            eprintln!("Error: {err}");
        }
        OutputFormat::Json => json::render_error(err),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn stamp_run_slots_after_a_leading_mode_row() {
        let mut out = CommandOutput::key_value(
            vec![
                ("Mode".into(), "[PAPER] Simulated Trading".into()),
                ("Action".into(), "Order cancelled".into()),
            ],
            json!({"action": "order_cancelled"}),
        );
        out.stamp_session("board-demo");
        assert_eq!(out.rows[1], vec!["Session", "board-demo"]);
        assert_eq!(
            out.data.get("session").and_then(|v| v.as_str()),
            Some("board-demo")
        );
    }

    #[test]
    fn stamp_run_leads_wide_tables_with_dash_filled_cells() {
        let mut out = CommandOutput::new(
            json!({"components": []}),
            vec!["Line".into(), "Amount".into(), "Explanation".into()],
            vec![vec!["Total".into(), "-16.83".into(), "ended at…".into()]],
        );
        out.stamp_session("board-demo");
        assert_eq!(out.rows[0], vec!["Session", "—", "board-demo"]);
        assert_eq!(
            out.data.get("session").and_then(|v| v.as_str()),
            Some("board-demo")
        );
    }

    #[test]
    fn stamp_run_keeps_single_column_output_json_only() {
        let mut out = CommandOutput::message("done");
        out.stamp_session("board-demo");
        assert!(out.rows.iter().all(|row| row[0] != "Session"));
        assert_eq!(
            out.data.get("session").and_then(|v| v.as_str()),
            Some("board-demo")
        );
    }
}
