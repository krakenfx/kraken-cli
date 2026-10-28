//! The `level3` channel: the full order-by-order book.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/level3>. A snapshot lists the
//! resting orders per side; an update lists order events
//! (`add`/`modify`/`delete`), so `event` is absent on snapshot orders. The
//! entry-level `timestamp` the schema describes is absent in captured payloads,
//! hence optional.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// The book mutation an update entry describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Level3Event {
    Add,
    Modify,
    Delete,
    /// Vocabulary Kraken adds later; re-serializes as `"unknown"`, not the
    /// original token.
    #[serde(other)]
    Unknown,
}

/// One book side's resting order (snapshot) or order event (update).
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Level3Order {
    /// Update only; absent on snapshot orders.
    pub event: Option<Level3Event>,
    pub order_id: String,
    pub limit_price: Decimal,
    pub order_qty: Decimal,
    pub timestamp: String,
    /// Wire fields outside the pinned schema, preserved verbatim (see
    /// [`ExtraFields`](super::ExtraFields)).
    #[serde(flatten)]
    pub extra: super::ExtraFields,
}

/// One `level3` entry: a symbol's per-order book state or change (see the
/// module schema).
///
/// No `deny_unknown_fields`: an extensible inbound payload — a field Kraken
/// adds later is ignored rather than failing the decode, which would drop every
/// frame on the channel.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Level3Data {
    pub symbol: String,
    /// CRC32 over the top price levels, for client-side book verification.
    pub checksum: u32,
    pub timestamp: Option<String>,
    pub bids: Vec<Level3Order>,
    pub asks: Vec<Level3Order>,
    /// Wire fields outside the pinned schema, preserved verbatim (see
    /// [`ExtraFields`](super::ExtraFields)).
    #[serde(flatten)]
    pub extra: super::ExtraFields,
}

impl Level3Data {
    /// Whether any channel-local enum field decoded through its `Unknown`
    /// fallback.
    pub(crate) fn has_unknown_vocabulary(&self) -> bool {
        self.bids
            .iter()
            .chain(&self.asks)
            .any(|order| order.event == Some(Level3Event::Unknown))
    }
}
