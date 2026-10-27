//! The `ohlc` channel: candles at a fixed interval.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/ohlc>.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// One `ohlc` candle.
///
/// `timestamp` is deprecated on the wire (superseded by `interval_begin`) and
/// so optional; the recorder falls back to `interval_begin` when it is absent.
///
/// No `deny_unknown_fields`: an extensible inbound payload (see the `ticker`
/// rationale) — an exchange-added field is ignored, not a frame-dropping decode
/// error.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OhlcData {
    pub symbol: String,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub vwap: Decimal,
    pub trades: u64,
    pub volume: Decimal,
    pub interval_begin: String,
    pub interval: u32,
    pub timestamp: Option<String>,
}

impl OhlcData {
    /// The deprecated wire `timestamp` when present, else `interval_begin` —
    /// the one event-time rule every consumer shares, so a recording's write
    /// and read sides can't disagree.
    pub fn event_ts(&self) -> &str {
        self.timestamp.as_deref().unwrap_or(&self.interval_begin)
    }
}
