//! The `instrument` channel: reference data for all active assets and tradeable pairs —
//! symbol identifiers, precisions, trading parameters and rules.
//!
//! Source: <https://docs.kraken.com/api/docs/websocket-v2/instrument>. Unlike the other
//! channels, `data` is a single object holding two catalogues, not an array of entries.
//! Fields the docs mark conditional (margin parameters, tokenized-asset extras) are
//! `Option`s; everything else is required.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use strum::Display;

/// One asset in the reference catalogue.
///
/// No `deny_unknown_fields`: an extensible inbound payload — a field Kraken adds later is
/// ignored rather than failing the decode, which would drop every frame on the channel.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetInfo {
    pub id: String,
    pub status: AssetStatus,
    pub precision: u32,
    pub precision_display: u32,
    pub borrowable: Option<bool>,
    pub collateral_value: Option<Decimal>,
    pub margin_rate: Option<Decimal>,
    /// Fixed conversion rate of a tokenized asset.
    pub multiplier: Option<Decimal>,
    pub class: Option<String>,
    /// Wire fields outside the pinned schema, preserved verbatim (see [`ExtraFields`](super::ExtraFields)).
    #[serde(flatten)]
    pub extra: super::ExtraFields,
}

/// One tradeable pair in the reference catalogue.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairInfo {
    pub symbol: String,
    pub base: String,
    pub quote: String,
    pub status: PairStatus,
    pub qty_precision: u32,
    pub price_precision: u32,
    pub cost_precision: u32,
    pub qty_increment: Decimal,
    pub price_increment: Decimal,
    pub qty_min: Decimal,
    pub marginable: Option<bool>,
    pub has_index: Option<bool>,
    pub cost_min: Option<Decimal>,
    pub margin_initial: Option<Decimal>,
    pub position_limit_long: Option<u64>,
    pub position_limit_short: Option<u64>,
    pub ws_display_price_precision: Option<u32>,
    /// Deprecated on the wire (use `price_increment`); still sent, so still decoded.
    pub tick_size: Option<Decimal>,
    /// Wire fields outside the pinned schema, preserved verbatim (see [`ExtraFields`](super::ExtraFields)).
    #[serde(flatten)]
    pub extra: super::ExtraFields,
}

/// An asset's tradability state. Unseparated lowercase tokens on the wire
/// (`depositonly`), unlike the pair statuses' snake_case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
#[non_exhaustive]
pub enum AssetStatus {
    DepositOnly,
    Disabled,
    Enabled,
    FundingTemporarilyDisabled,
    WithdrawalOnly,
    WorkInProgress,
    /// Vocabulary Kraken adds later; re-serializes as `"unknown"`, not the original token.
    #[serde(other)]
    Unknown,
}

/// A pair's trading state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[non_exhaustive]
pub enum PairStatus {
    CancelOnly,
    Delisted,
    LimitOnly,
    Maintenance,
    Online,
    PostOnly,
    ReduceOnly,
    WorkInProgress,
    /// Vocabulary Kraken adds later; re-serializes as `"unknown"`, not the original token.
    #[serde(other)]
    Unknown,
}

/// The `data` object: the full asset and pair catalogues (snapshot), or the changed
/// subset (update).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstrumentData {
    pub assets: Vec<AssetInfo>,
    pub pairs: Vec<PairInfo>,
    /// Wire fields outside the pinned schema, preserved verbatim (see [`ExtraFields`](super::ExtraFields)).
    #[serde(flatten)]
    pub extra: super::ExtraFields,
}

impl InstrumentData {
    /// Whether any channel-local enum field decoded through its `Unknown` fallback.
    pub(crate) fn has_unknown_vocabulary(&self) -> bool {
        self.assets.iter().any(|a| a.status == AssetStatus::Unknown)
            || self.pairs.iter().any(|p| p.status == PairStatus::Unknown)
    }
}
