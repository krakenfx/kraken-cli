//! The `trade` channel: individual executions.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/trade>.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{OrderSide, OrderType};

/// One `trade` execution entry.
///
/// `ord_type` decodes into the full [`OrderType`] — a lenient superset of the docs'
/// `limit`/`market` taker types, so an unexpected value never drops the frame.
///
/// No `deny_unknown_fields`: an extensible inbound payload (see the `ticker` rationale)
/// — an exchange-added field is ignored, not a frame-dropping decode error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeData {
    pub symbol: String,
    pub side: OrderSide,
    pub price: Decimal,
    pub qty: Decimal,
    pub ord_type: OrderType,
    pub trade_id: u64,
    pub timestamp: String,
}
