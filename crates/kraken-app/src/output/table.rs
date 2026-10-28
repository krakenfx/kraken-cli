use super::CommandOutput;
/// Human-readable table output renderer using comfy-table.
use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL_CONDENSED};

pub(crate) fn render(output: &CommandOutput) {
    if output.rows.is_empty() {
        println!("No results.");
    } else {
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

    // The footer prints even with no rows. `workspace report` on a workspace
    // with no sessions has an empty table but a footer holding its equity, its
    // caveats, and — on a live workspace — the real money standing behind it;
    // "No results." alone reads as "nothing here" about a funded account.
    if !output.footer.is_empty() {
        println!();
        for line in &output.footer {
            println!("{line}");
        }
    }
}

/// Format a single append-only line for WebSocket streaming in table mode.
/// Returns the rendered string rather than printing it, so the WebSocket sink
/// stays the sole owner of stdout (every line goes out through its writer
/// task).
pub(crate) fn stream_line(fields: &[(&str, &str)]) -> String {
    let parts: Vec<String> = fields.iter().map(|(k, v)| format!("{k}: {v}")).collect();
    parts.join("  |  ")
}
