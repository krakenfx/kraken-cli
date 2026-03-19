/// Human-readable table output renderer using comfy-table.
use comfy_table::{presets::UTF8_FULL_CONDENSED, ContentArrangement, Table};

use super::CommandOutput;

/// Render a `CommandOutput` as a human-readable table to stdout.
pub(crate) fn render(output: &CommandOutput) {
    if output.rows.is_empty() {
        println!("No results.");
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(&output.headers);

    for row in &output.rows {
        table.add_row(row);
    }

    println!("{table}");
}

/// Render a single append-only line for WebSocket streaming in table mode.
pub(crate) fn render_stream_line(fields: &[(&str, &str)]) {
    let parts: Vec<String> = fields.iter().map(|(k, v)| format!("{k}: {v}")).collect();
    println!("{}", parts.join("  |  "));
}
