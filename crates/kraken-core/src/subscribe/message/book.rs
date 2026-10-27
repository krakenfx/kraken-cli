//! The `book` channel: order-book snapshots and incremental updates.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/book>. A `snapshot` carries
//! the full requested depth; an `update` carries only the changed levels — a
//! level with `qty == 0` means "remove this price". `checksum` is the CRC32 of
//! the top 10 levels per side.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// One price level in a `book` frame: `price` and the aggregated `qty` at it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: Decimal,
    pub qty: Decimal,
}

/// One `book` snapshot/update entry.
///
/// No `deny_unknown_fields`: an extensible inbound payload (see the `ticker`
/// rationale) — an exchange-added field is ignored, not a frame-dropping decode
/// error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookData {
    pub symbol: String,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    pub checksum: u32,
    pub timestamp: String,
}
