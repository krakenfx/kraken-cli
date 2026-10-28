//! Spot vocabulary shared by several endpoints. The enums Kraken grows without
//! notice carry `Unrecognized`; the rest reject a token they do not know.

use serde::{Deserialize, Serialize};

/// An asset's or pair's class. Open: Kraken adds classes as it lists products.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetClass {
    Currency,
    Equity,
    EquityPair,
    Forex,
    Nft,
    TokenizedAsset,
    Volume,
    FuturesContract,
    Derivatives,
    #[serde(untagged, deserialize_with = "crate::primitives::unrecognized")]
    Unrecognized(String),
}

/// How a tokenized asset's balance is stated across a rebase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RebaseMultiplier {
    /// After applying the current multiplier: the default.
    Rebased,
    /// In the asset's base units.
    Base,
}

/// An order's execution model, as Kraken reports it. Wider than what the CLI
/// can place: the exchange also reports types it creates itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OrderType {
    Market,
    Limit,
    Iceberg,
    StopLoss,
    TakeProfit,
    StopLossLimit,
    TakeProfitLimit,
    TrailingStop,
    TrailingStopLimit,
    SettlePosition,
    StopLossProfit,
    StopLossProfitLimit,
    StopLossAndLimit,
    LiquidatePosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    Pending,
    Open,
    Closed,
    Canceled,
    Expired,
    CancelRequestedWithVendor,
    PendingAmend,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeInForce {
    /// Kraken's default; an order row that omits the field is
    /// good-till-cancelled.
    #[default]
    Gtc,
    Ioc,
    Gtd,
    Fok,
    Day,
    DayExtended,
    Unknown,
}

/// The price a triggered order watches: the same tokens on REST as on WS.
pub use kraken_core::TriggerReference as Trigger;

/// A note Kraken attaches to an order row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderNote {
    Stopped,
    Touched,
    Liquidated,
    Partial,
    Creditliquidated,
    Amended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderFlag {
    /// Fee in base currency.
    Fcib,
    /// Fee in quote currency.
    Fciq,
    /// No market price protection.
    Nompp,
    /// Post-only.
    Post,
    /// Volume in quote currency.
    Viqc,
    Iceberg,
    TriggeredByAutomation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionStatus {
    Open,
    Closed,
}

/// What moved a ledger entry. Open: Kraken adds entry types as it adds
/// products.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LedgerType {
    None,
    Trade,
    Deposit,
    Withdrawal,
    Transfer,
    Margin,
    Adjustment,
    Rollover,
    Spend,
    Receive,
    Settled,
    Credit,
    Staking,
    Reward,
    Dividend,
    Sale,
    Conversion,
    Nfttrade,
    Nftcreatorfee,
    Nftrebate,
    Custodytransfer,
    #[serde(untagged, deserialize_with = "crate::primitives::unrecognized")]
    Unrecognized(String),
}
