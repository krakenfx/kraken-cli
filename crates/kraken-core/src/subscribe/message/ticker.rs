//! The `ticker` channel: best bid/ask plus rolling 24h stats.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/ticker>.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// One `ticker` snapshot/update entry.
///
/// No `deny_unknown_fields`: an extensible inbound payload — a field Kraken
/// adds later is ignored rather than failing the decode, which would drop every
/// frame on the channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickerData {
    pub symbol: String,
    pub bid: Decimal,
    pub bid_qty: Decimal,
    pub ask: Decimal,
    pub ask_qty: Decimal,
    pub last: Decimal,
    pub volume: Decimal,
    pub vwap: Decimal,
    pub low: Decimal,
    pub high: Decimal,
    pub change: Decimal,
    pub change_pct: Decimal,
    pub timestamp: String,
}
