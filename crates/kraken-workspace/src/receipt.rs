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

/// The journal records where fills come from; a receipt reports which backend
/// produced them. Same distinction, two vocabularies — bridged here so the
/// renderers derive one mode label from either.
impl From<kraken_paper::FillSource> for TradeMode {
    fn from(source: kraken_paper::FillSource) -> Self {
        match source {
            kraken_paper::FillSource::Simulated => Self::Paper,
            kraken_paper::FillSource::Venue => Self::Live,
        }
    }
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
