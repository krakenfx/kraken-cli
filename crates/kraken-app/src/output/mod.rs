//! Unified output rendering.
//!
//! Every command produces a `CommandOutput` rendered here per the chosen
//! output format (table or json). Diagnostics go to stderr via `tracing` (see
//! [`crate::logging`]) and never touch stdout, keeping it clean for machine
//! consumption in JSON mode.

pub(crate) mod json;
pub(crate) mod summary;
pub(crate) mod table;

use crate::errors::KrakenError;
use kraken_workspace::receipt::TradeMode;
use rust_decimal::{Decimal, RoundingStrategy};

/// A `Decimal` ready for `{:.dp}` display: rust_decimal's `Display`
/// *truncates* extra digits where f64 rounded, which would render a
/// half-cent loss as `-0.00` and a 0.126% fee as `0.12%`. Half-way cases
/// round away from zero, matching the old float renders.
pub(crate) fn rounded(value: Decimal, dp: u32) -> Decimal {
    value.round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero)
}

/// A signed figure at `dp` resolution that keeps its sign when it is too small
/// to show (`-<0.01`), so `+0.00` means exactly zero.
///
/// `Decimal` has no signed zero, so a rounded tiny loss renders as `+0.00`. A
/// P&L cell pairs an amount with a percentage, and at different magnitudes only
/// one of them collapses — which is how `+0.00 BTC (-0.21%)` and
/// `-0.27 (+0.00%)` reached users, each half contradicting the other. Mirrors
/// the convention `lab.rs` already applies to its Return cell.
pub(crate) fn signed_or_sub_resolution(value: Decimal, dp: u32) -> String {
    let shown = rounded(value, dp);
    if shown.is_zero() && !value.is_zero() {
        let sign = if value.is_sign_negative() { '-' } else { '+' };
        let smallest = Decimal::ONE / Decimal::from(10u64.pow(dp));
        return format!("{sign}<{smallest}");
    }
    format!("{shown:+.*}", dp as usize)
}

/// What a live workspace's money is, on every output it stamps. Says "REAL"
/// because this label is the one that survives into a footer, where it may be
/// the only warning a reader sees.
pub(crate) const LIVE_SCOPE_LABEL: &str = "REAL orders — allocation of the real account";

/// What a pre-promotion session's money was, on outputs read back after the
/// workspace went live. States "simulated" so paper history can never be
/// mistaken for real-money evidence.
pub(crate) const PAPER_ERA_LABEL: &str = "paper era — simulated fills, recorded before promotion";

/// Rows that describe the whole output rather than one of its fields. They hold
/// the top of a key/value table, and a new one lands after the others.
const CONTEXT_LABELS: [&str; 5] = ["Mode", "Workspace", "Session", "Scope", "Reconciled"];

/// What replaces the simulator's fee and slippage rates on a live ledger, whose
/// fills carry the venue's own fee. Printing the contract's rates there would
/// read as a promise about real executions.
pub(crate) const LIVE_FEES_NOTE: &str = "Kraken's own, charged per fill";

/// The marker every table leads its first header with, and the `Mode` row of a
/// key/value table.
///
/// One pair of functions for the whole CLI: paper and live share the renderers,
/// and the mode marker is the only thing that may differ between them. Deriving
/// both from [`TradeMode`] is what stops a real order printing under `[PAPER]`.
pub(crate) const fn mode_marker(mode: TradeMode) -> &'static str {
    match mode {
        TradeMode::Paper => "[PAPER]",
        TradeMode::Live => "[LIVE]",
    }
}

/// Says the money is real without saying whose slice it is: the same label
/// serves an order on the whole account and one inside a live workspace. Which
/// slice is the `Scope` row's job, added only when a workspace is in play — so
/// this must not claim an allocation that may not exist.
pub(crate) const fn mode_label(mode: TradeMode) -> &'static str {
    match mode {
        TradeMode::Paper => "[PAPER] Simulated Trading",
        TradeMode::Live => "[LIVE] Real Money",
    }
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
pub struct CommandOutput {
    pub data: serde_json::Value,
    pub(crate) headers: Vec<String>,
    pub(crate) rows: Vec<Vec<String>>,
    /// Lines rendered below the table in table mode. For whole-output notes
    /// that would misread as a column's own value if placed in a table cell;
    /// JSON carries them as typed fields, so this stays table-only.
    pub(crate) footer: Vec<String>,
    /// The command already wrote its output; [`render`] prints nothing.
    inline: bool,
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
            inline: false,
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
            inline: false,
        }
    }

    pub(crate) fn message(msg: &str) -> Self {
        Self {
            data: serde_json::json!({ "message": msg }),
            headers: vec!["Message".to_string()],
            rows: vec![vec![msg.to_string()]],
            footer: Vec::new(),
            inline: false,
        }
    }

    /// A streaming/interactive command's receipt: output already emitted
    /// inline, with no structured result to return.
    pub(crate) fn inline() -> Self {
        Self {
            data: serde_json::json!({}),
            headers: Vec::new(),
            rows: Vec::new(),
            footer: Vec::new(),
            inline: true,
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
        self.stamp_context("Session", name);
    }

    /// Stamp the workspace that scoped this output — same placement rules as
    /// [`Self::stamp_session`], with a `workspace` JSON field.
    pub(crate) fn stamp_workspace(&mut self, name: &str) {
        if let Some(obj) = self.data.as_object_mut() {
            obj.insert("workspace".into(), serde_json::json!(name));
        }
        self.stamp_context("Workspace", name);
    }

    fn stamp_context(&mut self, label: &str, value: &str) {
        self.stamp_context_rows(&[(label, value)]);
    }

    /// Carry one whole-output fact, for a caller outside this module.
    pub(crate) fn stamp_context_row(&mut self, label: &str, value: &str) {
        self.stamp_context_rows(&[(label, value)]);
    }

    /// Carry `label: value` facts about the whole output, in order.
    ///
    /// A key/value table takes them as rows, after `Mode` when one leads. A
    /// wide table (balances, open orders, trades) has real records in those
    /// columns, so a label row there reads as a bogus record — those get
    /// footer lines. Single-column outputs carry the facts in JSON only. A
    /// label already present is left alone, so stamping twice cannot double
    /// a row.
    fn stamp_context_rows(&mut self, facts: &[(&str, &str)]) {
        match self.headers.len() {
            0 | 1 => {}
            2 => {
                // After the leading run of context rows, so `Mode`/`Workspace`
                // keep the top of the table and repeated stamps land in call
                // order instead of reversing each other.
                let mut at = self
                    .rows
                    .iter()
                    .take_while(|row| {
                        row.first()
                            .is_some_and(|cell| CONTEXT_LABELS.contains(&cell.as_str()))
                    })
                    .count();
                for &(label, value) in facts {
                    if self
                        .rows
                        .iter()
                        .any(|row| row.first().is_some_and(|cell| cell == label))
                    {
                        continue;
                    }
                    self.rows
                        .insert(at, vec![label.to_string(), value.to_string()]);
                    at += 1;
                }
            }
            _ => self
                .footer
                .extend(facts.iter().map(|(l, v)| format!("{l}: {v}"))),
        }
    }

    /// Carry summary facts at the END of a key/value table: identity rows
    /// lead, summaries close. A wide table takes them as footer lines, same
    /// as context rows; single-column outputs carry them in JSON only.
    fn stamp_summary_rows(&mut self, facts: &[(&str, String)]) {
        match self.headers.len() {
            0 | 1 => {}
            2 => {
                for (label, value) in facts {
                    if self
                        .rows
                        .iter()
                        .any(|row| row.first().is_some_and(|cell| cell == label))
                    {
                        continue;
                    }
                    self.rows.push(vec![label.to_string(), value.clone()]);
                }
            }
            _ => self
                .footer
                .extend(facts.iter().map(|(l, v)| format!("{l}: {v}"))),
        }
    }

    /// Name the live workspace and say its money is real, without asking the
    /// venue anything.
    ///
    /// For reads that do not reconcile — session metadata, for one. Those still
    /// have to disclose that they describe real money, and a session's own
    /// `Market data: live` says nothing about that.
    pub(crate) fn stamp_live_scope(&mut self, workspace: &str) {
        if let Some(obj) = self.data.as_object_mut() {
            obj.insert("workspace".into(), serde_json::json!(workspace));
            obj.insert("mode".into(), serde_json::json!("live"));
            obj.insert("scope".into(), serde_json::json!("allocation"));
        }
        self.stamp_context_rows(&[("Workspace", workspace), ("Scope", LIVE_SCOPE_LABEL)]);
    }

    /// Name the sessions in a multi-session output whose fills were simulated
    /// (recorded before promotion), in both renders: a `paper_era` JSON field
    /// for machine consumers, a context row or footer line for humans.
    pub(crate) fn stamp_paper_era_sessions(&mut self, sessions: &[String]) {
        if sessions.is_empty() {
            return;
        }
        if let Some(obj) = self.data.as_object_mut() {
            obj.insert("paper_era".into(), serde_json::json!(sessions));
        }
        let value = format!(
            "{} — simulated fills, recorded before promotion",
            sessions.join(", ")
        );
        self.stamp_context_rows(&[("Paper era", value.as_str())]);
    }

    /// Name the live workspace but say this session's money was simulated:
    /// its window sealed before promotion, so [`Self::stamp_live_scope`]
    /// would inflate paper history into real-money evidence.
    pub(crate) fn stamp_paper_era_scope(&mut self, workspace: &str) {
        if let Some(obj) = self.data.as_object_mut() {
            obj.insert("workspace".into(), serde_json::json!(workspace));
            obj.insert("mode".into(), serde_json::json!("paper"));
            obj.insert("scope".into(), serde_json::json!("paper_era"));
        }
        self.stamp_context_rows(&[("Workspace", workspace), ("Scope", PAPER_ERA_LABEL)]);
    }

    /// Stamp a live workspace: its name, that its ledger is an allocation
    /// rather than the whole account, whether the venue confirmed that ledger
    /// on this read, and what actually stands behind the allocation.
    ///
    /// This replaces the older `isolation: "virtual"` label. In a CLI whose
    /// other mode is a simulator, "virtual" read as "not real"; a balance the
    /// caller can check is both honest and unambiguous.
    pub(crate) fn stamp_live_workspace(
        &mut self,
        name: &str,
        backing: &kraken_workspace::backing::Backing,
        reconciled: bool,
        spendable: rust_decimal::Decimal,
    ) {
        if let Some(obj) = self.data.as_object_mut() {
            obj.insert("workspace".into(), serde_json::json!(name));
            obj.insert("mode".into(), serde_json::json!("live"));
            obj.insert("scope".into(), serde_json::json!("allocation"));
            obj.insert("reconciled".into(), serde_json::json!(reconciled));
            if let Ok(mut backing) = serde_json::to_value(backing) {
                // Rides inside `backing`: it is the same story's ledger
                // dimension — what this workspace can still spend now.
                if let Some(backing) = backing.as_object_mut() {
                    backing.insert("spendable".into(), serde_json::json!(spendable.to_string()));
                }
                obj.insert("backing".into(), backing);
            }
        }
        let mut facts = vec![("Workspace", name), ("Scope", LIVE_SCOPE_LABEL)];
        if !reconciled {
            facts.push(("Reconciled", "no — venue unconfirmed"));
        }
        self.stamp_context_rows(&facts);
        // The backing facts close the table as rows rather than riding a prose
        // footer — one fact per row for humans, the same tokens the JSON
        // `backing` object carries. "unread" is stated, never implied by a
        // missing row.
        let asset = &backing.asset;
        let mut summary: Vec<(&str, String)> = vec![("Backing", backing.status.to_string())];
        summary.push((
            "Account",
            backing.account.map_or_else(
                || "unread".to_string(),
                |held| format!("{} {asset}", held.normalize()),
            ),
        ));
        if backing.allocated_elsewhere > rust_decimal::Decimal::ZERO {
            summary.push((
                "Allocated elsewhere",
                format!("{} {asset}", backing.allocated_elsewhere.normalize()),
            ));
        }
        if let Some(unallocated) = backing.unallocated {
            summary.push((
                "Unallocated",
                format!("{} {asset}", unallocated.normalize()),
            ));
        }
        summary.push(("Spendable", format!("{spendable} {asset}")));
        self.stamp_summary_rows(&summary);
        if !reconciled {
            self.footer.push(
                "The venue did not confirm every recorded order, so fills may be missing."
                    .to_string(),
            );
        }
    }

    /// A typed result whose table is its published JSON as key-value rows.
    pub(crate) fn published(value: &impl serde::Serialize) -> crate::errors::Result<Self> {
        published_json(value).map(Self::from_untyped)
    }

    /// Render a JSON payload with no bespoke table as key-value rows: objects
    /// by field, arrays by index, anything else as a single `Result` row.
    pub(crate) fn from_untyped(data: serde_json::Value) -> Self {
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
        Self::key_value(pairs, data)
    }
}

/// A typed result as its published JSON payload. Tables render from this same
/// value, so the two output formats cannot disagree.
pub(crate) fn published_json(
    value: &impl serde::Serialize,
) -> crate::errors::Result<serde_json::Value> {
    Ok(serde_json::to_value(value)?)
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
    // An inline receipt is not a result: in JSON mode its `{}` would be a
    // second value after the command's own output.
    if output.inline {
        return;
    }
    match format {
        OutputFormat::Table => table::render(output),
        OutputFormat::Json => json::render_success(&output.data),
    }
}

pub fn render_error(format: OutputFormat, err: &KrakenError) {
    match format {
        OutputFormat::Table => {
            eprintln!("Error: {}", terminal_safe(&err.to_string()));
            for line in err.detail_lines() {
                eprintln!("{}", terminal_safe(&line));
            }
        }
        OutputFormat::Json => json::render_error(err),
    }
}

/// An error can quote a server reply, so a control or bidi character in it is
/// printed escaped, never interpreted by the terminal. JSON escapes its own.
fn terminal_safe(text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    for c in text.chars() {
        let bidi = matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
        if (c.is_control() && c != '\n') || bidi {
            safe.extend(c.escape_unicode());
        } else {
            safe.push(c);
        }
    }
    safe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_never_writes_a_terminal_escape_or_a_bidi_control() {
        let safe = terminal_safe("Kraken refused the sign-in: \u{1b}]0;pwned\u{7}\u{202e}gpj");
        assert!(
            !safe.contains('\u{1b}') && !safe.contains('\u{202e}'),
            "{safe}"
        );
        assert!(safe.contains("\\u{1b}"), "{safe}");
    }
    use rust_decimal_macros::dec;
    use serde_json::json;

    fn measured_backing() -> kraken_workspace::backing::Backing {
        kraken_workspace::backing::Backing::evaluate(
            "USD",
            dec!(5),
            kraken_workspace::backing::Claims::known(dec!(0)),
            Some(dec!(9.7534)),
        )
    }

    /// The backing facts are table rows, not a prose footer: one fact per
    /// row, closing the table after the command's own data.
    #[test]
    fn the_live_stamp_closes_a_key_value_table_with_backing_rows() {
        let mut out =
            CommandOutput::key_value(vec![("Allocation".into(), "5 USD".into())], json!({}));
        out.stamp_live_workspace("prod", &measured_backing(), true, dec!(3.46967));
        let labels: Vec<&str> = out.rows.iter().map(|r| r[0].as_str()).collect();
        assert_eq!(
            labels,
            [
                "Workspace",
                "Scope",
                "Allocation",
                "Backing",
                "Account",
                "Unallocated",
                "Spendable"
            ],
            "identity leads, the command's data holds the middle, backing closes"
        );
        let row = |label: &str| {
            out.rows
                .iter()
                .find(|r| r[0] == label)
                .map(|r| r[1].clone())
                .unwrap_or_default()
        };
        assert_eq!(row("Backing"), "backed");
        assert_eq!(row("Account"), "9.7534 USD");
        assert_eq!(row("Unallocated"), "4.7534 USD");
        assert_eq!(row("Spendable"), "3.46967 USD");
        assert!(
            out.footer.iter().all(|line| !line.contains("allocation")),
            "no prose summary footer remains: {:?}",
            out.footer
        );
        assert_eq!(out.data["backing"]["spendable"], "3.46967");
    }

    /// A wide table has real records in its columns, so the same facts arrive
    /// as labeled footer lines instead of bogus rows.
    #[test]
    fn the_live_stamp_footers_a_wide_table_with_labeled_lines() {
        let mut out = CommandOutput::new(
            json!({}),
            vec!["ID".into(), "Pair".into(), "Volume".into()],
            vec![],
        );
        out.stamp_live_workspace("prod", &measured_backing(), true, dec!(3.46967));
        let footer = out.footer.join("\n");
        assert!(footer.contains("Backing: backed"), "{footer}");
        assert!(footer.contains("Spendable: 3.46967 USD"), "{footer}");
    }

    /// An unread account balance is a stated fact, not a missing row.
    #[test]
    fn an_unread_account_shows_as_unread_not_as_absence() {
        let backing = kraken_workspace::backing::Backing::evaluate(
            "USD",
            dec!(5),
            kraken_workspace::backing::Claims::known(dec!(0)),
            None,
        );
        let mut out = CommandOutput::key_value(vec![], json!({}));
        out.stamp_live_workspace("prod", &backing, true, dec!(3.5));
        let row = |label: &str| {
            out.rows
                .iter()
                .find(|r| r[0] == label)
                .map(|r| r[1].clone())
                .unwrap_or_default()
        };
        assert_eq!(row("Backing"), "unverified");
        assert_eq!(row("Account"), "unread");
        assert!(
            !out.rows.iter().any(|r| r[0] == "Unallocated"),
            "no headroom may be implied from an unread balance"
        );
    }

    /// The two halves of a P&L cell must never disagree about direction. Both
    /// of these reached users: a tiny BTC loss as `+0.00 BTC (-0.21%)`, and a
    /// small USD loss as `-0.27 (+0.00%)`.
    #[test]
    fn a_sub_resolution_loss_keeps_its_sign() {
        assert_eq!(signed_or_sub_resolution(dec!(-0.00208), 2), "-<0.01");
        assert_eq!(signed_or_sub_resolution(dec!(-0.0021), 2), "-<0.01");
        assert_eq!(signed_or_sub_resolution(dec!(0.0021), 2), "+<0.01");
    }

    /// `+0.00` has to keep meaning exactly zero, or the flag above says
    /// nothing.
    #[test]
    fn an_exact_zero_still_renders_as_zero() {
        assert_eq!(signed_or_sub_resolution(Decimal::ZERO, 2), "+0.00");
        assert_eq!(signed_or_sub_resolution(dec!(-0.27), 2), "-0.27");
        assert_eq!(signed_or_sub_resolution(dec!(1.005), 2), "+1.01");
    }

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

    /// A wide table's columns hold real records, so context belongs in the
    /// footer. As a dash-filled row it rendered as a bogus record — in
    /// `balance` it read as an asset named "Workspace" whose available balance
    /// was the workspace name.
    #[test]
    fn stamp_run_keeps_wide_tables_free_of_label_rows() {
        let mut out = CommandOutput::new(
            json!({"components": []}),
            vec!["Line".into(), "Amount".into(), "Explanation".into()],
            vec![vec!["Total".into(), "-16.83".into(), "ended at…".into()]],
        );
        out.stamp_session("board-demo");
        assert_eq!(
            out.rows,
            vec![vec!["Total", "-16.83", "ended at…"]],
            "the records are untouched"
        );
        assert!(out.footer.iter().any(|line| line == "Session: board-demo"));
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
