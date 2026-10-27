//! Incoming channel-data frames.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2>. Every frame is a
//! [`ChannelMessage`]: the shared `{channel, type, data, sequence?}` envelope around a
//! [`ChannelData`] payload, dispatched by serde on the frame's own `channel`
//! field. Every channel in the catalogue decodes into its own typed payload —
//! [`ticker`], [`trade`], [`book`], [`ohlc`], [`instrument`], [`executions`],
//! [`balances`], [`level3`], [`status`]. Strictly: there is no verbatim
//! fallback; a payload parses to its channel's schema or the frame fails.

mod balances;
mod book;
mod channel_message;
mod envelope;
mod executions;
mod instrument;
mod level3;
mod ohlc;
mod status;
mod ticker;
mod trade;

/// Catch-all for wire fields Kraken adds after a payload schema was pinned:
/// captured on decode and re-emitted on serialize, so account and reference
/// records are never silently narrowed to the pinned schema. Market payloads
/// (`ticker`/`trade`/`book`/ `ohlc`) deliberately omit it: stable pinned
/// schemas on the hot path, where `flatten`'s content buffering has a real
/// per-entry cost.
pub type ExtraFields = serde_json::Map<String, serde_json::Value>;

pub use balances::{
    BalanceData, LedgerCategory, LedgerEntryType, LedgerSubtype, WalletBalance, WalletType,
};
pub use book::{BookData, PriceLevel};
pub use channel_message::{ChannelData, ChannelMessage};
pub use envelope::MessageType;
pub use executions::{
    ContingentOrder, ExecType, ExecutionData, Fee, FeeCurrencyPreference, LiquidityIndicator,
    OrderStatus, PositionStatus, TriggerState, TriggerStatus,
};
pub use instrument::{AssetInfo, AssetStatus, InstrumentData, PairInfo, PairStatus};
pub use level3::{Level3Data, Level3Event, Level3Order};
pub use ohlc::OhlcData;
pub use status::{StatusData, SystemStatus};
pub use ticker::TickerData;
pub use trade::TradeData;
