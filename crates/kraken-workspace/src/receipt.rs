//! One trading-response schema across paper and live execution.
//!
//! Stable identity stays in the flattened core; backend-specific data remains
//! in the optional `venue` and `paper` blocks.

use kraken_core::OrderSide;
use rust_decimal::Decimal;
use serde::Serialize;
use serde_with::skip_serializing_none;

/// Which backend executed the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TradeMode {
    Paper,
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus {
    /// Resting on the book (or accepted by the venue without a fill yet).
    Submitted,
    Filled,
    Cancelled,
    /// `--validate`: checked, never executed.
    Validated,
}

/// The substitutable identity every trading response carries, mode-agnostic.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct ReceiptCore {
    pub mode: TradeMode,
    pub status: ReceiptStatus,
    pub order_id: Option<String>,
    pub pair: Option<String>,
    pub side: Option<OrderSide>,
    #[serde(rename = "type")]
    pub order_type: Option<String>,
    #[serde(with = "rust_decimal::serde::str_option")]
    pub volume: Option<Decimal>,
    #[serde(with = "rust_decimal::serde::str_option")]
    pub price: Option<Decimal>,
}

/// One order command's receipt.
#[skip_serializing_none]
#[derive(Debug, Serialize)]
pub struct OrderReceipt {
    #[serde(flatten)]
    pub core: ReceiptCore,
    /// The raw venue response, verbatim — live mode only, never parsed into
    /// shape (prose fields like `descr` stay the venue's own).
    pub venue: Option<serde_json::Value>,
    /// The simulator's fill economics — paper mode only, honest paper shapes.
    pub paper: Option<PaperFill>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct PaperFill {
    pub trade_id: Option<String>,
    #[serde(with = "rust_decimal::serde::str_option")]
    pub fee: Option<Decimal>,
    #[serde(with = "rust_decimal::serde::str_option")]
    pub cost: Option<Decimal>,
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn live_and_paper_receipts_share_one_core_shape() {
        let core = |mode| ReceiptCore {
            mode,
            status: ReceiptStatus::Filled,
            order_id: Some("X".into()),
            pair: Some("BTCUSD".into()),
            side: Some(OrderSide::Buy),
            order_type: Some("market".into()),
            volume: Some(dec!(0.01)),
            price: Some(dec!(50000)),
        };
        let paper = serde_json::to_value(OrderReceipt {
            core: core(TradeMode::Paper),
            venue: None,
            paper: Some(PaperFill {
                trade_id: Some("PAPER-00002".into()),
                fee: Some(dec!(1.3)),
                cost: Some(dec!(500)),
            }),
        })
        .expect("serialize");
        let live = serde_json::to_value(OrderReceipt {
            core: core(TradeMode::Live),
            venue: Some(serde_json::json!({"txid": ["X"]})),
            paper: None,
        })
        .expect("serialize");

        // Scripts depend on these identity keys remaining mode-independent.
        for key in [
            "mode", "status", "order_id", "pair", "side", "type", "volume",
        ] {
            assert!(paper.get(key).is_some(), "paper receipt lacks {key}");
            assert!(live.get(key).is_some(), "live receipt lacks {key}");
        }
        assert_eq!(paper["volume"], "0.01", "money is a string on the wire");
    }
}
