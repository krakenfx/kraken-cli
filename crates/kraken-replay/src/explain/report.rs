//! Public response model for `kraken explain pnl`.
//!
//! The additive JSON contract reuses paper-engine vocabulary; decomposition
//! logic remains in [`super`].

use chrono::{DateTime, Utc};
use kraken_core::OrderSide;
use kraken_paper::PaperOrderType;
use kraken_paper::account::Origin;
use rust_decimal::Decimal;
use serde::Serialize;
use serde_with::skip_serializing_none;

/// A reconciled P&L explanation with counterfactuals kept outside the sum.
#[skip_serializing_none]
#[derive(Debug, Serialize)]
pub struct PnlReport {
    /// Output-envelope discriminators: `group` names the command surface
    /// (distinct from an account's execution `mode` of paper/live), `type` the
    /// result kind.
    pub group: &'static str,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub session: String,
    pub strategy: Option<String>,
    pub anchor: Anchor,
    pub components: Vec<ComponentLine>,
    pub trades: Vec<TradeLine>,
    pub missed_fills: MissedFillSection,
    pub attribution: Vec<OriginSlice>,
    pub window: Option<WindowSummary>,
    /// Earlier `Initialized`/`Reset` epochs on the journal, not decomposed —
    /// only the final epoch is (parity with what `session stop` folded).
    pub epochs_skipped: usize,
    pub caveats: Vec<String>,
}

/// Authoritative stop totals from the final account epoch.
#[derive(Debug, Serialize)]
pub struct Anchor {
    pub starting_balance: Decimal,
    pub final_value: Decimal,
    pub total_pnl: Decimal,
    pub ended_at: DateTime<Utc>,
    pub currency: String,
}

/// One term in `price_movement + fees + spread + slippage + residual = total_pnl`.
#[derive(Debug, Serialize)]
pub struct ComponentLine {
    pub kind: ComponentKind,
    /// Missing when unavailable; the residual absorbs it and a caveat explains why.
    pub amount: Option<Decimal>,
    /// One sentence of what this number means for *this* session.
    pub explanation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentKind {
    PriceMovement,
    Fees,
    Spread,
    Slippage,
    Residual,
}

/// One fill's contributions to the components, in the anchor currency.
#[skip_serializing_none]
#[derive(Debug, Serialize)]
pub struct TradeLine {
    pub trade_id: String,
    pub order_id: String,
    pub pair: String,
    pub side: OrderSide,
    pub order_type: PaperOrderType,
    pub volume: Decimal,
    pub price: Decimal,
    /// The fill's cost converted to the anchor currency; `None` when the
    /// quote asset has no recorded end mark (the whole fill sits in the
    /// residual).
    pub notional: Option<Decimal>,
    /// Mid of the recorded reference quote at fill time; `None` = no valid
    /// recorded quote (that fill contributes zero spread/slippage).
    pub mid: Option<Decimal>,
    /// This fill's price-movement contribution; `None` = its base or quote
    /// asset has no recorded end mark (folded into the residual).
    pub price_movement: Option<Decimal>,
    pub fees: Decimal,
    pub spread: Decimal,
    pub slippage: Decimal,
    pub origin: Origin,
    /// The decision log's rationale, joined by order id (`--reason`).
    pub reason: Option<String>,
}

/// The counterfactual section: what resting limit orders *might* have done.
/// Never part of the component sum — a cancelled order contributed $0 to
/// actual P&L.
#[derive(Debug, Serialize)]
pub struct MissedFillSection {
    /// The honesty preamble: how the engine actually fills, and why this
    /// scan is a recording-based proxy that can disagree with it.
    pub assumptions: &'static str,
    pub orders: Vec<MissedFill>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize)]
pub struct MissedFill {
    pub order_id: String,
    pub pair: String,
    pub side: OrderSide,
    pub volume: Decimal,
    pub limit_price: Decimal,
    pub resting_from: DateTime<Utc>,
    /// Cancellation instant, or the recording's window end while still
    /// open; absent when neither is known (unfinalized capture).
    pub resting_until: Option<DateTime<Utc>>,
    pub outcome: MissedOutcome,
    pub coverage: Coverage,
    /// Why the recording cannot answer the question (uncovered only).
    pub uncovered_reason: Option<String>,
    /// Whether a recorded ticker frame crossed the limit while resting.
    pub crossed: Option<bool>,
    pub first_crossed_at: Option<DateTime<Utc>>,
    /// The hypothetical fill marked at the recorded end, net of fees.
    pub hypothetical_pnl: Option<Decimal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum MissedOutcome {
    Cancelled,
    StillOpen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    Covered,
    Uncovered,
}

/// One origin's share of the activity: who (CLI vs agent) traded, paid, and
/// decided what. A limit fill belongs to whoever *submitted* the order —
/// the fill record's origin is whichever later command happened to
/// reconcile it.
#[derive(Debug, Serialize)]
pub struct OriginSlice {
    pub origin: Origin,
    pub trades: usize,
    /// Σ fill cost in the anchor currency; fills whose quote asset has no
    /// end mark are excluded (their whole effect sits in the residual).
    pub gross_notional: Decimal,
    pub fees: Decimal,
    pub spread: Decimal,
    pub slippage: Decimal,
    /// Fills whose order id joins a logged decision (`--reason`).
    pub decided: usize,
}

/// What the recorded market did over the session — the whole story for a
/// zero-trade session.
#[skip_serializing_none]
#[derive(Debug, Serialize)]
pub struct WindowSummary {
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    pub symbols: Vec<SymbolWindow>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize)]
pub struct SymbolWindow {
    /// Canonical `BASEQUOTE` built from the folded base/quote (XBT→BTC) —
    /// alias pairs therefore differ from the decision log's alias-preserving
    /// canon (`XBTUSD` stays `XBTUSD` there).
    pub symbol: String,
    pub first_mid: Option<Decimal>,
    pub last_mid: Option<Decimal>,
    pub move_pct: Option<Decimal>,
    /// Mean recorded ticker spread; `None` when no ticker was recorded.
    pub avg_spread: Option<Decimal>,
    pub frames: usize,
    pub end_mark_source: Option<EndMarkSource>,
}

/// Where a symbol's end-of-recording mark came from, best first: the mark
/// mirrors `compute_portfolio_value`'s bid marking, so the residual stays
/// interpretable as purely "live vs recorded".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndMarkSource {
    TickerBid,
    TradePrice,
    OhlcClose,
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn vocabulary_serializes_snake_case() {
        for (value, expected) in [
            (
                serde_json::to_string(&ComponentKind::PriceMovement).unwrap(),
                "\"price_movement\"",
            ),
            (
                serde_json::to_string(&MissedOutcome::StillOpen).unwrap(),
                "\"still_open\"",
            ),
            (
                serde_json::to_string(&Coverage::Uncovered).unwrap(),
                "\"uncovered\"",
            ),
            (
                serde_json::to_string(&EndMarkSource::TickerBid).unwrap(),
                "\"ticker_bid\"",
            ),
        ] {
            assert_eq!(value, expected);
        }
    }

    #[test]
    fn engine_enums_normalize_to_snake_case_on_the_report_wire() {
        // The reused engine enums spell snake_case at the source; this pins
        // the report's public contract against a casing regression there.
        let line = TradeLine {
            trade_id: "PAPER-00002".into(),
            order_id: "PAPER-00001".into(),
            pair: "BTCUSD".into(),
            side: OrderSide::Buy,
            order_type: PaperOrderType::Limit,
            volume: dec!(0.1),
            price: dec!(45_000.0),
            notional: Some(dec!(4_500.0)),
            mid: None,
            price_movement: None,
            fees: -dec!(1.0),
            spread: dec!(0.0),
            slippage: dec!(0.0),
            origin: Origin::Mcp,
            reason: None,
        };
        let json = serde_json::to_value(&line).unwrap();
        assert_eq!(json["side"], "buy");
        assert_eq!(json["order_type"], "limit");
        assert_eq!(json["origin"], "mcp");
    }

    #[test]
    fn report_serializes_the_pinned_top_level_keys() {
        // The JSON contract agents consume: pinned so a rename fails here,
        // not in a consumer.
        let report = PnlReport {
            group: "session",
            kind: "pnl_explanation",
            session: "btc-dip".into(),
            strategy: None,
            anchor: Anchor {
                starting_balance: dec!(10_000.0),
                final_value: dec!(10_250.5),
                total_pnl: dec!(250.5),
                ended_at: "2026-07-08T10:30:00Z".parse().unwrap(),
                currency: "USD".into(),
            },
            components: vec![ComponentLine {
                kind: ComponentKind::Residual,
                amount: Some(dec!(250.5)),
                explanation: "test".into(),
            }],
            trades: Vec::new(),
            missed_fills: MissedFillSection {
                assumptions: "test",
                orders: Vec::new(),
            },
            attribution: Vec::new(),
            window: None,
            epochs_skipped: 0,
            caveats: Vec::new(),
        };
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["group"], "session");
        assert_eq!(json["type"], "pnl_explanation");
        assert_eq!(json["session"], "btc-dip");
        assert_eq!(json["anchor"]["total_pnl"], 250.5);
        assert_eq!(json["components"][0]["kind"], "residual");
        assert!(json.get("strategy").is_none(), "absent optionals drop out");
        assert!(json.get("window").is_none());
    }
}
