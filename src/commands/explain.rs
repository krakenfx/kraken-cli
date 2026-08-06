//! CLI and MCP adapter for [`kraken_replay::explain_pnl`].

use clap::Subcommand;
use enum_dispatch::enum_dispatch;
use kraken_paper::account::Origin;
use kraken_replay::{
    ComponentKind, ComponentLine, MissedFill, OriginSlice, PnlReport, TradeLine, WindowSummary,
};
use rust_decimal::Decimal;

use super::Execute;
use crate::cli::AppContext;
use crate::errors::Result;
use crate::output::{CommandOutput, rounded};

#[enum_dispatch(Execute)]
#[derive(Debug, Subcommand)]
pub(crate) enum ExplainCommand {
    /// Decompose a stopped session's P&L into plain-terms components.
    Pnl(Pnl),
}

#[derive(Debug, clap::Args)]
pub(crate) struct Pnl {
    /// Which session: `latest`, an ordinal (`s3`), or a label.
    #[arg(long, default_value = "latest")]
    session: String,
}

impl Execute for Pnl {
    async fn execute(self, ctx: &AppContext) -> Result<CommandOutput> {
        super::session::ensure_session_stopped(ctx, &self.session)?;
        let (session_id, window) = super::session::read_session_window(ctx, &self.session)?;
        let report = kraken_replay::explain_pnl(&window)?;
        let mut out = output(&report)?;
        out.stamp_session(&session_id.to_string());
        if let Some(workspace) = ctx.workspace.as_deref() {
            out.stamp_workspace(workspace);
        }
        Ok(out)
    }
}

fn output(report: &PnlReport) -> Result<CommandOutput> {
    let headers = ["Line", "Amount", "Explanation"].map(String::from).to_vec();
    let mut rows = vec![total_row(report)];
    rows.extend(report.components.iter().map(component_row));
    rows.extend(trade_rows(report));
    rows.extend(report.window.iter().flat_map(window_rows));
    rows.extend(report.missed_fills.orders.iter().map(missed_row));
    rows.extend(report.attribution.iter().map(attribution_row));
    rows.extend(report.caveats.iter().map(|caveat| caveat_row(caveat)));
    Ok(CommandOutput::new(
        serde_json::to_value(report)?,
        headers,
        rows,
    ))
}

fn total_row(report: &PnlReport) -> Vec<String> {
    vec![
        "Total".to_string(),
        signed(report.anchor.total_pnl),
        format!(
            "ended at {:.2} {} from a {:.2} start (as reported at session stop)",
            rounded(report.anchor.final_value, 2),
            report.anchor.currency,
            rounded(report.anchor.starting_balance, 2)
        ),
    ]
}

fn component_row(component: &ComponentLine) -> Vec<String> {
    vec![
        component_label(component.kind).to_string(),
        component.amount.map_or_else(|| "?".to_string(), signed),
        component.explanation.clone(),
    ]
}

/// One row per fill; a session without fills gets the cash story instead
/// (the market's side of it rides in the window rows).
fn trade_rows(report: &PnlReport) -> Vec<Vec<String>> {
    if report.trades.is_empty() {
        let where_it_sat = if report.window.is_some() {
            "over the recorded window"
        } else {
            "and no market was recorded"
        };
        return vec![vec![
            "No trades".to_string(),
            "—".to_string(),
            format!(
                "your {:.2} {} sat in cash {where_it_sat}",
                rounded(report.anchor.starting_balance, 2),
                report.anchor.currency
            ),
        ]];
    }
    report.trades.iter().map(trade_row).collect()
}

fn trade_row(line: &TradeLine) -> Vec<String> {
    // The row's amount is the fill's whole recorded-mark contribution;
    // unknown movement (no end mark) makes the total unknowable too.
    let contribution = line
        .price_movement
        .map(|movement| movement + line.fees + line.spread + line.slippage);
    let reason = line
        .reason
        .as_deref()
        .map(|reason| format!(" — {reason}"))
        .unwrap_or_default();
    vec![
        "Trade".to_string(),
        contribution.map_or_else(|| "?".to_string(), signed),
        format!(
            "{}: {} {} {} @ {:.2} ({}, {}){reason}",
            line.trade_id,
            line.side,
            qty(line.volume),
            line.pair,
            rounded(line.price, 2),
            line.order_type,
            origin_label(line.origin),
        ),
    ]
}

fn window_rows(window: &WindowSummary) -> Vec<Vec<String>> {
    window
        .symbols
        .iter()
        .map(|symbol| {
            let movement = symbol.move_pct.map_or_else(
                || "had no mid data".to_string(),
                |pct| format!("moved {:+.1}%", rounded(pct, 1)),
            );
            let spread = symbol
                .avg_spread
                .map(|avg| format!(", avg spread {:.2}", rounded(avg, 2)))
                .unwrap_or_default();
            vec![
                "Window".to_string(),
                "—".to_string(),
                format!(
                    "{} {movement}{spread} over the recorded window ({} frames)",
                    symbol.symbol, symbol.frames
                ),
            ]
        })
        .collect()
}

fn missed_row(missed: &MissedFill) -> Vec<String> {
    vec![
        "Missed fill".to_string(),
        missed
            .hypothetical_pnl
            .map_or_else(|| "—".to_string(), |pnl| format!("~{}", signed(pnl))),
        missed_story(missed),
    ]
}

fn attribution_row(slice: &OriginSlice) -> Vec<String> {
    vec![
        "Attribution".to_string(),
        "—".to_string(),
        format!(
            "{}: {} fill(s), {:.2} notional, fees {}, {} with a logged reason",
            origin_label(slice.origin),
            slice.trades,
            rounded(slice.gross_notional, 2),
            signed(slice.fees),
            slice.decided
        ),
    ]
}

fn caveat_row(caveat: &str) -> Vec<String> {
    vec!["Caveat".to_string(), "—".to_string(), caveat.to_string()]
}

/// Signed money for the Amount column. A rounded zero clamps to +0.00 so a
/// break-even line never reads as a loss.
fn signed(amount: Decimal) -> String {
    let cents = rounded(amount, 2);
    let amount = if cents.is_zero() {
        Decimal::ZERO
    } else {
        cents
    };
    format!("{amount:+.2}")
}

/// A quantity without decimal-tail noise: at most 8 decimals, trimmed.
fn qty(volume: Decimal) -> String {
    format!("{:.8}", rounded(volume, 8))
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn component_label(kind: ComponentKind) -> &'static str {
    match kind {
        ComponentKind::PriceMovement => "Price movement",
        ComponentKind::Fees => "Fees",
        ComponentKind::Spread => "Spread",
        ComponentKind::Slippage => "Slippage",
        ComponentKind::Residual => "Residual",
    }
}

fn missed_story(missed: &MissedFill) -> String {
    let order = format!(
        "{} ({} {} {} @ {:.2}, {})",
        missed.order_id,
        missed.side,
        qty(missed.volume),
        missed.pair,
        rounded(missed.limit_price, 2),
        missed.outcome
    );
    match missed.crossed {
        None => {
            let reason = missed
                .uncovered_reason
                .as_deref()
                .unwrap_or("coverage unknown");
            format!("{order}: the recording cannot answer whether it would have filled — {reason}")
        }
        Some(false) => format!("{order}: the recorded stream never touched the limit price"),
        Some(true) => format!(
            "{order}: the recorded stream first crossed the limit at {}; not part of the sum above",
            missed
                .first_crossed_at
                .map(kraken_recording::format_instant)
                .unwrap_or_else(|| "?".to_string())
        ),
    }
}

fn origin_label(origin: Origin) -> &'static str {
    match origin {
        Origin::Cli => "you (CLI)",
        Origin::Mcp => "an agent (MCP)",
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn signed_clamps_subcent_noise_to_break_even() {
        assert_eq!(signed(-dec!(0.0049)), "+0.00");
        assert_eq!(signed(-dec!(0.0)), "+0.00");
        assert_eq!(signed(-dec!(0.005)), "-0.01");
        assert_eq!(signed(dec!(81.987)), "+81.99");
    }

    #[test]
    fn qty_trims_trailing_zeros_and_point() {
        assert_eq!(qty(dec!(0.01)), "0.01");
        assert_eq!(qty(dec!(0.1) + dec!(0.2)), "0.3");
        assert_eq!(qty(dec!(1.0)), "1");
    }
}
